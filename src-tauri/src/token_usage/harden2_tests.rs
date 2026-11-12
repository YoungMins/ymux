//! More hardening tests (round 2): attribution, Codex, unreliable listings,
//! migrations, card totals, clocks, two-phase retirement, store size.
use super::collect::collect_with;
use super::disk::{self, Budget, Listing, Presence};
use super::harden_tests::settle_all;
use super::index::{
    decode_shard, migrate_shard, shard_path, Decoded, FileSummary, Index, StoredEvent,
    INDEX_VERSION,
};
use super::index_tests::{env, fresh, line, roots, snap, total, Env, NOW};
use super::retired::{decode_retired, migrate_retired, Candidate, Retired, RETIRED_FORMAT};
use super::{scan_roots, ScanRoot, TokenSnapshot};
use crate::accounts::AgentDirs;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

const HEADER: &str = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"sess\",\"cwd\":\"/c\"}}\n";

/// The versions before `current` (kept out of a constant range so a layout
/// still at its first version does not trip the empty-range lint).
fn older(current: u32) -> std::ops::Range<u32> {
    1..current
}

fn cp(env: &Env, name: &str) -> PathBuf {
    env.claude.join("projects").join("p").join(name)
}

fn count(time: &str, total: u64) -> String {
    format!("{{\"timestamp\":\"{time}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":{total},\"total_tokens\":{total}}}}}}}}}\n")
}

fn figures(s: &TokenSnapshot) -> String {
    let cards: Vec<_> = s
        .providers
        .iter()
        .map(|p| (p.id, &p.account, p.five_hour, p.seven_day, p.all_time))
        .collect();
    serde_json::to_string(&(cards, &s.rows, &s.hourly)).unwrap()
}

fn rfc3339(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

struct Accounts {
    roots: Vec<ScanRoot>,
    /// `<dir>/claude/projects/p` of each non-default account, in order.
    claude: Vec<PathBuf>,
}

/// The default account (`env`) plus `names`, each with its own directories.
fn accounts(env: &Env, names: &[&str]) -> Accounts {
    let mut dirs = vec![(
        String::new(),
        AgentDirs {
            claude: env.claude.display().to_string(),
            codex: env.codex.display().to_string(),
            claude_env: None,
        },
    )];
    let mut claude = Vec::new();
    for name in names {
        let base = env._dir.path().join(name);
        fs::create_dir_all(base.join("claude/projects/p")).unwrap();
        fs::create_dir_all(base.join("codex/sessions")).unwrap();
        claude.push(base.join("claude/projects/p"));
        dirs.push((
            (*name).to_owned(),
            AgentDirs {
                claude: base.join("claude").display().to_string(),
                codex: base.join("codex").display().to_string(),
                claude_env: None,
            },
        ));
    }
    Accounts {
        roots: scan_roots(&dirs),
        claude,
    }
}

fn collect(index: &mut Index, roots: &[ScanRoot]) -> TokenSnapshot {
    collect_with(index, roots, NOW, Budget::unlimited())
}

fn shards_in(env: &Env) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(&env.index_dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|e| e == "json")
                        && p.file_stem().is_some_and(|s| s.len() == 16)
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn bak_path(env: &Env) -> PathBuf {
    env.index_dir.join("retired.json.bak")
}

fn store_path(env: &Env) -> PathBuf {
    env.index_dir.join("retired.json")
}

// ---- A3: attribution does not depend on when the file was deleted -------------

#[test]
fn attribution_is_identical_whether_a_file_is_deleted_while_running_or_while_closed() {
    for victim in ["first", "second"] {
        let mut results = Vec::new();
        for online in [true, false] {
            let env = env();
            let acc = accounts(&env, &["x"]);
            let a = cp(&env, "a.jsonl");
            let b = acc.claude[0].join("b.jsonl");
            // The same message in both accounts' files, equally large, plus
            // one message each.
            let shared = line("s", "2026-09-21T00:00:00Z", "m", 10);
            fs::write(
                &a,
                shared.clone() + &line("s", "2026-09-21T01:00:00Z", "ua", 1),
            )
            .unwrap();
            fs::write(&b, shared + &line("t", "2026-09-21T02:00:00Z", "ub", 2)).unwrap();
            let mut index = fresh(Some(&env.index_dir));
            let before = figures(&collect(&mut index, &acc.roots));
            let gone = if victim == "first" { &a } else { &b };
            let after = if online {
                fs::remove_file(gone).unwrap();
                collect(&mut index, &acc.roots)
            } else {
                drop(index);
                fs::remove_file(gone).unwrap();
                collect(&mut fresh(Some(&env.index_dir)), &acc.roots)
            };
            assert_eq!(figures(&after), before, "victim {victim} online {online}");
            results.push(figures(&after));
        }
        assert_eq!(results[0], results[1], "victim {victim}");
    }
}

// ---- A: Codex ---------------------------------------------------------------

#[test]
fn a_later_discovered_earlier_codex_checkpoint_adds_nothing_and_moves_nothing() {
    let env = env();
    let x = |n: &str| env.codex.join("sessions").join(n);
    fs::write(
        x("late.jsonl"),
        format!("{HEADER}{}", count("2026-09-21T11:00:00Z", 100)),
    )
    .unwrap();
    let mut index = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut index, &env)), 100);
    fs::remove_file(x("late.jsonl")).unwrap();
    let retired = snap(&mut index, &env);
    assert_eq!(total(&retired), 100);
    // The earlier part of the same session turns up afterwards (an unplugged
    // directory returns): per-session all-time stays the maximum, 100.
    fs::write(
        x("early.jsonl"),
        format!("{HEADER}{}", count("2026-09-21T01:00:00Z", 40)),
    )
    .unwrap();
    let after = snap(&mut index, &env);
    assert_eq!(total(&after), 100);
    assert_eq!(figures(&after), figures(&retired), "nothing re-attributed");
    let hours: Vec<_> = after.hourly.iter().map(|b| (b.hour, b.total)).collect();
    assert_eq!(hours.len(), 1);
    assert_eq!(hours[0].1, 100);
    fs::remove_file(x("early.jsonl")).unwrap();
    let done = settle_all(&mut index, &env);
    assert_eq!(figures(&done), figures(&retired));
    assert_eq!(
        figures(&snap(&mut fresh(Some(&env.index_dir)), &env)),
        figures(&retired)
    );
}

