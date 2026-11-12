//! Secret redaction, sensitive-path detection and char-safe clipping.
//!
//! Everything here is total: no function panics on any input, which matters
//! because release builds abort on panic (CLAUDE.md rule 13). Redaction is
//! best-effort pattern matching, not a guarantee; callers still keep prompts
//! out of outbound payloads by default.

use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

/// What a redacted span is replaced with.
pub const REDACTED: &str = "[REDACTED]";

/// `(pattern, replacement)`. Order matters: block-shaped secrets (PEM) and
/// URLs run before the generic key/value and long-run rules so the generic
/// ones don't chew a half-redacted string.
const PATTERNS: &[(&str, &str)] = &[
    // PEM private key blocks; an unterminated block (clipped input) runs to the end.
    (
        r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)",
        REDACTED,
    ),
    // Chat webhook URLs carry their secret in the path.
    (
        r"https://(?:ptb\.|canary\.)?discord(?:app)?\.com/api/(?:v\d+/)?webhooks/\S+",
        REDACTED,
    ),
    (r"https://hooks\.slack\.com/\S+", REDACTED),
    (
        r"https://[A-Za-z0-9.-]*(?:webhook\.office\.com|logic\.azure\.com|powerplatform\.com)/\S+",
        REDACTED,
    ),
    // `scheme://user:pass@host`: keep the scheme and host, drop the userinfo.
    (r"(://)[^\s/@]+@", "${1}[REDACTED]@"),
    // Vendor token prefixes.
    (r"\bsk-ant-[A-Za-z0-9_-]{8,}", REDACTED),
    (r"\bsk-[A-Za-z0-9_-]{16,}", REDACTED),
    (r"\bgh[pousr]_[A-Za-z0-9]{20,}", REDACTED),
    (r"\bgithub_pat_[A-Za-z0-9_]{20,}", REDACTED),
    (r"\bxox[abprs]-[A-Za-z0-9-]{10,}", REDACTED),
    (r"\bAKIA[0-9A-Z]{16}\b", REDACTED),
    (r"\bAIza[0-9A-Za-z_-]{35}", REDACTED),
    // Stripe secret / restricted keys, GitLab PAT, HuggingFace, npm, Google OAuth.
    (r"\b[sr]k_(?:live|test)_[A-Za-z0-9]{8,}", REDACTED),
    (r"\bglpat-[A-Za-z0-9_-]{16,}", REDACTED),
    (r"\bhf_[A-Za-z0-9]{20,}", REDACTED),
    (r"\bnpm_[A-Za-z0-9]{30,}", REDACTED),
    (r"\bya29\.[A-Za-z0-9_-]{20,}", REDACTED),
    // JWT.
    (
        r"\beyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}",
        REDACTED,
    ),
    // `Authorization: Bearer ...`.
    (r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]{8,}", REDACTED),
    // Other authorization schemes, and a bare `Basic <base64>`.
    (
        r#"(?i)(authorization["']?\s*[:=]\s*["']?)(?:basic|token|digest|negotiate|apikey)\s+[^\s"']+"#,
        "${1}[REDACTED]",
    ),
    (r"\bBasic\s+[A-Za-z0-9+/]{16,}={0,2}", REDACTED),
    // `curl -u user:pass`, `--user user:pass`.
    (
        r#"((?:^|\s)(?:-u|--user|--proxy-user)(?:=|\s+)['"]?)[^\s:'"]+:[^\s'"]+"#,
        "${1}[REDACTED]",
    ),
    // `--password hunter2`, `--api-key abc` (the value is the next word; a
    // following flag is not a value).
    (
        r#"(?i)(--?[\w-]*(?:password|passwd|secret|token|api[_-]?key)[\w-]*)(\s+)("[^"\n]*"|'[^'\n]*'|[^\s'"-]\S*)"#,
        "${1}${2}[REDACTED]",
    ),
    // `password=...`, `"api_key": "..."`, `GITHUB_TOKEN=...`,
    // `AWS_SECRET_ACCESS_KEY=...`: any name that contains a keyword, a quoted
    // value (spaces allowed) or a bare word. The name stays.
    (
        r#"(?i)([\w.-]*(?:password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credential)[\w.-]*)(["']?\s*[:=]\s*)("[^"\n]*"|'[^'\n]*'|\S+)"#,
        "${1}${2}[REDACTED]",
    ),
    // Short key names that only count when assigned: `DB_PWD=...`, `PASS=...`,
    // `AUTH=...`, `"auth": "..."`. A bare `:` after an unquoted word is prose
    // ("Auth: failed"), and `bypass=`/`compass=` have no separator before the
    // keyword, so neither is touched.
    (
        r#"(?i)(\b(?:[\w.-]*[_.-])?(?:pwd|pass|auth|cookie)(?:["']\s*[:=]\s*|\s*=\s*))("[^"\n]*"|'[^'\n]*'|\S+)"#,
        "${1}[REDACTED]",
    ),
    // `Cookie: ...` / `Set-Cookie: ...` header lines (up to a closing quote).
    (
        r#"(?i)(\b(?:set-)?cookie\s*:\s*)[^\r\n"']+"#,
        "${1}[REDACTED]",
    ),
    // `mysql -u root -pHunter2 db`: `-p` glued to its value, for the MySQL
    // family only (`-p` means other things elsewhere, e.g. `cargo test -p x`).
    (
        r#"\b((?i:mysql|mysqldump|mysqladmin|mariadb|mariadb-dump)(?:\.exe)?\b[^\n|;&]*?\s-p)("[^"\n]*"|'[^'\n]*'|[^\s'"]\S*)"#,
        "${1}[REDACTED]",
    ),
];

