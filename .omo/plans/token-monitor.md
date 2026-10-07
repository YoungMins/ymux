# yMux token monitor implementation

## Resolved scope

- Claude Code and Codex CLI local transcript token totals for rolling five hours, seven days, and all retained records.
- Provider, model, and project grouping; input, output, cache read, and cache write breakdown.
- Claude whole-account OAuth quota percentages with five-minute caching and failure backoff. Codex latest recorded quota snapshots.
- Explicit credential-presence, unavailable, partial-data, and expired/stale indications. Local histories may contain prior accounts.
- Guarded backend IPC, Linux-safe pure collector, top + launcher and palette entries for a persistent compact pane, 13 translations, and three README guides.

## Execution

- [x] Implement typed JSONL parsers and duplicate-safe cumulative aggregation.
- [x] Add bounded scans, newest-file-first provider budgets, metadata cache and eviction.
- [x] Add Claude quota adapter; keep credentials backend-only and use bounded HTTPS requests.
- [x] Implement a saved compact pane, polling cleanup, selected-period/group tables, and discovery.
- [x] Verify parser, quota, UI behavior, type checking, production build, and command compilation.

## Evidence

See ../evidence/token-monitor.md for verification results and platform limitations.