#[test]
fn a_future_dated_codex_checkpoint_is_kept_at_retirement_and_the_heatmap_stays_sane() {
    let env = env();
    let p = env.codex.join("sessions").join("f.jsonl");
    fs::write(&p, format!("{HEADER}{}", count("2026-12-01T00:00:00Z", 30))).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    let live = snap(&mut index, &env);
    assert_eq!(total(&live), 30, "counted now, not hidden");
    fs::remove_file(&p).unwrap();
    let retired = snap(&mut index, &env);
    assert_eq!(
        figures(&retired),
        figures(&live),
        "retiring makes no figure jump"
    );
    assert!(retired.hourly.iter().all(|b| b.hour <= NOW / 3600));
    assert_eq!(
        figures(&snap(&mut fresh(Some(&env.index_dir)), &env)),
        figures(&live)
    );
}

// ---- B: retire only what is confirmed gone -----------------------------------

fn five_files(env: &Env) {
    for i in 0..5u64 {
        fs::write(
            cp(env, &format!("f{i}.jsonl")),
            line("s", "2026-09-21T00:00:00Z", &format!("m{i}"), i + 1),
        )
        .unwrap();
    }
}

#[test]
fn a_truncated_listing_retires_nothing_and_says_so() {
    let env = env();
    five_files(&env);
    let mut index = fresh(Some(&env.index_dir));
    let all = snap(&mut index, &env);
    assert_eq!((total(&all), all.file_limit_hit), (15, false));
    index.file_limit = 2;
    let cut = snap(&mut index, &env);
    assert_eq!(total(&cut), 15);
    assert!(cut.file_limit_hit && cut.listing_incomplete);
    assert_eq!(cut.providers[0].files, 5, "unlisted files are still held");
    assert_eq!(index.retired().key_count(), 0);
    assert!(!store_path(&env).exists());
    index.file_limit = 1_000_000;
    let healed = snap(&mut index, &env);
    assert!(!healed.file_limit_hit && !healed.listing_incomplete);
    assert_eq!(total(&healed), 15);
}

const _: () = assert!(disk::MAX_FILES >= 1_000_000);

#[test]
fn the_file_limit_is_a_safety_valve_far_above_the_old_cap() {
    let env = env();
    five_files(&env);
    let listing = disk::list_folders(&[env.claude.join("projects")], 3);
    assert!(listing.truncated && listing.incomplete);
    assert_eq!(listing.files.len(), 3);
    let whole = disk::list_folders(&[env.claude.join("projects")], 100);
    assert!(!whole.truncated && !whole.incomplete);
    assert_eq!(whole.files.len(), 5);
}

#[test]
fn a_failed_listing_retires_nothing() {
    let env = env();
    five_files(&env);
    let mut index = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut index, &env)), 15);
    index.set_lister(Box::new(|_, _| Listing {
        skipped: 1,
        incomplete: true,
        ..Listing::default()
    }));
    let got = snap(&mut index, &env);
    assert_eq!(total(&got), 15);
    assert!(got.listing_incomplete);
    assert_eq!(got.skipped_entries, 2);
    assert_eq!(index.retired().key_count(), 0);
    assert_eq!(shards_in(&env).len(), 5);
}

