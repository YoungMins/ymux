use super::{
    index::FileSummary,
    model::{HourBucket, TokenSnapshot, TokenTotals, UsageRow},
    retired::{static_provider, Retired},
};
use std::collections::{BTreeMap, HashMap};

/// Heatmap history: 53 weeks.
pub(super) const HOURLY_WINDOW_SECS: u64 = 371 * 86400;

/// One indexed transcript and the account whose root listed it.
pub(super) struct FileView<'a> {
    pub account: usize,
    pub provider: &'static str,
    pub file: &'a FileSummary,
}

/// An event that survived dedup, borrowing its strings from the summaries.
struct Counted<'a> {
    account: usize,
    provider: &'static str,
    timestamp: u64,
    model: &'a str,
    project: &'a str,
    tokens: TokenTotals,
    /// `(time, total)` that ranks this event for naming its project row.
    rank: (u64, u64),
    /// Folded retired history: counts toward all-time only (its hours and
    /// age are already accounted for) and is never "in the future".
    old: bool,
}

/// `accounts` maps `FileView::account` to the owning account id. Views must be
/// listed default-account first: dedup is global, so a transcript copied into
/// a profile stays attributed to whoever listed it first.
///
/// Claude: one record per message key across all files (a fork or resume
/// copies earlier messages under a new session id, the key is the message
/// id alone), the largest wins and the first listed wins a tie. Codex: a
/// session's checkpoints are cumulative, so every file's checkpoints are merged
/// per session in time order and only increases count, as the difference from
/// the previous total (a future-dated checkpoint counts at `collected_at`).
///
/// `retired` is the history of transcripts that no longer exist. It wins every
/// dedup: a live Claude message whose key is retired is skipped (a larger live
/// copy was folded into the retired record by `Index` before this runs), and a
/// Codex session starts from its retired total. It is added as pre-aggregated rows
/// and hours plus its few recent events, so the work here does not grow with
/// the number of retired events.
pub(super) fn aggregate(
    snapshot: &mut TokenSnapshot,
    files: &[FileView],
    accounts: &[String],
    retired: &Retired,
) {
    let mut accounts = accounts.to_vec();
    let mut account_of = |name: &str| {
        accounts.iter().position(|a| a == name).unwrap_or_else(|| {
            accounts.push(name.to_owned());
            accounts.len() - 1
        })
    };
    let mut winners: Vec<(usize, usize)> = Vec::new();
    let mut slot: HashMap<u64, usize> = HashMap::new();
    let mut sessions: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
    for (fi, view) in files.iter().enumerate() {
        for (ei, event) in view.file.events.iter().enumerate() {
            if view.provider == "claude" {
                if retired.has_claude(event.k) {
                    continue;
                }
                match slot.get(&event.k) {
                    Some(&at) => {
                        let (wf, we) = winners[at];
                        if event.t > files[wf].file.events[we].t {
                            winners[at] = (fi, ei);
                        }
                    }
                    None => {
                        slot.insert(event.k, winners.len());
                        winners.push((fi, ei));
                    }
                }
            } else {
                let Some(session) = view.file.sessions.get(event.n as usize) else {
                    continue;
                };
                sessions.entry(session).or_default().push((fi, ei));
            }
        }
    }
    let counted = |fi: usize, ei: usize, tokens: TokenTotals, timestamp: u64| {
        let view = &files[fi];
        let event = &view.file.events[ei];
        Counted {
            account: view.account,
            provider: view.provider,
            timestamp,
            model: view
                .file
                .models
                .get(event.m as usize)
                .map_or("unknown", String::as_str),
            project: view
                .file
                .projects
                .get(event.p as usize)
                .map_or("unknown", String::as_str),
            tokens,
            rank: (event.at, tokens.total),
            old: false,
        }
    };
    let mut events: Vec<Counted> = winners
        .iter()
        .map(|&(fi, ei)| {
            let event = &files[fi].file.events[ei];
            counted(fi, ei, event.tokens(), event.at)
        })
        .collect();
    for (session, mut checkpoints) in sessions {
        checkpoints.sort_by_key(|&(fi, ei)| {
            let event = &files[fi].file.events[ei];
            (event.at, event.t)
        });
        let mut previous = retired.previous(session);
        for (fi, ei) in checkpoints {
            let event = &files[fi].file.events[ei];
            let totals = event.tokens();
            if totals.total <= previous.total {
                continue;
            }
            let delta = totals.difference(previous);
            previous = totals;
            // A future-dated checkpoint counts now: it is cumulative, so
            // hiding it would hide every later one's base too (and
            // retiring it must not make the figures jump).
            events.push(counted(fi, ei, delta, event.at.min(snapshot.collected_at)));
        }
    }
    for event in retired.recent() {
        let Some(provider) = static_provider(&event.provider) else {
            continue;
        };
        events.push(Counted {
            account: account_of(&event.account),
            provider,
            timestamp: event.at,
            model: &event.model,
            project: &event.project,
            tokens: event.tokens,
            rank: (event.at, event.tokens.total),
            old: false,
        });
    }
    for row in retired.rollup() {
        let Some(provider) = static_provider(&row.provider) else {
            continue;
        };
        events.push(Counted {
            account: account_of(&row.account),
            provider,
            timestamp: row.first_at,
            model: &row.model,
            project: &row.project,
            tokens: row.tokens,
            rank: (row.first_at, row.first_total),
            old: true,
        });
    }

    let hourly_start = snapshot.collected_at.saturating_sub(HOURLY_WINDOW_SECS);
    let week_start = snapshot.collected_at.saturating_sub(7 * 86400);
    let five_start = snapshot.collected_at.saturating_sub(5 * 3600);
    // Project spellings that differ only by comparison key share a row, which
    // shows the spelling of the earliest event (by time, then size) using it.
    let mut spelling_ids: HashMap<&str, usize> = HashMap::new();
    let mut key_ids: HashMap<String, usize> = HashMap::new();
    let mut keys: Vec<String> = Vec::new();
    let mut shown: Vec<(&str, (u64, u64))> = Vec::new();
    let mut rows: HashMap<(usize, &'static str, &str, usize), UsageRow> = HashMap::new();
    let mut hours: BTreeMap<(u64, usize, &'static str), u64> = BTreeMap::new();
    // A retired hour is included whole if it overlaps the window.
    for (hour, account, provider, total) in retired.hours_from(hourly_start / 3600) {
        // A corrupt store can hold any hour: overflow would panic (and abort
        // a release build), so an hour that does not fit is skipped.
        if hour
            .checked_mul(3600)
            .map_or(true, |at| at > snapshot.collected_at)
        {
            continue;
        }
        let Some(provider) = static_provider(provider) else {
            continue;
        };
        let bucket = hours
            .entry((hour, account_of(account), provider))
            .or_default();
        *bucket = bucket.saturating_add(total);
    }
    for event in &events {
        if event.timestamp > snapshot.collected_at && !event.old {
            continue;
        }
        let tokens = event.tokens;
        // Hour-aligned like `retired.hours_from`: the heatmap's oldest hour is
        // whole for live and folded data alike.
        if !event.old && tokens.total > 0 && event.timestamp / 3600 >= hourly_start / 3600 {
            let bucket = hours
                .entry((event.timestamp / 3600, event.account, event.provider))
                .or_default();
            *bucket = bucket.saturating_add(tokens.total);
        }
        let kid = *spelling_ids.entry(event.project).or_insert_with(|| {
            let key = ypath::comparison_key(event.project);
            *key_ids.entry(key.clone()).or_insert_with(|| {
                keys.push(key);
                shown.push((event.project, (u64::MAX, u64::MAX)));
                keys.len() - 1
            })
        });
        let rank = event.rank;
        if rank < shown[kid].1 {
            shown[kid] = (event.project, rank);
        }
        let row = rows
            .entry((event.account, event.provider, event.model, kid))
            .or_insert_with(|| UsageRow {
                account: accounts.get(event.account).cloned().unwrap_or_default(),
                provider: event.provider,
                model: event.model.to_owned(),
                project: String::new(),
                five_hour: TokenTotals::default(),
                seven_day: TokenTotals::default(),
                all_time: TokenTotals::default(),
            });
        row.all_time.add(tokens);
        // Folded rows are older than every window by construction; a clock
        // that jumped back must not pull them into one.
        if !event.old && event.timestamp >= week_start {
            row.seven_day.add(tokens);
        }
        if !event.old && event.timestamp >= five_start {
            row.five_hour.add(tokens);
        }
    }
    let mut ordered: BTreeMap<(usize, &str, &str, &str), UsageRow> = BTreeMap::new();
    for ((account, provider, model, kid), mut row) in rows {
        shown[kid].0.clone_into(&mut row.project);
        ordered.insert((account, provider, model, keys[kid].as_str()), row);
    }
    snapshot.hourly = hours
        .into_iter()
        .map(|((hour, account, provider), total)| HourBucket {
            hour,
            account: accounts.get(account).cloned().unwrap_or_default(),
            provider,
            total,
        })
        .collect();
    snapshot.rows = ordered.into_values().collect();
    for row in &snapshot.rows {
        if let Some(at) = card_for(&snapshot.providers, row.provider, &row.account) {
            let provider = &mut snapshot.providers[at];
            provider.five_hour.add(row.five_hour);
            provider.seven_day.add(row.seven_day);
            provider.all_time.add(row.all_time);
        }
    }
}

/// The card a row's tokens belong to. Retired history keeps the account it
/// had when it was retired, which may no longer own a card (removed account,
/// environment change, "none"), so: the card of that account; else the card
/// whose directory that account shares (`also`); else the default account's
/// card for the provider (history of an account that is gone stays visible
/// where the user's main usage is); else the provider's first card.
fn card_for(providers: &[super::model::ProviderUsage], id: &str, account: &str) -> Option<usize> {
    let of_provider = || providers.iter().enumerate().filter(|(_, p)| p.id == id);
    of_provider()
        .find(|(_, p)| p.account == account)
        .or_else(|| of_provider().find(|(_, p)| p.also.iter().any(|a| a == account)))
        .or_else(|| of_provider().find(|(_, p)| p.account.is_empty()))
        .or_else(|| of_provider().next())
        .map(|(at, _)| at)
}
