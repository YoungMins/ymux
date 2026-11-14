// The modal shown when a phone asks to pair. Built on the app's Dialog shell,
// so it appears over whatever is open, Settings closed or not. The device's
// name comes from the phone: it only ever goes in through textContent.
//
// The six-digit check number is NOT shown here. The phone shows it (the
// desktop sent it there, sealed, and nowhere else), and the user TYPES it: the
// desktop pairs only the device whose user read that number off its screen, so
// someone who merely saw the QR code, or the relay, cannot get approved. The
// backend compares it, counts the wrong ones (three and the request is
// refused) and says how many tries remain; this dialog stays open until the
// request is approved, rejected or refused.

import { t } from "../i18n/i18n";
import { fill } from "../git/worktreeFlow";
import { open } from "../ui/Dialog";
import type { RemoteCap, RemotePairDecision } from "../types";
import { REMOTE_CAPS, isRemoteCap } from "./remoteModel";

const NAME_SHOWN = 80;
/// What the backend's name cleaning leaves when nothing is left of a name.
const UNNAMED_DEVICE = "Unnamed device";
/// Digits in the check number.
export const CHECK_DIGITS = 6;

/// How the dialog ended:
///  - `approved`: the right number, the device is paired;
///  - `refused`: too many wrong numbers, nothing was paired;
///  - `dismissed`: Reject, Esc, a click outside, another dialog or a conflict
///    (never an approval: the caller rejects the request);
///  - `error`: the backend could not be asked or said no for another reason.
export type PairEnd = "approved" | "refused" | "dismissed" | "error";

/// Closes the dialog that is open now (a conflict ended the pairing window).
let dismiss: ((v: string | null) => void) | null = null;

export function dismissPairRequest(): void {
  dismiss?.(null);
}

/// Only digits, at most [`CHECK_DIGITS`] of them.
export function digitsOnly(raw: string): string {
  return raw.replace(/\D/g, "").slice(0, CHECK_DIGITS);
}

/// Shows the request and resolves when it is over. `decide` asks the backend
/// about an approval attempt with the capability and the typed number.
export async function askPairRequest(
  deviceName: string,
  decide: (cap: RemoteCap, check: string) => Promise<RemotePairDecision>,
): Promise<PairEnd> {
  // The backend cleans the name (letters, marks, spaces and . - _ ' only, 40
  // chars, "Unnamed device" when nothing is left); this is a second bound.
  const raw = deviceName.length > NAME_SHOWN ? `${deviceName.slice(0, NAME_SHOWN)}…` : deviceName;
  const shown = raw === UNNAMED_DEVICE ? t("settings.remote.pairReq.unnamed") : raw;
  let mine: ((v: string | null) => void) | null = null;
  const answer = await open((card, done) => {
    mine = done;
    dismiss = done;
    const title = document.createElement("div");
    title.className = "dialog__message";
    title.textContent = t("settings.remote.pairReq.title");

    // The name is the phone's own claim, so it sits in a labelled element of
    // its own and never inside the sentence that asks for the number.
    const nameBox = document.createElement("div");
    nameBox.className = "remote-pairreq__device";
    const nameLabel = document.createElement("span");
    nameLabel.className = "act-note";
    nameLabel.textContent = t("settings.remote.pairReq.deviceLabel");
    const nameValue = document.createElement("div");
    nameValue.className = "remote-pairreq__devname";
    nameValue.textContent = shown;
    nameBox.append(nameLabel, nameValue);

    // Fixed text: no part of it comes from the phone, the relay or a push.
    const body = document.createElement("div");
    body.className = "dialog__detail";
    body.textContent = t("settings.remote.pairReq.body");

    const form = document.createElement("form");
    form.className = "remote-pairreq__form";
    form.noValidate = true;

    const typeLabel = document.createElement("label");
    typeLabel.className = "remote-pairreq__type";
    const typeText = document.createElement("span");
    typeText.textContent = t("settings.remote.pairReq.type");
    const input = document.createElement("input");
    input.type = "text";
    input.className = "settings-hex-input remote-pairreq__input";
    input.inputMode = "numeric";
    input.autocomplete = "off";
    input.maxLength = CHECK_DIGITS;
    input.setAttribute("pattern", "[0-9]*");
    input.setAttribute("aria-label", t("settings.remote.pairReq.type"));
    input.spellcheck = false;
    typeLabel.append(typeText, input);

    const problem = document.createElement("div");
    problem.className = "act-note act-note--warn remote-pairreq__problem";
    problem.setAttribute("role", "alert");
    problem.hidden = true;

    const capRow = document.createElement("label");
    capRow.className = "remote-pairreq__cap";
    const capLabel = document.createElement("span");
    capLabel.textContent = t("settings.remote.pairReq.cap");
    const sel = document.createElement("select");
    sel.className = "settings-hex-input";
    for (const c of REMOTE_CAPS) {
      const o = document.createElement("option");
      o.value = c;
      o.textContent = t(`settings.remote.cap.${c}`);
      sel.appendChild(o);
    }
    sel.value = "respond";
    const capDesc = document.createElement("div");
    capDesc.className = "act-note";
    const paintDesc = (): void => {
      capDesc.textContent = t(`settings.remote.capDesc.${sel.value}`);
    };
    sel.addEventListener("change", paintDesc);
    paintDesc();
    capRow.append(capLabel, sel);

    const warn = document.createElement("div");
    warn.className = "act-note act-note--warn";
    warn.textContent = t("settings.remote.pairReq.warn");

    const row = document.createElement("div");
    row.className = "dialog__buttons";
    const reject = document.createElement("button");
    reject.type = "button";
    reject.className = "dialog__btn";
    reject.textContent = t("settings.remote.pairReq.reject");
    reject.addEventListener("click", () => done(null));
    const approve = document.createElement("button");
    approve.type = "submit";
    approve.className = "dialog__btn dialog__btn--primary";
    approve.textContent = t("settings.remote.pairReq.approve");
    approve.disabled = true;
    row.append(reject, approve);

    let busy = false;
    const paint = (): void => {
      approve.disabled = busy || input.value.length !== CHECK_DIGITS;
      input.disabled = busy;
      sel.disabled = busy;
    };
    input.addEventListener("input", () => {
      const clean = digitsOnly(input.value);
      if (clean !== input.value) input.value = clean;
      paint();
    });

    form.addEventListener("submit", (ev) => {
      ev.preventDefault();
      const cap = sel.value;
      if (busy || input.value.length !== CHECK_DIGITS || !isRemoteCap(cap)) return;
      busy = true;
      paint();
      decide(cap, input.value)
        .then((r) => {
          if (r.result === "approved") {
            done("approved");
          } else if (r.result === "wrong") {
            busy = false;
            input.value = "";
            problem.hidden = false;
            problem.textContent = fill(t("settings.remote.pairReq.wrong"), {
              n: String(r.tries_left),
            });
            paint();
            input.focus();
          } else if (r.result === "refused") {
            done("refused");
          } else {
            done("error");
          }
        })
        .catch(() => done("error"));
    });
    form.append(typeLabel, problem, capRow, capDesc, warn, row);

    card.append(title, nameBox, body, form);
    // The number goes in first: type it, press Enter. (Reject and Esc stay
    // one key away; nothing approves by reflex because Approve is disabled
    // until all six digits are in.)
    return input;
  });
  if (dismiss === mine) dismiss = null;
  switch (answer) {
    case "approved":
    case "refused":
    case "error":
      return answer;
    default:
      return "dismissed";
  }
}