#[test]
fn a_root_that_cannot_be_confirmed_gone_keeps_its_files_across_restarts() {
    let env = env();
    five_files(&env);
    let mut index = fresh(Some(&env.index_dir));
    let before = snap(&mut index, &env);
    // The drive holding the root is "unplugged": the files are NotFound but
    // nothing proves they were deleted.
    fs::remove_dir_all(&env.claude).unwrap();
    index.set_probe(Box::new(|_, _| Presence::Unknown));
    assert_eq!(figures(&snap(&mut index, &env)), figures(&before));
    assert_eq!(index.retired().key_count(), 0);
    assert!(!store_path(&env).exists());
    // A restart finds orphan shards: held, still counted, not retired.
    let mut restarted = fresh(Some(&env.index_dir));
    restarted.set_probe(Box::new(|_, _| Presence::Unknown));
    for _ in 0..3 {
        assert_eq!(figures(&snap(&mut restarted, &env)), figures(&before));
    }
    assert_eq!(restarted.retired().key_count(), 0);
    assert_eq!(shards_in(&env).len(), 5);
    // Once the files are known to be gone they are retired, nothing changes.
    let mut later = fresh(Some(&env.index_dir));
    assert_eq!(figures(&snap(&mut later, &env)), figures(&before));
    assert_eq!(later.retired().key_count(), 5);
    assert_eq!(figures(&settle_all(&mut later, &env)), figures(&before));
    assert!(shards_in(&env).is_empty());
}

#[test]
fn a_vanished_hot_file_is_retired_only_after_it_is_confirmed_gone() {
    let env = env();
    let a = cp(&env, "a.jsonl");
    fs::write(&a, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    fs::write(
        cp(&env, "b.jsonl"),
        line("s", "2026-09-21T00:00:00Z", "b", 20),
    )
    .unwrap();
    let mut index = fresh(Some(&env.index_dir));
    snap(&mut index, &env);
    index.sweep_every = Duration::from_secs(3600);
    fs::remove_file(&a).unwrap();
    index.set_probe(Box::new(|_, _| Presence::Unknown));
    assert_eq!(total(&snap(&mut index, &env)), 30);
    assert_eq!(index.retired().key_count(), 0);
    index.set_probe(Box::new(disk::presence));
    assert_eq!(total(&snap(&mut index, &env)), 30);
    assert_eq!(index.retired().key_count(), 1);
}

#[test]
fn presence_tells_a_deleted_file_from_an_unreachable_root() {
    let env = env();
    let f = cp(&env, "f.jsonl");
    fs::write(&f, "x").unwrap();
    assert_eq!(disk::presence(&f, &env.claude), Presence::Present);
    fs::remove_file(&f).unwrap();
    assert_eq!(disk::presence(&f, &env.claude), Presence::Gone);
    // The root itself deleted, its parent there: gone.
    fs::remove_dir_all(&env.claude).unwrap();
    assert_eq!(disk::presence(&f, &env.claude), Presence::Gone);
    // The root's parent is missing too (a drive that is not there): unknown.
    let off = env._dir.path().join("no-such-drive").join("claude");
    assert_eq!(
        disk::presence(&off.join("projects/p/f.jsonl"), &off),
        Presence::Unknown
    );
}

#[test]
fn a_failed_orphan_pass_is_retried_not_given_up_for_the_run() {
    let env = env();
    let a = cp(&env, "a.jsonl");
    let b = cp(&env, "b.jsonl");
    fs::write(&a, line("s", "2026-09-21T00:00:00Z", "a", 10)).unwrap();
    fs::write(&b, line("s", "2026-09-21T00:00:00Z", "b", 20)).unwrap();
    drop({
        let mut index = fresh(Some(&env.index_dir));
        snap(&mut index, &env);
        index
    });
    fs::remove_file(&a).unwrap();
    fs::remove_file(&b).unwrap();
    // `b`'s shard is out of sight, and a "shard" that cannot be read is in
    // the way: the first pass fails.
    let hidden = env._dir.path().join("hidden.json");
    let b_shard = shard_path(&env.index_dir, &b);
    fs::rename(&b_shard, &hidden).unwrap();
    let poison = env.index_dir.join("ffffffffffffffff.json");
    fs::create_dir(&poison).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut index, &env)), 10);
    // The obstacle goes away, `b`'s shard returns: the next pass finds it.
    fs::remove_dir(&poison).unwrap();
    fs::rename(&hidden, &b_shard).unwrap();
    assert_eq!(total(&snap(&mut index, &env)), 30);
}

#[test]
fn an_unreadable_file_is_counted_honestly_and_does_not_keep_indexing_forever() {
    let env = env();
    five_files(&env);
    let ghost = cp(&env, "ghost.jsonl");
    let mut index = fresh(Some(&env.index_dir));
    index.set_lister(Box::new(move |spec, limit| {
        let mut l = disk::list_folders(&spec.folders, limit);
        if spec.provider == "claude" {
            l.files.push((
                ghost.clone(),
                disk::Stat {
                    len: 100,
                    mtime: Some(1),
                },
            ));
        }
        l
    }));
    let got = snap(&mut index, &env);
    assert_eq!(got.unreadable_files, 1);
    assert!(!got.indexing, "an unreadable file is not pending work");
    assert!(
        got.total_bytes >= got.indexed_bytes + 100,
        "its bytes are in the total"
    );
    assert_eq!(total(&got), 15);
}

// ---- C ------------------------------------------------------------------------

#[test]
fn a_missing_store_starts_empty_without_a_flag_and_a_good_one_is_not_flagged() {
    let env = env();
    five_files(&env);
    let mut index = fresh(Some(&env.index_dir));
    let got = snap(&mut index, &env);
    assert!(!got.retired_unavailable && got.retired_unavailable_reason.is_empty());
    fs::remove_file(cp(&env, "f0.jsonl")).unwrap();
    snap(&mut index, &env);
    let again = snap(&mut fresh(Some(&env.index_dir)), &env);
    assert!(!again.retired_unavailable);
}

