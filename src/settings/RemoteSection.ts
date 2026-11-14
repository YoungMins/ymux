// Settings -> "Remote server": the address and token of the user's own
// yMux-Server, the connection switch, notifications and the paired phones.
// Pure decisions live in `remote/remoteModel.ts`; this file is DOM + IPC only.
//
// Rules this file keeps:
//  - Every dynamic string goes in through textContent / value, never innerHTML.
//  - The setup token is write-only: nothing here reads it back, the backend
//    only reports `has_token`.
//  - The QR code is an inline <svg> (`remote/qr.ts`), no data: URL.

import { t } from "../i18n/i18n";
import { fill } from "../git/worktreeFlow";
import { describeError, onRemoteStatus, remoteApi } from "../ipc/bridge";
import { qrSvg } from "../remote/qr";
import { REMOTE_CAPS, countdown, isRemoteCap, statusLine, type StatusTone } from "../remote/remoteModel";
import type { RemoteDevice, RemoteStatus } from "../types";
import { askConfirm } from "../ui/Dialog";

const STATUS_POLL_MS = 5000;

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls = "",
  text?: string,
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function note(text: string, tone: StatusTone = ""): HTMLElement {
  return el("div", `act-note${tone ? ` act-note--${tone}` : ""}`, text);
}

function setNote(e: HTMLElement, text: string, tone: StatusTone = ""): void {
  e.className = `act-note${tone ? ` act-note--${tone}` : ""}`;
  e.textContent = text;
}

function field(label: string, control: HTMLElement, ...hints: HTMLElement[]): HTMLElement {
  const row = el("div", "act-row");
  row.appendChild(el("div", "act-row__label", label));
  const box = el("div", "act-row__control");
  box.appendChild(control);
  for (const h of hints) box.appendChild(h);
  row.appendChild(box);
  return row;
}

function button(label: string, primary = false): HTMLButtonElement {
  const b = el("button", `hotkey-modal__btn${primary ? " hotkey-modal__btn--primary" : ""}`, label);
  b.type = "button";
  return b;
}

function textInput(placeholder = ""): HTMLInputElement {
  const i = el("input", "settings-hex-input remote-wide");
  i.type = "text";
  i.spellcheck = false;
  i.placeholder = placeholder;
  return i;
}

function checkbox(label: string): { wrap: HTMLElement; box: HTMLInputElement } {
  const wrap = el("label", "act-check");
  const box = el("input");
  box.type = "checkbox";
  wrap.append(box, el("span", "", label));
  return { wrap, box };
}

