// Pure helpers for the "Test agent tracking" checklist (Settings -> General).
// The backend (hook_http::selftest) returns the items and a plain-text report;
// this only decides how they are labelled and summarised.

/** Mirror of `hook_http::selftest::Item`. */
export interface SelfTestItem {
  id: string;
  status: SelfTestStatus;
  /** Short English detail: ports, paths, fixed phrases. Never a secret. */
  detail: string;
}

export type SelfTestStatus = "ok" | "warn" | "fail" | "skip";

/** Mirror of `hook_http::selftest::Report`. `text` is the copyable report. */
export interface SelfTestReport {
  items: SelfTestItem[];
  text: string;
}

/** Check ids the backend can send; anything else gets the generic label. */
const KNOWN_IDS = [
  "tracking",
  "receiver",
  "ping",
  "roundtrip",
  "wrong_token",
  "origin",
  "hooks_dir",
  "claude",
] as const;

/** i18n key of a check's label. */
export function itemLabelKey(id: string): string {
  return (KNOWN_IDS as readonly string[]).includes(id)
    ? `settings.activity.st.${id}`
    : "settings.activity.st.receiver";
}

/** i18n key of a status word. */
export function statusKey(status: SelfTestStatus): string {
  return `settings.activity.st.${status}`;
}

/** The worst status in the list; "skip" never counts against it. */
export function worstStatus(items: readonly SelfTestItem[]): SelfTestStatus {
  if (items.some((i) => i.status === "fail")) return "fail";
  if (items.some((i) => i.status === "warn")) return "warn";
  return "ok";
}
