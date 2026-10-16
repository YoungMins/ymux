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
    for (re, rep) in high_confidence().iter().chain(generic()) {
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
}
