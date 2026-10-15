// Pane title row: the label text plus, when an account applies, a small
// coloured dot and the account name. Shared by `TerminalPane`'s own title and
// `PaneGroup`'s shared one so both read the same.

import { t } from "../i18n/i18n";
import type { AccountBadge } from "./accountModel";

/// Replace the contents of a `.pane-title` element. `pending` marks a badge
/// whose account was changed after the pane's PTY started: the running shell
/// still has the old environment until the terminal is restarted.
export function paintPaneTitle(
  el: HTMLElement,
  text: string,
  badge: AccountBadge | null,
  pending = false,
): void {
  el.replaceChildren(document.createTextNode(text));
  if (!badge) return;
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
  el.appendChild(span);
}
