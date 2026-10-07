use super::model::{Event, TokenSnapshot, TokenTotals, UsageRow};
use std::collections::BTreeMap;

pub(super) fn aggregate(snapshot: &mut TokenSnapshot, events: Vec<Event>) {
    let mut unique: BTreeMap<(&str, &str, &str), &Event> = BTreeMap::new();
    for event in &events {
        let key = (event.provider, event.session.as_str(), event.id.as_str());
        let entry = unique.entry(key).or_insert(event);
        if event.tokens.total > entry.tokens.total {
            *entry = event;
        }
    }
    let mut rows: BTreeMap<(&str, &str, String), UsageRow> = BTreeMap::new();
    let mut projects: BTreeMap<String, String> = BTreeMap::new();
    let mut ordered: Vec<_> = unique.into_values().collect();
    ordered.sort_by_key(|event| (event.timestamp, event.tokens.total));
    let mut cumulative: BTreeMap<&str, TokenTotals> = BTreeMap::new();
    for event in ordered {
        if event.timestamp > snapshot.collected_at {
            continue;
        }
        let tokens = if event.provider == "codex" {
            let previous = cumulative.entry(&event.session).or_default();
            if event.tokens.total <= previous.total {
                continue;
            }
            let delta = event.tokens.difference(*previous);
            *previous = event.tokens;
            delta
        } else {
            event.tokens
        };
        let project_key = ypath::comparison_key(&event.project);
        let project = projects
            .entry(project_key.clone())
            .or_insert_with(|| event.project.clone());
        let row = rows
            .entry((event.provider, &event.model, project_key))
            .or_insert_with(|| UsageRow {
                provider: event.provider,
                model: event.model.clone(),
                project: project.clone(),
                five_hour: TokenTotals::default(),
                seven_day: TokenTotals::default(),
                all_time: TokenTotals::default(),
            });
        row.all_time.add(tokens);
        if event.timestamp >= snapshot.collected_at.saturating_sub(7 * 86400) {
            row.seven_day.add(tokens);
        }
        if event.timestamp >= snapshot.collected_at.saturating_sub(5 * 3600) {
            row.five_hour.add(tokens);
        }
    }
    snapshot.rows = rows.into_values().collect();
    for row in &snapshot.rows {
        if let Some(provider) = snapshot.providers.iter_mut().find(|p| p.id == row.provider) {
            provider.five_hour.add(row.five_hour);
            provider.seven_day.add(row.seven_day);
            provider.all_time.add(row.all_time);
        }
    }
}