/// Low-confidence shape rules, applied after [`PATTERNS`]: long opaque runs
/// (hex digests/keys, base64-ish blobs). They also match git SHAs and long
/// markdown rules, so they redact but never block (see
/// [`contains_high_confidence_secret`]).
const GENERIC_PATTERNS: &[(&str, &str)] = &[
    // No `/` in the class, so long file paths are not swallowed.
    (r"\b[A-Fa-f0-9]{40,}\b", REDACTED),
    (r"[A-Za-z0-9+_-]{40,}={0,2}", REDACTED),
];

fn compile(list: &'static [(&'static str, &'static str)]) -> Vec<(Regex, &'static str)> {
    list.iter()
        .filter_map(|(p, r)| Regex::new(p).ok().map(|re| (re, *r)))
        .collect()
}

fn high_confidence() -> &'static [(Regex, &'static str)] {
    static CELL: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    CELL.get_or_init(|| compile(PATTERNS))
}

fn generic() -> &'static [(Regex, &'static str)] {
    static CELL: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    CELL.get_or_init(|| compile(GENERIC_PATTERNS))
}

/// Maximal runs of characters a token can contain, for the AWS-secret check.
fn token_runs() -> Option<&'static Regex> {
    static CELL: OnceLock<Option<Regex>> = OnceLock::new();
    CELL.get_or_init(|| Regex::new(r"[A-Za-z0-9/+=_.-]+").ok())
        .as_ref()
}

/// An AWS secret access key: exactly 40 base64 characters (`/` and `+`
/// allowed), mixed case with a digit. The mixed-case-and-digit requirement
/// keeps a 40-character path or word from matching.
fn aws_secret_shaped(run: &str) -> bool {
    run.len() == 40
        && run
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'/' || b == b'+')
        && run.bytes().any(|b| b.is_ascii_uppercase())
        && run.bytes().any(|b| b.is_ascii_lowercase())
        && run.bytes().any(|b| b.is_ascii_digit())
}

/// Maximal `[A-Za-z0-9+/]` runs with up to two `=` of padding: base64 keys,
/// which (unlike the generic rule's class) may contain `/`.
fn base64_runs() -> Option<&'static Regex> {
    static CELL: OnceLock<Option<Regex>> = OnceLock::new();
    CELL.get_or_init(|| Regex::new(r"[A-Za-z0-9+/]{40,}={0,2}").ok())
        .as_ref()
}

/// A base64 key that contains `/` (Azure storage keys and the like), which
/// the generic rule would split at the slash. Kept narrow so paths are left
/// alone: mixed case plus a digit, no leading `/` or `//`, at most one `/`
/// per eight characters, and no run of 10+ lowercase letters (a word).
fn base64_slash_key_shaped(run: &str) -> bool {
    let body = run.trim_end_matches('=');
    let b = body.as_bytes();
    b.len() >= 40
        && b.contains(&b'/')
        && b[0] != b'/'
        && !body.contains("//")
        && b.iter().filter(|c| **c == b'/').count() * 8 <= b.len()
        && b.iter().any(u8::is_ascii_uppercase)
        && b.iter().any(u8::is_ascii_lowercase)
        && b.iter().any(u8::is_ascii_digit)
        && !b.split(|c| !c.is_ascii_lowercase()).any(|w| w.len() >= 10)
}