#[test]
fn a_newer_store_is_never_replaced_even_with_a_good_backup_and_the_wipe_is_explicit() {
    let env = env();
    five_files(&env);
    let mut index = fresh(Some(&env.index_dir));
    snap(&mut index, &env);
    fs::remove_file(cp(&env, "f0.jsonl")).unwrap();
    settle_all(&mut index, &env);
    fs::remove_file(cp(&env, "f1.jsonl")).unwrap();
    settle_all(&mut index, &env);
    assert!(bak_path(&env).is_file());
    let newer = b"{\"format\":99,\"from\":\"a newer yMux\"}".to_vec();
    fs::write(store_path(&env), &newer).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    let got = settle_all(&mut index, &env);
    assert!(got.retired_unavailable, "downgrade: read-only");
    assert_eq!(fs::read(store_path(&env)).unwrap(), newer);
    assert_eq!(total(&got), 3 + 4 + 5, "live figures are still shown");
    // The explicit wipe is the only way out, and it keeps both files aside.
    index.reset_retired().unwrap();
    assert!(!store_path(&env).exists());
    assert!(!bak_path(&env).exists());
    let aside: Vec<_> = fs::read_dir(&env.index_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".reset-"))
        .collect();
    assert_eq!(aside.len(), 2);
    assert!(!snap(&mut index, &env).retired_unavailable);
}

// ---- D ------------------------------------------------------------------------

#[test]
fn an_unfinished_reparse_does_not_overwrite_the_complete_shard_on_disk() {
    let env = env();
    let a = cp(&env, "a.jsonl");
    let old: String = (0..60)
        .map(|i| line("s", "2026-09-21T00:00:00Z", &format!("old{i}"), 2))
        .collect();
    fs::write(&a, &old).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    assert_eq!(total(&snap(&mut index, &env)), 120);
    let new: String = (0..200)
        .map(|i| line("s", "2026-09-21T00:00:00Z", &format!("new{i}"), 1))
        .collect();
    fs::write(&a, &new).unwrap();
    assert!(collect_with(&mut index, &roots(&env), NOW, Budget::bytes(3000)).indexing);
    assert_eq!(index.superseded_count(), 1);
    // The process dies, and the file is deleted while yMux is closed.
    drop(index);
    fs::remove_file(&a).unwrap();
    assert_eq!(total(&snap(&mut fresh(Some(&env.index_dir)), &env)), 120);
}

#[test]
fn both_summaries_are_retired_when_a_file_vanishes_mid_reparse() {
    let env = env();
    let a = cp(&env, "a.jsonl");
    fs::write(&a, line("s", "2026-09-21T00:00:00Z", "kept", 7)).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    snap(&mut index, &env);
    let new: String = (0..200)
        .map(|i| line("s", "2026-09-21T00:00:00Z", &format!("new{i}"), 1))
        .collect();
    fs::write(&a, &new).unwrap();
    collect_with(&mut index, &roots(&env), NOW, Budget::bytes(3000));
    fs::remove_file(&a).unwrap();
    let got = snap(&mut index, &env);
    // The old complete summary (7) and whatever was re-read both count.
    assert!(total(&got) > 7);
    assert_eq!(index.superseded_count(), 0);
}

const SHARD_FIXTURES: &[(u32, &str)] = &[(
    1,
    r#"{"version":1,"path":"/x/a.jsonl","provider":"claude","owner":"","len":10,"mtime":null,"offset":10,"scanned":10,"fingerprint":0,"skipped":0,"session":"s","project":"p","model":"m","models":["m"],"projects":["p"],"sessions":[],"events":[{"k":1,"at":100,"i":1,"o":2,"r":0,"w":0,"t":3,"m":0,"p":0,"n":0}],"quotas":[]}"#,
)];

/// Bumping `INDEX_VERSION` without a `migrate_shard` arm for the version it
/// replaces would strand the shards of every already-deleted transcript: this
/// fails until the arm and a fixture of the old layout exist.
#[test]
fn every_older_shard_version_has_a_fixture_and_a_migration() {
    for version in older(INDEX_VERSION) {
        let (_, text) = SHARD_FIXTURES
            .iter()
            .find(|(v, _)| *v == version)
            .unwrap_or_else(|| {
                panic!("add a fixture of shard version {version} to SHARD_FIXTURES")
            });
        let value: serde_json::Value = serde_json::from_str(text).unwrap();
        let migrated = migrate_shard(version, value).unwrap_or_else(|| {
            panic!("INDEX_VERSION was bumped: migrate_shard needs an arm for version {version}")
        });
        assert_eq!(migrated.version, INDEX_VERSION);
        assert!(migrated.consistent());
        assert_eq!(
            migrated.events[0].t, 3,
            "version {version} content survives"
        );
    }
    // The fixture of the current layout decodes as it is.
    let (_, current) = SHARD_FIXTURES
        .iter()
        .find(|(v, _)| *v == INDEX_VERSION)
        .expect("add a fixture of the current shard version to SHARD_FIXTURES");
    assert!(matches!(
        decode_shard(current.as_bytes(), INDEX_VERSION, &migrate_shard),
        Decoded::Summary(_)
    ));
}