/// Renders into `host`; the returned function stops the polling and the clock.
export function renderRemoteSection(host: HTMLElement): () => void {
  let disposed = false;
  let status: RemoteStatus | null = null;
  let formFilled = false;

  host.appendChild(el("h3", "", t("settings.remote.heading")));
  host.appendChild(el("p", "settings-row__hint", t("settings.remote.intro")));

  // ── Connection ──────────────────────────────────────────────────────
  const url = textInput("https://ymux.example.com");
  const name = textInput();
  const token = el("input", "settings-hex-input remote-wide");
  token.type = "password";
  token.autocomplete = "off";
  token.spellcheck = false;
  token.placeholder = t("settings.remote.tokenPlaceholder");
  const tokenNote = note("");
  const clearToken = button(t("settings.remote.tokenClear"));
  clearToken.hidden = true;
  const enable = checkbox(t("settings.remote.connect"));
  const save = button(t("settings.remote.save"), true);
  const saveMsg = note("");
  const statusEl = note("");
  const noticeEl = note("");
  noticeEl.hidden = true;

  const conn = el("div", "act-block");
  conn.appendChild(field(t("settings.remote.serverUrl"), url));
  conn.appendChild(field(t("settings.remote.name"), name, note(t("settings.remote.nameHint"))));
  const tokenBox = el("div", "act-inline");
  tokenBox.append(token, clearToken);
  conn.appendChild(field(t("settings.remote.token"), tokenBox, tokenNote));
  conn.appendChild(field(t("settings.remote.connectLabel"), enable.wrap));
  const actions = el("div", "act-actions");
  actions.append(save, saveMsg);
  conn.appendChild(actions);
  conn.appendChild(statusEl);
  conn.appendChild(noticeEl);
  host.appendChild(conn);

  // ── Notifications ───────────────────────────────────────────────────
  const notifyBlock = el("div", "act-block");
  notifyBlock.appendChild(el("h4", "", t("settings.remote.notifyHeading")));
  const nWaiting = checkbox(t("settings.remote.notifyWaiting"));
  const nDone = checkbox(t("settings.remote.notifyDone"));
  const checks = el("div", "act-checks");
  checks.append(nWaiting.wrap, nDone.wrap);
  notifyBlock.appendChild(checks);
  notifyBlock.appendChild(note(t("settings.remote.notifyHint")));
  host.appendChild(notifyBlock);

  // ── Devices ─────────────────────────────────────────────────────────
  const devBlock = el("div", "act-block");
  devBlock.appendChild(el("h4", "", t("settings.remote.devicesHeading")));
  const pairBtn = button(t("settings.remote.pair"));
  const pairMsg = note("");
  const pairActions = el("div", "act-actions");
  pairActions.append(pairBtn, pairMsg);
  const pairCard = el("div", "remote-pair");
  pairCard.hidden = true;
  const deviceList = el("div", "remote-devices");
  devBlock.append(pairActions, pairCard, deviceList);
  host.appendChild(devBlock);

  // ── Painting ────────────────────────────────────────────────────────
  const fillForm = (s: RemoteStatus): void => {
    url.value = s.server_url;
    name.value = s.name;
    enable.box.checked = s.enabled;
    formFilled = true;
  };

  let shownDevices = "";
  let shownPairing = "";
  let clock: number | null = null;

  const stopClock = (): void => {
    if (clock !== null) window.clearInterval(clock);
    clock = null;
  };

  const paintPairing = (s: RemoteStatus): void => {
    const key = s.pairing ? `${s.pairing.url}|${s.pairing.expires_ms}` : "";
    if (key === shownPairing) return;
    shownPairing = key;
    stopClock();
    pairCard.textContent = "";
    pairCard.hidden = !s.pairing;
    if (!s.pairing) return;
    const p = s.pairing;
    const left = note("");
    const cancel = button(t("settings.remote.pairCancel"));
    cancel.addEventListener("click", () => {
      void remoteApi
        .pairCancel()
        .catch(() => {})
        .then(refresh);
    });
    // The QR code only, never the link as text: anyone who sees the link can
    // ask to pair.
    pairCard.append(qrSvg(p.url), note(t("settings.remote.pairScan")), left, cancel);
    const tick = (): void => {
      if (Date.now() >= p.expires_ms) {
        stopClock();
        setNote(left, t("settings.remote.pairExpired"), "warn");
        void refresh();
        return;
      }
      setNote(left, fill(t("settings.remote.pairExpires"), { time: countdown(p.expires_ms, Date.now()) }));
    };
    tick();
    clock = window.setInterval(tick, 1000);
  };

  const deviceRow = (d: RemoteDevice): HTMLElement => {
    const row = el("div", "remote-device");
    const dot = el("span", `remote-dot${d.online ? " remote-dot--on" : ""}`);
    dot.title = t(d.online ? "settings.remote.deviceOnline" : "settings.remote.deviceOffline");
    const head = el("div", "remote-device__head");
    head.append(
      dot,
      el("span", "remote-device__name", d.name),
      el("span", "act-note", fill(t("settings.remote.added"), { date: new Date(d.added_ms).toLocaleDateString() })),
    );
    const sel = el("select", "settings-hex-input remote-device__cap");
    for (const c of REMOTE_CAPS) {
      const o = el("option", "", t(`settings.remote.cap.${c}`));
      o.value = c;
      sel.appendChild(o);
    }
    sel.value = d.cap;
    const desc = note(t(`settings.remote.capDesc.${d.cap}`));
    sel.addEventListener("change", () => {
      if (!isRemoteCap(sel.value)) return;
      const cap = sel.value;
      setNote(desc, t(`settings.remote.capDesc.${cap}`));
      void remoteApi
        .deviceSetCap(d.id, cap)
        .then(paint)
        .catch((e) => {
          setNote(desc, describeError(e), "error");
        });
    });
    const remove = button(t("settings.remote.remove"));
    remove.addEventListener("click", () => {
      void (async () => {
        const ok = await askConfirm(
          fill(t("settings.remote.removeConfirm"), { name: d.name }),
          t("settings.remote.remove"),
        );
        if (!ok) return;
        try {
          paint(await remoteApi.deviceRemove(d.id));
        } catch (e) {
          setNote(desc, describeError(e), "error");
        }
      })();
    });
    const controls = el("div", "act-inline");
    controls.append(sel, remove);
    row.append(head, controls, desc);
    return row;
  };

  const paintDevices = (s: RemoteStatus): void => {
    const key = JSON.stringify(s.devices);
    if (key === shownDevices) return;
    shownDevices = key;
    deviceList.textContent = "";
    if (s.devices.length === 0) {
      deviceList.appendChild(note(t("settings.remote.devicesEmpty")));
      return;
    }
    for (const d of s.devices) deviceList.appendChild(deviceRow(d));
  };

  function paint(s: RemoteStatus): void {
    if (disposed) return;
    status = s;
    if (!formFilled) fillForm(s);
    nWaiting.box.checked = s.notify_waiting;
    nDone.box.checked = s.notify_done;
    tokenNote.textContent = t(s.has_token ? "settings.remote.tokenSet" : "settings.remote.tokenNone");
    clearToken.hidden = !s.has_token;
    const line = statusLine(s);
    const text = t(`settings.remote.${line.key}`);
    setNote(statusEl, line.key === "err.other" ? fill(text, { detail: line.detail || "?" }) : text, line.tone);
    noticeEl.hidden = s.notice !== "identity_reset";
    if (!noticeEl.hidden) setNote(noticeEl, t("settings.remote.notice.identity_reset"), "warn");
    pairBtn.disabled = s.state !== "online" || s.pairing !== null;
    if (s.state !== "online" && !s.pairing) setNote(pairMsg, t("settings.remote.pairNeedOnline"));
    else if (pairMsg.textContent === t("settings.remote.pairNeedOnline")) setNote(pairMsg, "");
    paintPairing(s);
    paintDevices(s);
  }

  async function refresh(): Promise<void> {
    try {
      const s = await remoteApi.status();
      paint(s);
    } catch (e) {
      if (!disposed) setNote(statusEl, fill(t("settings.remote.loadFailed"), { error: describeError(e) }), "error");
    }
  }

  // ── Actions ─────────────────────────────────────────────────────────
  const configure = () =>
    remoteApi.configure({
      serverUrl: url.value.trim(),
      name: name.value.trim(),
      enabled: enable.box.checked,
      notifyWaiting: nWaiting.box.checked,
      notifyDone: nDone.box.checked,
    });

  save.addEventListener("click", () => {
    void (async () => {
      save.disabled = true;
      setNote(saveMsg, "");
      try {
        // The address first: the backend forgets the stored token when the
        // address changes, so a token typed with a new address must go in after.
        let s = await configure();
        let tokenSaved = false;
        const typed = token.value.trim();
        if (typed !== "") {
          s = await remoteApi.setToken(typed);
          token.value = "";
          tokenSaved = true;
        }
        fillForm(s);
        paint(s);
        setNote(
          saveMsg,
          t(tokenSaved ? "settings.remote.tokenSaved" : "settings.remote.saved"),
          "ok",
        );
      } catch (e) {
        setNote(saveMsg, describeError(e), "error");
      } finally {
        save.disabled = false;
      }
    })();
  });

  clearToken.addEventListener("click", () => {
    void remoteApi
      .setToken("")
      .then((s) => {
        paint(s);
        setNote(saveMsg, t("settings.remote.tokenCleared"), "ok");
      })
      .catch((e) => setNote(saveMsg, describeError(e), "error"));
  });

  // A notification switch applies at once, with the saved address and name
  // (an address typed but not yet saved is not sent along).
  const onNotify = (): void => {
    const s = status;
    if (!s) return;
    void remoteApi
      .configure({
        serverUrl: s.server_url,
        name: s.name,
        enabled: s.enabled,
        notifyWaiting: nWaiting.box.checked,
        notifyDone: nDone.box.checked,
      })
      .then(paint)
      .catch((e) => {
        setNote(saveMsg, describeError(e), "error");
        void refresh();
      });
  };
  nWaiting.box.addEventListener("change", onNotify);
  nDone.box.addEventListener("change", onNotify);

  pairBtn.addEventListener("click", () => {
    void (async () => {
      pairBtn.disabled = true;
      setNote(pairMsg, "");
      try {
        await remoteApi.pairStart();
        await refresh();
      } catch (e) {
        setNote(pairMsg, fill(t("settings.remote.pairFailed"), { error: describeError(e) }), "error");
        pairBtn.disabled = false;
      }
    })();
  });

  // ── Live updates ────────────────────────────────────────────────────
  const timer = window.setInterval(() => void refresh(), STATUS_POLL_MS);
  let unlisten: (() => void) | null = null;
  onRemoteStatus(() => void refresh())
    .then((off) => {
      if (disposed) off();
      else unlisten = off;
    })
    .catch((e) => console.warn("remote://status listen failed:", e));
  void refresh();

  return () => {
    disposed = true;
    window.clearInterval(timer);
    stopClock();
    unlisten?.();
  };
}