/// Redact [`base64_slash_key_shaped`] runs that stand alone (not glued to a
/// longer `.`/`_`/`-` token).
fn redact_base64_slash_keys(s: &str) -> String {
    let Some(re) = base64_runs() else {
        return s.to_owned();
    };
    let glued = |c: Option<u8>| c.is_some_and(|c| matches!(c, b'.' | b'_' | b'-'));
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for m in re.find_iter(s) {
        let before = m.start().checked_sub(1).map(|i| bytes[i]);
        if base64_slash_key_shaped(m.as_str())
            && !glued(before)
            && !glued(bytes.get(m.end()).copied())
        {
            out.push_str(&s[last..m.start()]);
            out.push_str(REDACTED);
            last = m.end();
        }
    }
    out.push_str(&s[last..]);
    out
}

/// Does `s` contain something that is almost certainly a secret: a PEM block,
/// a vendor-prefixed token, a JWT, a `key=value` credential, an AWS secret key
/// shape, or a Bearer/Basic credential? Unlike `redact(s) != s` this ignores
/// the generic long-run rules, so git SHAs and long markdown rules pass.
pub fn contains_high_confidence_secret(s: &str) -> bool {
    if high_confidence().iter().any(|(re, _)| re.is_match(s)) {
        return true;
    }
    token_runs().is_some_and(|re| re.find_iter(s).any(|m| aws_secret_shaped(m.as_str())))
}

/// Replace every secret-looking span in `s` with [`REDACTED`].
pub fn redact(s: &str) -> String {
    let mut out = s.to_owned();
    for (re, rep) in high_confidence() {
        if let std::borrow::Cow::Owned(next) = re.replace_all(&out, *rep) {
            out = next;
        }
    }
    // Before the generic rules, which would split such a key at its `/`.
    out = redact_base64_slash_keys(&out);
    for (re, rep) in generic() {
        if let std::borrow::Cow::Owned(next) = re.replace_all(&out, *rep) {
            out = next;
        }
    }
    if let Some(re) = token_runs() {
        if let std::borrow::Cow::Owned(next) = re.replace_all(&out, |c: &regex::Captures| {
            let run = &c[0];
            if aws_secret_shaped(run) {
                REDACTED.to_owned()
            } else {
                run.to_owned()
            }
        }) {
            out = next;
        }
    }
    out
}

/// Redact every string inside a JSON value, in place (object keys are kept).
pub fn redact_json(v: &mut Value) {
    match v {
        Value::String(s) => *s = redact(s),
        Value::Array(a) => a.iter_mut().for_each(redact_json),
        Value::Object(o) => o.values_mut().for_each(redact_json),
        _ => {}
    }
}

/// `true` for paths that must never be attached, listed or quoted: env files,
/// key material, credential stores, and anything under `.git`/`.ssh`/`.aws`.
pub fn is_sensitive_path(path: &str) -> bool {
    let norm = path.replace('\\', "/").to_lowercase();
    if norm
        .split('/')
        .any(|c| c == ".git" || c == ".ssh" || c == ".aws")
    {
        return true;
    }
    let name = norm.rsplit('/').next().unwrap_or("");
    name.starts_with(".env")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.starts_with("id_rsa")
        || name.starts_with("id_ed25519")
        || name == ".npmrc"
        || name == ".netrc"
        || name.contains("credentials")
        || name.contains("secret")
}

/// At most `n` chars of `s`, cut on a char boundary; an over-long input ends
/// in `…` (the ellipsis counts toward `n`).
pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    if n == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(n - 1).collect();
    out.push('…');
    out
}

/// The first line of `s`, clipped to `n` chars.
pub fn first_line_clipped(s: &str, n: usize) -> String {
    clip(s.lines().next().unwrap_or("").trim_end(), n)
}

