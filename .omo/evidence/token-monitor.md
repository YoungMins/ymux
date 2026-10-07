# Token monitor verification

## Persistent compact pane update

- Top workspace + launcher and command palette add a saved token_usage pane. Bottom token button and modal removed.
- Panel tests: 12 pass; related layout and tree tests: 37 pass; TypeScript check passes.
- Nested Rust TOML token_usage pane serialization test and no-desktop library/test compile pass.
- Browser mock-data QA: inspected 320px and 620px panels; narrow panel switches seven-day/model selections without overflow. Temporary preview resources removed.
- Five review lanes pass after fixing initial hidden-tab data fetching. Hidden panels defer reads and cleanup releases timers/listeners.
- Final build uses target/token-pane because target/release/ymux.exe is currently running and locked by Windows.
- Final Windows release EXE and MSI build succeeded at target/token-pane/release.

## Earlier collector implementation

- Pure token collector: 10 tests pass (duplicate streamed Claude messages and copied Codex sessions, cumulative deltas, rolling boundaries, out-of-order/future observations, bounded reads, invalid/oversized input, platform-aware project grouping).
- Claude quota adapter: 5 tests pass (percentages/reset parsing, invalid responses, credential handling, bounded response reads, identity-isolated stale cache).
- Frontend panel/model: 9 tests pass (aggregation, all 13 translations, safe text, lifecycle, failure translation, selected period, expired quotas).
- Full frontend regression before final targeted fixes: 726 tests passed. Final targeted panel tests, tsc and production build passed.
- cargo check --no-default-features --lib --tests -p ymux passed; workspace Clippy with -D warnings passed.
- Real local collector smoke: Claude and Codex records produced grouped totals and partial-data warnings. No credentials or transcript content were printed. Temporary example removed.
- Browser preview with representative mock records: visually inspected Korean panel; selected seven-day period and model grouping changed displayed values; Escape removed panel. Temporary HTML/server/tab removed.
- Five review lanes audited goals, code, security, context and executable QA; findings fixed and rechecked.

## Limits

- Full pure Rust regression previously reported 481 passed, 8 pre-existing Windows OSC 7 path-separator failures, 1 ignored.
- macOS Keychain access and a native Tauri runtime session were not exercised on this Windows host.
- Browser preview uses mock data; live Claude network quota success was not validated against this account.
- Production build has an existing large-chunk warning.