fn upgrade_v1(_from: u32, mut value: serde_json::Value) -> Option<FileSummary> {
    value["version"] = 2.into();
    serde_json::from_value(value).ok()
}

fn broken_files(env: &Env) -> Vec<PathBuf> {
    fs::read_dir(&env.index_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".broken-"))
        .map(|e| e.path())
        .collect()
}

#[test]
fn a_migratable_shard_of_a_deleted_file_is_migrated_and_retired_never_set_aside() {
    let env = env();
    five_files(&env);
    let a = cp(&env, "f4.jsonl");
    drop({
        let mut index = fresh(Some(&env.index_dir));
        snap(&mut index, &env);
        index
    });
    fs::remove_file(&a).unwrap();
    // The app is now at shard version 2, with a migration from 1.
    let mut index = fresh(Some(&env.index_dir));
    index.set_shard_version(2, upgrade_v1);
    let got = settle_all(&mut index, &env);
    assert_eq!(total(&got), 15, "f4 (5 tokens) is migrated and retired");
    assert!(index.retired().key_count() >= 1);
    assert!(broken_files(&env).is_empty());
}

#[test]
fn an_unmigratable_shard_of_a_deleted_file_is_kept_aside_with_its_bytes() {
    let env = env();
    five_files(&env);
    drop({
        let mut index = fresh(Some(&env.index_dir));
        snap(&mut index, &env);
        index
    });
    let a = cp(&env, "f4.jsonl");
    let bytes = fs::read(shard_path(&env.index_dir, &a)).unwrap();
    fs::remove_file(&a).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    index.set_shard_version(2, |_, _| None);
    snap(&mut index, &env);
    let kept = broken_files(&env);
    assert_eq!(kept.len(), 1);
    assert_eq!(fs::read(&kept[0]).unwrap(), bytes);
}

// ---- E ------------------------------------------------------------------------

#[test]
fn history_of_a_removed_account_stays_on_a_card() {
    let env = env();
    let both = accounts(&env, &["x"]);
    fs::write(
        cp(&env, "d.jsonl"),
        line("s", "2026-09-21T00:00:00Z", "d", 20),
    )
    .unwrap();
    fs::write(
        both.claude[0].join("x.jsonl"),
        line("t", "2026-09-21T00:00:00Z", "x", 10),
    )
    .unwrap();
    let mut index = fresh(Some(&env.index_dir));
    let before = collect(&mut index, &both.roots);
    assert_eq!(total(&before), 30);
    fs::remove_file(both.claude[0].join("x.jsonl")).unwrap();
    collect(&mut index, &both.roots);
    // Account x is removed from the settings: its roots are not scanned.
    let only_default = roots(&env);
    let got = collect(&mut index, &only_default);
    let rows: u64 = got.rows.iter().map(|r| r.all_time.total).sum();
    assert_eq!(rows, 30);
    assert_eq!(total(&got), 30, "the cards add up to the rows");
    assert_eq!(
        got.providers[0].all_time.total, 30,
        "on the default account's card"
    );
}

#[test]
fn a_retired_account_that_shares_a_directory_lands_on_that_card_not_the_default_one() {
    let env = env();
    let sep = accounts(&env, &["y", "x"]);
    fs::write(
        cp(&env, "d.jsonl"),
        line("s", "2026-09-21T00:00:00Z", "d", 20),
    )
    .unwrap();
    fs::write(
        sep.claude[1].join("x.jsonl"),
        line("t", "2026-09-21T00:00:00Z", "x", 10),
    )
    .unwrap();
    let mut index = fresh(Some(&env.index_dir));
    assert_eq!(total(&collect(&mut index, &sep.roots)), 30);
    fs::remove_file(sep.claude[1].join("x.jsonl")).unwrap();
    collect(&mut index, &sep.roots);
    // Now x uses y's directory.
    let y_dir = env._dir.path().join("y");
    let dirs_of = |base: &std::path::Path| AgentDirs {
        claude: base.join("claude").display().to_string(),
        codex: base.join("codex").display().to_string(),
        claude_env: None,
    };
    let shared = scan_roots(&[
        (
            String::new(),
            AgentDirs {
                claude: env.claude.display().to_string(),
                codex: env.codex.display().to_string(),
                claude_env: None,
            },
        ),
        ("y".to_owned(), dirs_of(&y_dir)),
        ("x".to_owned(), dirs_of(&y_dir)),
    ]);
    let got = collect(&mut index, &shared);
    let card = |account: &str| {
        got.providers
            .iter()
            .find(|p| p.id == "claude" && p.account == account)
            .map(|p| p.all_time.total)
    };
    assert_eq!(card("y"), Some(10));
    assert_eq!(card(""), Some(20));
}