/// The last `n` chars of `s` (for output tails), on a char boundary.
pub fn tail(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        return s.to_owned();
    }
    s.chars().skip(count - n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_compiles() {
        assert_eq!(high_confidence().len(), PATTERNS.len());
        assert_eq!(generic().len(), GENERIC_PATTERNS.len());
    }

    #[test]
    fn high_confidence_ignores_shas_and_long_rules() {
        let sha = "commit 0123456789abcdef0123456789abcdef01234567 fixed";
        let rule = format!(
            "{}
",
            "-".repeat(60)
        );
        let blob = "A".repeat(50);
        for s in [sha, rule.as_str(), blob.as_str(), "plain prose"] {
            assert!(!contains_high_confidence_secret(s), "{s}");
        }
        assert_ne!(redact(sha), sha, "redact still scrubs the digest");
    }

    #[test]
    fn high_confidence_flags_real_secrets() {
        for s in [
            "-----BEGIN RSA PRIVATE KEY-----
abc",
            "key sk-ant-abcdefgh12345678",
            "ghp_abcdefghijklmnopqrstuv",
            "eyJhbGciOi.eyJzdWIiOi.abcdefghij",
            "password=hunter2",
            "Authorization: Bearer abcdefgh12345",
            "Basic dXNlcjpwYXNzd29yZDEyMzQ1",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        ] {
            assert!(contains_high_confidence_secret(s), "{s}");
        }
    }

    #[test]
    fn redacts_token_prefixes() {
        for s in [
            "sk-ant-api03-abcdefgh12345678",
            "sk-abcdefghijklmnopqrstuv",
            "ghp_abcdefghijklmnopqrstuvwxyz0123",
            "github_pat_11ABCDEFG0abcdefghijklmn",
            "xoxb-1234567890-abcdefghij",
            "AKIAABCDEFGHIJKLMNOP",
            "AIzaSyA-abcdefghijklmnopqrstuvwxyz01234",
        ] {
            let out = redact(&format!("use {s} now"));
            assert_eq!(out, format!("use {REDACTED} now"), "input {s}");
        }
    }

    #[test]
    fn redacts_jwt_bearer_and_pem() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.c2lnbmF0dXJl";
        assert_eq!(redact(jwt), REDACTED);
        assert_eq!(
            redact("Authorization: Bearer abc123def456"),
            "Authorization: [REDACTED]"
        );
        let pem = "a\n-----BEGIN RSA PRIVATE KEY-----\nMIIE\nxyz\n-----END RSA PRIVATE KEY-----\nb";
        assert_eq!(redact(pem), "a\n[REDACTED]\nb");
        // Clipped block with no END line still goes.
        let cut = "x -----BEGIN PRIVATE KEY-----\nMIIEvQ";
        assert_eq!(redact(cut), "x [REDACTED]");
    }

    #[test]
    fn redacts_key_value_but_keeps_key() {
        assert_eq!(redact("password=hunter2"), "password=[REDACTED]");
        assert_eq!(redact("API_KEY: abc"), "API_KEY: [REDACTED]");
        assert_eq!(redact(r#""token": "abc","#), r#""token": [REDACTED],"#);
        assert_eq!(
            redact("export GITHUB_TOKEN=zzz"),
            "export GITHUB_TOKEN=[REDACTED]"
        );
    }

    #[test]
    fn redacts_url_userinfo_and_webhooks() {
        assert_eq!(
            redact("git clone https://bob:pw@example.com/r.git"),
            "git clone https://[REDACTED]@example.com/r.git"
        );
        assert_eq!(
            redact("post https://discord.com/api/webhooks/123/abc-DEF"),
            "post [REDACTED]"
        );
        assert_eq!(
            redact("https://hooks.slack.com/services/T0/B0/xyz"),
            REDACTED
        );
        assert_eq!(
            redact("https://contoso.webhook.office.com/webhookb2/abc"),
            REDACTED
        );
    }

    #[test]
    fn redacts_long_runs_but_not_paths_or_words() {
        let hex = "a".repeat(40);
        assert_eq!(redact(&format!("sha {hex}")), format!("sha {REDACTED}"));
        let b64 = "Qm9v".repeat(12);
        assert_eq!(redact(&b64), REDACTED);
        let path = "/home/user/projects/some-long-directory/another-long-directory/src/main.rs";
        assert_eq!(redact(path), path);
        assert_eq!(redact("cargo test -p ymux"), "cargo test -p ymux");
    }

    #[test]
    fn redaction_is_idempotent_and_total() {
        let once = redact("password=x sk-ant-abcdefgh1234 héllo 한글");
        assert_eq!(redact(&once), once);
        assert_eq!(redact(""), "");
    }

    #[test]
    fn redact_json_walks_nested_strings() {
        let mut v = serde_json::json!({"a": ["token=abc", {"b": "fine"}], "n": 3});
        redact_json(&mut v);
        assert_eq!(v["a"][0], "token=[REDACTED]");
        assert_eq!(v["a"][1]["b"], "fine");
        assert_eq!(v["n"], 3);
    }

    #[test]
    fn sensitive_paths() {
        for p in [
            ".env",
            "/r/.env.local",
            "C:\\r\\server.pem",
            "/r/tls.key",
            "/home/u/.ssh/config",
            "/r/id_rsa",
            "/r/id_ed25519.pub",
            "/r/.npmrc",
            "/r/.netrc",
            "/r/aws-credentials.json",
            "/r/my_secrets.toml",
            "/r/.git/config",
            "C:\\Users\\u\\.aws\\config",
        ] {
            assert!(is_sensitive_path(p), "{p}");
        }
        for p in [
            "/r/src/main.rs",
            "/r/.gitignore",
            "/r/environment.md",
            "/r/README.md",
        ] {
            assert!(!is_sensitive_path(p), "{p}");
        }
    }

    #[test]
    fn clip_respects_char_boundaries() {
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("abcd", 3), "ab…");
        assert_eq!(clip("한글한글한글", 4), "한글한…");
        assert_eq!(clip("abc", 0), "");
        assert_eq!(first_line_clipped("one\ntwo", 10), "one");
        assert_eq!(tail("héllo wörld", 5), "wörld");
    }

    #[test]
    fn redacts_suffix_keyword_names() {
        assert_eq!(
            redact("AWS_SECRET_ACCESS_KEY=abcd/efgh"),
            "AWS_SECRET_ACCESS_KEY=[REDACTED]"
        );
        assert_eq!(redact("DB_PASSWORD: hunter2"), "DB_PASSWORD: [REDACTED]");
        assert_eq!(
            redact("export STRIPE_SECRET_KEY=sk_live_x"),
            "export STRIPE_SECRET_KEY=[REDACTED]"
        );
        assert_eq!(redact("client_secret=zz"), "client_secret=[REDACTED]");
    }

    #[test]
    fn redacts_quoted_values_with_spaces() {
        assert_eq!(
            redact(r#"password="correct horse battery staple" next"#),
            "password=[REDACTED] next"
        );
        assert_eq!(redact("api_key='two words' x"), "api_key=[REDACTED] x");
        assert_eq!(
            redact(r#"{"password": "a b c", "user": "bob"}"#),
            r#"{"password": [REDACTED], "user": "bob"}"#
        );
    }

    #[test]
    fn redacts_cli_secret_flags() {
        assert_eq!(
            redact("tool --password hunter2 --verbose"),
            "tool --password [REDACTED] --verbose"
        );
        assert_eq!(redact("x --token abc123"), "x --token [REDACTED]");
        assert_eq!(redact("x --api-key 'k k'"), "x --api-key [REDACTED]");
        assert_eq!(redact("x --secret=abc"), "x --secret=[REDACTED]");
        // A flag after the keyword flag is not its value.
        assert_eq!(redact("x --token --other"), "x --token --other");
    }

    #[test]
    fn redacts_auth_headers_and_basic_credentials() {
        assert_eq!(
            redact("curl -H 'Authorization: Basic dXNlcjpwYXNzd29yZA==' u"),
            "curl -H 'Authorization: [REDACTED]' u"
        );
        assert_eq!(
            redact("Authorization: Token abc123def456"),
            "Authorization: [REDACTED]"
        );
        assert_eq!(
            redact("send Basic dXNlcjpwYXNzd29yZA== now"),
            "send [REDACTED] now"
        );
        assert_eq!(redact("Basic knowledge helps"), "Basic knowledge helps");
    }

    #[test]
    fn redacts_curl_user_pass() {
        assert_eq!(
            redact("curl -u admin:s3cret https://x.io"),
            "curl -u [REDACTED] https://x.io"
        );
        assert_eq!(
            redact("curl --user 'bob:pw' x"),
            "curl --user '[REDACTED]' x"
        );
        assert_eq!(redact("git push -u origin main"), "git push -u origin main");
    }

    #[test]
    fn redacts_aws_secret_shaped_runs() {
        let secret = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        assert_eq!(secret.len(), 40);
        assert_eq!(redact(&format!("key {secret} end")), "key [REDACTED] end");
        assert_eq!(redact(&format!("\"{secret}\"")), "\"[REDACTED]\"");
        // 40 characters of path without mixed case + digit are left alone.
        let path = "/usr/local/share/applications/foo/bar/ba";
        assert_eq!(path.len(), 40);
        assert_eq!(redact(path), path);
        // Idempotent.
        let once = redact(secret);
        assert_eq!(redact(&once), once);
    }

    #[test]
    fn redacts_more_vendor_tokens() {
        // Prefix and body are joined at run time: a literal that has a real
        // vendor token's shape trips GitHub's push protection, fake or not.
        for (prefix, body) in [
            ("sk_live_", "51HabcdefGhijklMNOP"),
            ("sk_test_", "51HabcdefGhijklMNOPqrstuv"),
            ("rk_live_", "51HabcdefGhijklMNOP"),
            ("glpat-", "xxxxxxxxxxxxxxxxxxxx"),
            ("hf_", "abcdefghijklmnopqrstuvwxyzABCDEFGH"),
            ("npm_", "abcdefghijklmnopqrstuvwxyz0123456789"),
            ("ya29.", "a0AfH6SMBabcdefghijklmnop_qrstuv"),
            ("xoxp-", "1234567890-abcdefghij"),
            ("github_pat_", "11ABCDEFG0abcdefghijklmn_opqrstuv"),
        ] {
            let s = &format!("{prefix}{body}");
            assert_eq!(
                redact(&format!("use {s} now")),
                format!("use {REDACTED} now"),
                "{s}"
            );
            assert!(contains_high_confidence_secret(s), "{s}");
        }
    }

    #[test]
    fn redacts_short_secret_key_names_and_cookies() {
        assert_eq!(redact("DB_PWD=hunter2 run"), "DB_PWD=[REDACTED] run");
        assert_eq!(redact("PASS=hunter2"), "PASS=[REDACTED]");
        assert_eq!(redact("export AUTH=abc123"), "export AUTH=[REDACTED]");
        assert_eq!(redact("x?auth=abc&y=1"), "x?auth=[REDACTED]");
        assert_eq!(redact(r#"{"auth": "abc def"}"#), r#"{"auth": [REDACTED]}"#);
        assert_eq!(redact("Cookie: sid=abc; theme=dark"), "Cookie: [REDACTED]");
        assert_eq!(
            redact("Set-Cookie: sid=abc; HttpOnly\nnext"),
            "Set-Cookie: [REDACTED]\nnext"
        );
        assert_eq!(
            redact("curl -H 'Cookie: sid=abc' x"),
            "curl -H 'Cookie: [REDACTED]' x"
        );
        assert_eq!(redact("COOKIE=abc"), "COOKIE=[REDACTED]");
        let once = redact("PASS=x Cookie: a=b");
        assert_eq!(redact(&once), once);
    }

    #[test]
    fn redacts_mysql_glued_password_only() {
        assert_eq!(
            redact("mysql -u root -pHunter2 db"),
            "mysql -u root -p[REDACTED] db"
        );
        assert_eq!(
            redact("mysqldump -h x -p'two words' db"),
            "mysqldump -h x -p[REDACTED] db"
        );
        assert_eq!(redact("mariadb -pS3cret"), "mariadb -p[REDACTED]");
        // Prompting form, other tools and the uppercase port flag stay.
        assert_eq!(redact("mysql -u root -p db"), "mysql -u root -p db");
        assert_eq!(redact("mysql -P3306 -h x"), "mysql -P3306 -h x");
        assert_eq!(redact("cargo test -pymux"), "cargo test -pymux");
        assert_eq!(redact("pip install -pfoo"), "pip install -pfoo");
    }

    #[test]
    fn redacts_base64_keys_with_slashes_but_not_paths() {
        let azure = "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";
        assert_eq!(
            redact(&format!("AccountKey={azure};")),
            format!("AccountKey={REDACTED};")
        );
        assert_eq!(
            redact(&format!("key {azure} end")),
            format!("key {REDACTED} end")
        );
        let once = redact(azure);
        assert_eq!(redact(&once), once);
        for path in [
            "/home/user/projects/some-long-directory/another-long-directory/src/main.rs",
            "src/features/authentication/components/LoginForm2/useLoginFormState",
            "node_modules/@types/react-dom/Something/AnotherLongSegmentName1/index",
            "Users/Alice/Documents2/Projects/MyApplication/src/components/App",
        ] {
            assert_eq!(redact(path), path, "{path}");
        }
    }

    #[test]
    fn ordinary_text_is_not_over_redacted() {
        for s in [
            "Please reset your password later",
            "bypass the auth layer, then compass=north",
            "Auth: failed for user bob",
            "the cookie jar is empty",
            "pass --verbose; passthrough=1",
            "git log 0123456789abcdef0123456789abcdef0123456 -n 3",
            "hf_model and npm_config are names",
            "ya29 is a prefix, sk_live_ too",
        ] {
            assert_eq!(redact(s), s, "{s}");
        }
    }
}
