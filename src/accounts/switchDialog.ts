// "Switch account…": the other accounts with their quota headroom (best
// first), an honest note that the conversation does not carry over, and an
// optional editable hand-off note. Resolves with the pick, or null on cancel.

import { open } from "../ui/Dialog";
import { t } from "../i18n/i18n";
import type { SwitchOption } from "./quotaModel";

export interface SwitchDialogRequest {
  /// Pane name for the title, and the agent ("Claude") for the note.
  pane: string;
  agent: string;
  options: readonly SwitchOption[];
  /// Display name and dot colour ("" = none) of an account id.
  nameOf: (account: string) => string;
  colorOf: (account: string) => string;
  /// The command line that will be typed into the new shell.
  command: string;
  /// Prefilled hand-off note.
  note: string;
}

export interface SwitchChoice {
  account: string;
  /// The note to deliver, or null when the box was left unchecked.
  handoff: string | null;
}

const fill = (text: string, vars: Record<string, string>): string =>
  text.replace(/\{(\w+)\}/g, (m, k: string) => vars[k] ?? m);

export function askSwitchAccount(req: SwitchDialogRequest): Promise<SwitchChoice | null> {
  let picked: SwitchChoice | null = null;
  return open((card, done) => {
    card.classList.add("quota-dialog");
    const title = document.createElement("div");
    title.className = "dialog__message";
    title.textContent = fill(t("quota.dialogTitle"), { pane: req.pane });

    const note = document.createElement("div");
    note.className = "dialog__detail";
    note.textContent = fill(t("quota.dialogNote"), { agent: req.agent });

    const cmd = document.createElement("div");
    cmd.className = "dialog__detail";
    cmd.textContent = fill(t("quota.command"), { command: req.command });

    const list = document.createElement("div");
    list.className = "quota-dialog__list";
    list.setAttribute("role", "radiogroup");
    let chosen = req.options.find((o) => o.level === "ok" && o.headroom !== null)?.account ?? req.options[0]?.account ?? "";
    const rows = new Map<string, HTMLInputElement>();
    for (const o of req.options) {
      const label = document.createElement("label");
      label.className = "quota-dialog__option";
      const radio = document.createElement("input");
      radio.type = "radio";
      radio.name = "quota-account";
      radio.checked = o.account === chosen;
      radio.addEventListener("change", () => {
        chosen = o.account;
      });
      rows.set(o.account, radio);
      const dot = document.createElement("span");
      dot.className = "account-badge__dot";
      const color = req.colorOf(o.account);
      if (color) dot.style.background = color;
      const name = document.createElement("span");
      name.className = "quota-dialog__name";
      name.textContent = req.nameOf(o.account);
      const room = document.createElement("span");
      room.className = `quota-dialog__room quota-dialog__room--${o.level}`;
      room.textContent = o.headroom === null ? t("quota.headroomUnknown") : fill(t("quota.headroom"), { n: String(Math.round(o.headroom)) });
      label.append(radio, dot, name, room);
      list.appendChild(label);
    }

    const check = document.createElement("label");
    check.className = "dialog__check";
    const box = document.createElement("input");
    box.type = "checkbox";
    check.append(box, document.createTextNode(t("quota.handoffCheck")));

    const noteLabel = document.createElement("div");
    noteLabel.className = "quota-dialog__hint";
    noteLabel.textContent = t("quota.handoffLabel");
    const text = document.createElement("textarea");
    text.className = "dialog__input quota-dialog__note";
    text.rows = 5;
    text.value = req.note;
    const sync = (): void => {
      text.disabled = !box.checked;
      noteLabel.hidden = text.hidden = !box.checked;
    };
    box.addEventListener("change", sync);
    sync();

    const buttons = document.createElement("div");
    buttons.className = "dialog__buttons";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "dialog__btn";
    cancel.textContent = t("quota.cancel");
    cancel.addEventListener("click", () => done(null));
    const ok = document.createElement("button");
    ok.type = "button";
    ok.className = "dialog__btn dialog__btn--primary";
    ok.textContent = t("quota.ok");
    ok.disabled = req.options.length === 0;
    ok.addEventListener("click", () => {
      if (!chosen) return;
      picked = { account: chosen, handoff: box.checked && text.value.trim() !== "" ? text.value : null };
      done("ok");
    });
    buttons.append(cancel, ok);

    card.append(title, note, cmd, list, check, noteLabel, text, buttons);
    return ok;
  }).then((answer) => (answer === "ok" ? picked : null));
}
