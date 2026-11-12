// Pane title row: the label text plus, when an account applies, a small
// coloured dot and the account name. Shared by `TerminalPane`'s own title and
// `PaneGroup`'s shared one so both read the same.

import { t } from "../i18n/i18n";
import type { AccountBadge } from "./accountModel";
import { autoLog, formatEntry, isAutoContinue } from "../autocontinue/model";
import type { Uuid } from "../types";
import { quotaMarkOf, requestQuotaSwitch, type QuotaMark } from "./quotaMarks";

/// Replace the contents of a `.pane-title` element. `pending` marks a badge
/// whose account was changed after the pane's PTY started: the running shell
/// still has the old environment until the terminal is restarted.
export function paintPaneTitle(
  el: HTMLElement,
  text: string,
  badge: AccountBadge | null,
  pending = false,
  paneId?: Uuid,
): void {
  el.replaceChildren(document.createTextNode(text));
  if (paneId && isAutoContinue(paneId)) el.appendChild(autoBadge(paneId));
  if (badge) el.appendChild(accountSpan(badge, pending));
  const mark = paneId ? quotaMarkOf(paneId) : undefined;
  if (paneId && mark) el.appendChild(quotaChip(paneId, mark));
}

function accountSpan(badge: AccountBadge, pending: boolean): HTMLElement {
  const span = document.createElement("span");
  span.className = "account-badge";
  if (pending) span.classList.add("account-badge--pending");
  span.title = pending
    ? `${badge.name} — ${t("account.pendingRestart")}`
    : `${t("account.badgeTitle")}: ${badge.name}`;
  const dot = document.createElement("span");
  dot.className = "account-badge__dot";
  if (badge.color) dot.style.background = badge.color;
  span.appendChild(dot);
  span.appendChild(document.createTextNode(badge.name));
  return span;
}

/// "!" chip next to the account: the account is near / at its usage limit and
/// another one has room. A click opens "Switch account…".
function quotaChip(paneId: Uuid, mark: QuotaMark): HTMLElement {
  const chip = document.createElement("button");
  chip.type = "button";
  chip.className = `quota-chip quota-chip--${mark.level}`;
  chip.textContent = mark.level === "limit" ? "!!" : "!";
  const text = t(mark.level === "limit" ? "quota.chipLimit" : "quota.chipNear").replace("{n}", String(Math.round(mark.used)));
  chip.title = text;
  chip.setAttribute("aria-label", text);
  chip.addEventListener("click", (ev) => {
    ev.stopPropagation();
    requestQuotaSwitch(paneId);
  });
  return chip;
}

/// "AUTO" badge for a pane with auto-continue on; its tooltip lists the last
/// questions it answered.
function autoBadge(paneId: Uuid): HTMLElement {
  const span = document.createElement("span");
  span.className = "auto-badge";
  span.textContent = t("autoContinue.badge");
  const log = autoLog(paneId);
  const lines = log.length
    ? log.map((e) => formatEntry(e, t("autoContinue.denied")))
    : [t("autoContinue.noAnswers")];
  span.title = [t("autoContinue.badgeTitle"), ...lines].join("\n");
  return span;
}