#[test]
fn a_clock_that_jumps_back_does_not_pull_folded_history_into_the_windows() {
    let env = env();
    let p = cp(&env, "a.jsonl");
    fs::write(&p, line("s", "2026-06-01T00:00:00Z", "m", 10)).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    snap(&mut index, &env);
    fs::remove_file(&p).unwrap();
    snap(&mut index, &env);
    assert!(index.retired().recent().is_empty());
    let before_the_event =
        crate::agent_scan_disk::parse_rfc3339_secs("2026-05-30T00:00:00Z").unwrap();
    let got = collect_with(
        &mut index,
        &roots(&env),
        before_the_event,
        Budget::unlimited(),
    );
    assert_eq!(total(&got), 10);
    assert_eq!(got.rows[0].five_hour.total, 0);
    assert_eq!(got.rows[0].seven_day.total, 0);
}

#[test]
fn a_clock_that_jumps_forward_does_not_prune_the_heatmap_for_good() {
    let env = env();
    let p = cp(&env, "a.jsonl");
    fs::write(&p, line("s", "2026-09-21T00:00:00Z", "m", 10)).unwrap();
    let mut index = fresh(Some(&env.index_dir));
    let before = snap(&mut index, &env);
    fs::remove_file(&p).unwrap();
    snap(&mut index, &env);
    let far = NOW + 1000 * 86400;
    let jumped = collect_with(&mut index, &roots(&env), far, Budget::unlimited());
    assert!(jumped.hourly.is_empty());
    // The clock is corrected (in this process, then after a restart): the
    // heatmap and all-time are intact. (Folding recent events into the
    // aggregates is lossless for those two but does end their 7-day window
    // precision; that is a listed residual, not guarded.)
    let hours_and_total = |s: &TokenSnapshot| {
        (
            serde_json::to_string(&s.hourly).unwrap(),
            s.providers.iter().map(|p| p.all_time.total).sum::<u64>(),
        )
    };
    assert_eq!(
        hours_and_total(&snap(&mut index, &env)),
        hours_and_total(&before)
    );
    index.flush();
    assert_eq!(
        hours_and_total(&snap(&mut fresh(Some(&env.index_dir)), &env)),
        hours_and_total(&before)
    );
}

#[test]
fn the_heatmaps_oldest_hour_is_whole_for_live_and_retired_data_alike() {
    let env = env();
    let p = cp(&env, "a.jsonl");
    let edge_hour = (NOW - 371 * 86400) / 3600;
    let first = edge_hour * 3600 + 5;
    let last = edge_hour * 3600 + 3590;
    fs::write(
        &p,
        line("s", &rfc3339(first), "e1", 3) + &line("s", &rfc3339(last), "e2", 4),
    )
    .unwrap();
    let mut index = fresh(Some(&env.index_dir));
    let live = snap(&mut index, &env);
    assert_eq!(live.hourly.iter().map(|b| b.total).sum::<u64>(), 7);
    fs::remove_file(&p).unwrap();
    let retired = snap(&mut index, &env);
    assert_eq!(figures(&retired), figures(&live));
}

// ---- G ------------------------------------------------------------------------

/// Three files of 10, 20 and 5 tokens, the first deleted and fully settled.
fn g_setup(env: &Env) -> (Index, TokenSnapshot) {
    fs::write(
        cp(env, "a.jsonl"),
        line("s", "2026-09-21T00:00:00Z", "a", 10),
    )
    .unwrap();
    fs::write(
        cp(env, "b.jsonl"),
        line("s", "2026-09-21T01:00:00Z", "b", 20),
    )
    .unwrap();
    fs::write(
        cp(env, "c.jsonl"),
        line("s", "2026-09-21T02:00:00Z", "c", 5),
    )
    .unwrap();
    let mut index = fresh(Some(&env.index_dir));
    let before = snap(&mut index, env);
    assert_eq!(total(&before), 35);
    fs::remove_file(cp(env, "a.jsonl")).unwrap();
    settle_all(&mut index, env);
    (index, before)
}

fn restart_matches(env: &Env, before: &TokenSnapshot) -> Index {
    let mut index = fresh(Some(&env.index_dir));
    assert_eq!(figures(&snap(&mut index, env)), figures(before));
    settle_all(&mut index, env);
    assert_eq!(figures(&snap(&mut index, env)), figures(before));
    index
}

#[test]
fn a_crash_at_every_step_of_a_retirement_loses_nothing() {
    for step in [
        "bak_written",
        "store_written",
        "bak_refreshed",
        "shards_deleted",
    ] {
        let env = env();
        let (mut index, before) = g_setup(&env);
        fs::remove_file(cp(&env, "b.jsonl")).unwrap();
        index.crash_after = Some(step);
        // Phase 1 and phase 2 of the second retirement.
        for _ in 0..3 {
            snap(&mut index, &env);
        }
        drop(index);
        restart_matches(&env, &before);
        // And the aftermath is clean: no shard of a gone file is left.
        let left = shards_in(&env).len();
        assert_eq!(left, 1, "step {step}: only c's shard remains, got {left}");
    }
}

