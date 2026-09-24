/// In-app replacements for `window.prompt()` and `window.confirm()`.
///
/// Those two are unusable in this app. On macOS the webview is WKWebView,
/// which only shows a JavaScript dialog if the host implements the matching
/// `WKUIDelegate` method — and wry implements none of them. The call doesn't
/// throw or warn: `prompt()` simply returns `null` and `confirm()` returns
/// `false`, so every rename silently did nothing and every confirmation
/// silently answered "no". WebView2 on Windows provides them natively, which
/// is why this only ever showed up on the Mac build.
///
/// These render ordinary DOM, so they behave identically on both platforms.
/// They are async where the native calls were blocking; call sites `await`.
import { pushPopup, popPopup } from "../browser/popupBlur";
import { t } from "../i18n/i18n";

type Resolver = (value: string | null) => void;

/// The single open dialog, if any. Two dialogs at once would fight over focus
/// and over the popup counter, and nothing in the app legitimately needs it.
let active: { el: HTMLElement; resolve: Resolver } | null = null;

/// Close whatever is open, hand `value` to its waiter, and put focus back
/// where it was. Restoring focus matters more than it looks: without it the
/// terminal stays blurred after a rename and the next keystroke goes nowhere.
function close(value: string | null, restore: Element | null): void {
  if (!active) return;
  const { el, resolve } = active;
  active = null;
  el.remove();
  popPopup();
  if (restore instanceof HTMLElement) restore.focus();
  resolve(value);
}

/// Shared shell: backdrop + centred card, Esc to cancel, click-outside to
/// cancel. `build` fills the card and returns the element to focus on open.
function open(
  build: (card: HTMLElement, done: (v: string | null) => void) => HTMLElement,
): Promise<string | null> {
  // A second dialog cancels the first rather than stacking on top of it.
  if (active) close(null, null);

  const restore = document.activeElement;
  return new Promise<string | null>((resolve) => {
    const backdrop = document.createElement("div");
    backdrop.className = "dialog-backdrop";

    const card = document.createElement("div");
    card.className = "dialog";
    card.setAttribute("role", "dialog");
    card.setAttribute("aria-modal", "true");
    backdrop.appendChild(card);

    const done = (v: string | null) => close(v, restore);
    const focusTarget = build(card, done);

    backdrop.addEventListener("mousedown", (ev) => {
      if (ev.target === backdrop) done(null);
    });
    // Capture phase, and stop propagation on everything: ymux's global
    // shortcut handler lives on `window`, and a dialog that let Ctrl+Shift+W
    // through would close the pane behind it while the user was typing.
    card.addEventListener(
      "keydown",
      (ev) => {
        ev.stopPropagation();
        if (ev.key === "Escape") {
          ev.preventDefault();
          done(null);
        }
      },
      true,
    );

    document.body.appendChild(backdrop);
    pushPopup();
    active = { el: backdrop, resolve };
    focusTarget.focus();
  });
}

/// Ask for a line of text. Resolves to the entered string, or `null` if the
/// user cancelled. Replaces `window.prompt`.
///
/// `selectEnd` pre-selects only `[0, selectEnd)` — a file rename selects the
/// name without its extension, as every file manager does.
export async function askText(
  message: string,
  defaultValue = "",
  opts: { selectEnd?: number } = {},
): Promise<string | null> {
  return open((card, done) => {
    const label = document.createElement("div");
    label.className = "dialog__message";
    label.textContent = message;

    const input = document.createElement("input");
    input.type = "text";
    input.className = "dialog__input";
    input.value = defaultValue;

    const row = document.createElement("div");
    row.className = "dialog__buttons";

    const cancel = document.createElement("button");
    cancel.className = "dialog__btn";
    cancel.textContent = t("dialog.cancel");
    cancel.addEventListener("click", () => done(null));

    const ok = document.createElement("button");
    ok.className = "dialog__btn dialog__btn--primary";
    ok.textContent = t("dialog.ok");
    ok.addEventListener("click", () => done(input.value));

    input.addEventListener("keydown", (ev) => {
      // The Enter that commits a Hangul/Japanese composition is not a submit:
      // taking it as one reads `input.value` before the last syllable lands.
      if (ev.isComposing || ev.keyCode === 229) return;
      if (ev.key === "Enter") {
        ev.preventDefault();
        done(input.value);
      }
    });

    row.append(cancel, ok);
    card.append(label, input, row);
    // Select rather than just focus, so a rename can be typed straight over
    // the existing name — the same thing `window.prompt` did.
    queueMicrotask(() => {
      if (opts.selectEnd !== undefined) input.setSelectionRange(0, opts.selectEnd);
      else input.select();
    });
    return input;
  });
}

export interface Choice {
  id: string;
  label: string;
  primary?: boolean;
}

/// Ask the user to pick one of `choices`, with an optional checkbox (e.g.
/// "do this for the rest"). Resolves `null` on Esc or click-outside. The
/// primary choice gets focus, so Enter picks it.
export async function askChoice(
  message: string,
  detail: string | null,
  choices: Choice[],
  checkboxLabel?: string,
): Promise<{ id: string; checked: boolean } | null> {
  let box: HTMLInputElement | null = null;
  const answer = await open((card, done) => {
    const label = document.createElement("div");
    label.className = "dialog__message";
    label.textContent = message;
    card.appendChild(label);

    if (detail) {
      const d = document.createElement("div");
      d.className = "dialog__detail";
      d.textContent = detail;
      card.appendChild(d);
    }

    if (checkboxLabel) {
      const wrap = document.createElement("label");
      wrap.className = "dialog__check";
      box = document.createElement("input");
      box.type = "checkbox";
      wrap.append(box, document.createTextNode(checkboxLabel));
      card.appendChild(wrap);
    }

    const row = document.createElement("div");
    row.className = "dialog__buttons";
    let focus: HTMLElement | null = null;
    for (const c of choices) {
      const b = document.createElement("button");
      b.className = c.primary ? "dialog__btn dialog__btn--primary" : "dialog__btn";
      b.textContent = c.label;
      b.addEventListener("click", () => done(c.id));
      row.appendChild(b);
      if (c.primary || !focus) focus = b;
    }
    card.appendChild(row);
    return focus ?? card;
  });
  if (answer === null) return null;
  return { id: answer, checked: (box as HTMLInputElement | null)?.checked ?? false };
}

/// Ask a yes/no question. Resolves `true` only on explicit confirmation.
/// Replaces `window.confirm`.
/// `okLabel` names the action ("Move to Trash") instead of a bare "OK".
export async function askConfirm(message: string, okLabel?: string): Promise<boolean> {
  const answer = await open((card, done) => {
    const label = document.createElement("div");
    label.className = "dialog__message";
    label.textContent = message;

    const row = document.createElement("div");
    row.className = "dialog__buttons";

    const cancel = document.createElement("button");
    cancel.className = "dialog__btn";
    cancel.textContent = t("dialog.cancel");
    cancel.addEventListener("click", () => done(null));

    const ok = document.createElement("button");
    ok.className = "dialog__btn dialog__btn--primary";
    ok.textContent = okLabel ?? t("dialog.ok");
    ok.addEventListener("click", () => done("yes"));

    row.append(cancel, ok);
    card.append(label, row);
    return ok;
  });
  return answer === "yes";
}
