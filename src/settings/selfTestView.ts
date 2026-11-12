// "Test agent tracking": the button, the checklist and "Copy report" that sit
// under the agent-tracking toggle in Settings -> General. The backend does
// the work (hook_http::selftest); this only renders what it returns.

import { t } from "../i18n/i18n";
import { agentApi, describeError } from "../ipc/bridge";
import { itemLabelKey, statusKey, type SelfTestReport, type SelfTestStatus } from "./selfTestModel";

type Tone = "" | "ok" | "warn" | "error";

const TONE: Record<SelfTestStatus, Tone> = { ok: "ok", warn: "warn", fail: "error", skip: "" };

function line(text: string, tone: Tone = ""): HTMLElement {
  const e = document.createElement("div");
  e.className = `act-note${tone ? ` act-note--${tone}` : ""}`;
  e.textContent = text;
  return e;
}

function button(label: string): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "hotkey-modal__btn";
  b.textContent = label;
  return b;
}

async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/// A block (hint, button, result list) to append under the tracking toggle.
export function renderSelfTest(): HTMLElement {
  const host = document.createElement("div");
  host.className = "settings-row__hint";
  host.appendChild(line(t("settings.activity.selfTestHint")));

  const actions = document.createElement("div");
  actions.className = "act-actions";
  const run = button(t("settings.activity.selfTest"));
  const copy = button(t("settings.activity.selfTestCopy"));
  copy.hidden = true;
  actions.append(run, copy);
  host.appendChild(actions);

  const results = document.createElement("div");
  host.appendChild(results);

  let report: SelfTestReport | null = null;

  run.addEventListener("click", () => {
    run.disabled = true;
    copy.hidden = true;
    results.replaceChildren(line(t("settings.activity.selfTestRunning")));
    agentApi
      .selfTest()
      .then((r) => {
        report = r;
        results.replaceChildren(
          ...r.items.map((i) =>
            line(
              `${t(statusKey(i.status))} — ${t(itemLabelKey(i.id))}: ${i.detail}`,
              TONE[i.status],
            ),
          ),
        );
        copy.hidden = false;
      })
      .catch((e) => {
        results.replaceChildren(
          line(`${t("settings.activity.selfTestFailed")} ${describeError(e)}`, "error"),
        );
      })
      .finally(() => {
        run.disabled = false;
      });
  });

  copy.addEventListener("click", () => {
    if (!report) return;
    void copyText(report.text).then((ok) => {
      if (!ok) return;
      const label = copy.textContent;
      copy.textContent = t("settings.activity.selfTestCopied");
      window.setTimeout(() => {
        copy.textContent = label;
      }, 1500);
    });
  });
  return host;
}