#[test]
fn the_shards_stay_until_the_store_has_been_read_back_on_a_later_refresh() {
    let env = env();
    let (mut index, before) = g_setup(&env);
    let b_shard = shard_path(&env.index_dir, &cp(&env, "b.jsonl"));
    fs::remove_file(cp(&env, "b.jsonl")).unwrap();
    snap(&mut index, &env);
    assert!(b_shard.is_file(), "phase 1 deletes nothing");
    let on_disk = decode_retired(
        &fs::read(store_path(&env)).unwrap(),
        RETIRED_FORMAT,
        &migrate_retired,
    )
    .unwrap();
    assert_eq!(on_disk.pending_deletions().len(), 1);
    snap(&mut index, &env);
    assert!(!b_shard.exists());
    let after = decode_retired(
        &fs::read(store_path(&env)).unwrap(),
        RETIRED_FORMAT,
        &migrate_retired,
    )
    .unwrap();
    assert!(after.pending_deletions().is_empty());
    // The backup is the verified store that held b before its shard went.
    let bak = decode_retired(
        &fs::read(bak_path(&env)).unwrap(),
        RETIRED_FORMAT,
        &migrate_retired,
    )
    .unwrap();
    assert_eq!(bak.key_count(), 2);
    assert_eq!(figures(&snap(&mut index, &env)), figures(&before));
}

#[test]
fn a_lost_store_is_restored_from_the_backup_and_the_surviving_shards() {
    // After a finished retirement: the backup alone is complete.
    let env = env();
    let (mut index, before) = g_setup(&env);
    fs::remove_file(cp(&env, "b.jsonl")).unwrap();
    settle_all(&mut index, &env);
    drop(index);
    fs::remove_file(store_path(&env)).unwrap();
    restart_matches(&env, &before);
    assert!(store_path(&env).is_file());

    // After phase 1 only: the backup is the previous store, b's shard is
    // still there.
    let env = self::env();
    let (mut index, before) = g_setup(&env);
    fs::remove_file(cp(&env, "b.jsonl")).unwrap();
    snap(&mut index, &env);
    drop(index);
    let garbage = b"{\"format\":2,\"claude\":[[1,2".to_vec();
    fs::write(store_path(&env), &garbage).unwrap();
    restart_matches(&env, &before);
    let kept: Vec<_> = fs::read_dir(&env.index_dir)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("retired.json.broken-")
        })
        .collect();
    assert_eq!(kept.len(), 1, "the corrupt file is kept aside");
    assert_eq!(fs::read(kept[0].path()).unwrap(), garbage);

    // No backup, corrupt store: unavailable, left alone.
    let env = self::env();
    let (index, _) = g_setup(&env);
    drop(index);
    fs::remove_file(bak_path(&env)).ok();
    fs::write(store_path(&env), &garbage).unwrap();
    let got = snap(&mut fresh(Some(&env.index_dir)), &env);
    assert!(got.retired_unavailable);
    assert_eq!(fs::read(store_path(&env)).unwrap(), garbage);
}

#[test]
fn a_lost_store_after_the_shards_were_deleted_but_before_the_list_was_cleared_is_still_complete() {
    // The backup is refreshed to the verified store BEFORE the shards go, so
    // losing retired.json at this exact point still leaves bak + shards whole.
    let env = env();
    let (mut index, before) = g_setup(&env);
    fs::remove_file(cp(&env, "b.jsonl")).unwrap();
    index.crash_after = Some("shards_deleted");
    for _ in 0..3 {
        snap(&mut index, &env);
    }
    drop(index);
    assert!(shards_in(&env).len() == 1, "b's shard is already gone");
    fs::remove_file(store_path(&env)).unwrap();
    restart_matches(&env, &before);
}

#[test]
fn a_shard_the_store_does_not_cover_is_never_deleted() {
    let env = env();
    let (mut index, _) = g_setup(&env);
    let b = cp(&env, "b.jsonl");
    let b_shard = shard_path(&env.index_dir, &b);
    fs::remove_file(&b).unwrap();
    snap(&mut index, &env);
    // The shard grows (a message the store has not seen) before phase 2.
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&b_shard).unwrap()).unwrap();
    let mut extra = value["events"][0].clone();
    extra["k"] = 777.into();
    extra["i"] = 99.into();
    extra["t"] = 99.into();
    value["events"].as_array_mut().unwrap().push(extra);
    fs::write(&b_shard, serde_json::to_vec(&value).unwrap()).unwrap();
    settle_all(&mut index, &env);
    assert!(b_shard.is_file(), "unverified content stays");
    // A restart retires it properly.
    let mut restarted = fresh(Some(&env.index_dir));
    assert_eq!(total(&settle_all(&mut restarted, &env)), 35 + 99);
    assert!(!b_shard.exists());
}

// ---- retired.json: layout, migration, size ------------------------------------

const RETIRED_FIXTURES: &[(u32, &str)] = &[
    (
        1,
        r#"{"format":1,"claude_keys":[11,22],"codex":{"s":{"input":1,"output":0,"cache_read":0,"cache_write":0,"total":1}},"recent":[],"rollup":[],"hours":[]}"#,
    ),
    (
        2,
        r#"{"format":2,"claude":[[11,100,0,1,2,3,4,10]],"ctxs":[["acct","m","/p"]],"codex":{},"recent":[],"rollup":[],"hours":[],"pending_deletions":["0123456789abcdef.json"]}"#,
    ),
];

/// Bumping `RETIRED_FORMAT` without a `migrate_retired` arm for the layout it
/// replaces would make every existing store unusable: this fails until the arm
/// and a fixture of the old layout exist.
#[test]
fn every_older_retired_format_has_a_fixture_and_a_migration() {
    for format in older(RETIRED_FORMAT) {
        let (_, text) = RETIRED_FIXTURES
            .iter()
            .find(|(f, _)| *f == format)
            .unwrap_or_else(|| {
                panic!("add a fixture of retired format {format} to RETIRED_FIXTURES")
            });
        let migrated = decode_retired(text.as_bytes(), RETIRED_FORMAT, &migrate_retired)
            .unwrap_or_else(|e| {
                panic!("RETIRED_FORMAT was bumped: migrate_retired needs an arm for format {format}: {e:?}")
            });
        // What it migrates to survives a round trip through the current layout.
        let again = decode_retired(
            &migrated.encode().unwrap(),
            RETIRED_FORMAT,
            &migrate_retired,
        )
        .unwrap();
        assert_eq!(again.key_count(), migrated.key_count());
    }
    let (_, current) = RETIRED_FIXTURES
        .iter()
        .find(|(f, _)| *f == RETIRED_FORMAT)
        .expect("add a fixture of the current retired format to RETIRED_FIXTURES");
    let r = decode_retired(current.as_bytes(), RETIRED_FORMAT, &migrate_retired).unwrap();
    assert_eq!(r.key_count(), 1);
    assert_eq!(
        r.pending_deletions(),
        vec!["0123456789abcdef.json".to_owned()]
    );
}

#[test]
fn keys_migrated_from_format_one_are_suppressed_and_never_upgraded() {
    let (_, v1) = RETIRED_FIXTURES[0];
    let mut retired = decode_retired(v1.as_bytes(), RETIRED_FORMAT, &migrate_retired).unwrap();
    assert!(retired.has_claude(11) && retired.has_claude(22));
    assert_eq!(retired.previous("s").total, 1);
    // A live copy, however large, changes nothing for a frozen key.
    let mut live = FileSummary {
        version: INDEX_VERSION,
        provider: "claude".into(),
        models: vec!["m".into()],
        projects: vec!["p".into()],
        ..FileSummary::default()
    };
    live.events.push(StoredEvent {
        k: 11,
        at: 50,
        t: 999,
        ..Default::default()
    });
    assert!(!retired.upgrade_from_live(std::iter::once(&live), NOW));
    assert!(retired.recent().is_empty() && retired.rollup().is_empty());
}

fn synthetic_claude_file(keys: u64) -> FileSummary {
    let mut s = FileSummary {
        version: INDEX_VERSION,
        path: "/x/a.jsonl".into(),
        provider: "claude".into(),
        models: vec!["claude-opus-4".into()],
        projects: vec!["/home/u/project".into()],
        ..FileSummary::default()
    };
    for i in 0..keys {
        let k = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xDEAD_BEEF_CAFE_F00D;
        s.events.push(StoredEvent {
            k,
            at: 1_780_000_000 + i * 37,
            i: 2 + i % 9,
            o: 300 + i % 4000,
            r: 20_000 + (i * 131) % 150_000,
            w: (i * 17) % 9000,
            t: 20_300 + (i * 131) % 150_000 + i % 4000 + i % 9 + (i * 17) % 9000,
            ..Default::default()
        });
    }
    s
}

fn encoded_for(keys: u64) -> (Vec<u8>, Retired) {
    let file = synthetic_claude_file(keys);
    let mut retired = Retired::default();
    // All older than the fold horizon: they go straight into aggregates.
    retired.retire(
        &[Candidate {
            owner: "work",
            summary: &file,
            pending: true,
        }],
        1_790_000_000 + 90 * 86400,
    );
    (retired.encode().unwrap(), retired)
}

#[test]
fn a_retired_claude_key_costs_a_bounded_number_of_bytes() {
    let (bytes, retired) = encoded_for(20_000);
    assert_eq!(retired.key_count(), 20_000);
    let per_key = bytes.len() / 20_000;
    assert!(per_key < 80, "{per_key} bytes per key");
    let back = decode_retired(&bytes, RETIRED_FORMAT, &migrate_retired).unwrap();
    assert_eq!(back.key_count(), 20_000);
}

#[test]
#[ignore = "prints the size of retired.json for ~420k Claude keys"]
fn measure_retired_size_for_420k_keys() {
    let started = std::time::Instant::now();
    let (bytes, retired) = encoded_for(420_000);
    println!("encode: {:?}", started.elapsed());
    println!(
        "retired.json for {} keys: {} bytes ({:.1} MB), {:.1} bytes/key",
        retired.key_count(),
        bytes.len(),
        bytes.len() as f64 / 1e6,
        bytes.len() as f64 / retired.key_count() as f64
    );
    let started = std::time::Instant::now();
    let back = decode_retired(&bytes, RETIRED_FORMAT, &migrate_retired).unwrap();
    println!(
        "decode: {:?} ({} keys)",
        started.elapsed(),
        back.key_count()
    );
}
