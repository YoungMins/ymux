// The fan-out form: one prompt, N agent lanes, each lane its own worktree.
// Only collects and validates input; `WorkspaceManager.startFanout` acts on it.
// Built on `ui/Dialog.ts`'s shell so focus, Esc and click-outside behave like
// every other dialog.

import type { AccountProfile, DetectedAgent } from "../types";
import type { BranchList } from "../ipc/bridge";
import { open } from "../ui/Dialog";
import { t } from "../i18n/i18n";
import { ACCOUNT_NONE } from "../accounts/accountModel";
import { MAX_LANES, validateLanes, type Lane, type LaneError } from "./lanes";

export interface FanoutRequest {
  repo: string;
  base: string;
  prefix: string;
  prompt: string;
  lanes: Lane[];
  newWorkspace: boolean;
  /// Press Enter after pasting the prompt. Default ON: fan-out exists to start
  /// agents unattended.
  submit: boolean;
}

export interface FanoutInit {
  repo: string;
  branches: BranchList;
  /// Already limited to the agents a lane may run (claude / codex).
  agents: readonly DetectedAgent[];
  accounts: readonly AccountProfile[];
}

const ERR_KEY: Record<LaneError, string> = {
  lanes: "fanout.err.lanes",
  "prompt-empty": "fanout.err.promptEmpty",
  "prompt-too-long": "fanout.err.promptTooLong",
};

/// Local branches first, then remote ones; the current branch (or the first
/// local one) is the default.
export function baseChoices(b: BranchList): { options: string[]; initial: string } {
  const options = [...b.local, ...b.remote];
  const initial = b.current || b.local[0] || options[0] || "HEAD";
  if (!options.includes(initial)) options.unshift(initial);
  return { options, initial };
}

function field(label: string, control: HTMLElement): HTMLElement {
  const wrap = document.createElement("label");
  wrap.className = "fanout__field";
  const l = document.createElement("span");
  l.className = "fanout__label";
  l.textContent = label;
  wrap.append(l, control);
  return wrap;
}

function select(options: { value: string; label: string }[], value: string): HTMLSelectElement {
  const s = document.createElement("select");
  s.className = "dialog__input";
  for (const o of options) {
    const el = document.createElement("option");
    el.value = o.value;
    el.textContent = o.label;
    s.appendChild(el);
  }
  s.value = value;
  return s;
}

/// Resolves the validated request, or `null` on cancel.
export async function askFanout(init: FanoutInit): Promise<FanoutRequest | null> {
  let result: FanoutRequest | null = null;
  const answer = await open((card, done) => {
    card.classList.add("dialog--fanout");

    const title = document.createElement("div");
    title.className = "dialog__message";
    title.textContent = t("fanout.title");

    const repo = document.createElement("div");
    repo.className = "dialog__detail";
    repo.textContent = `${t("fanout.repo")}: ${init.repo}`;

    const { options, initial } = baseChoices(init.branches);
    const base = select(
      options.map((o) => ({ value: o, label: o })),
      initial,
    );

    const lanesBox = document.createElement("div");
    lanesBox.className = "fanout__lanes";
    const accountOptions = [
      { value: "", label: t("account.inherit") },
      { value: ACCOUNT_NONE, label: t("account.none") },
      ...init.accounts.map((a) => ({ value: a.id, label: a.name })),
    ];
    const rows: { agent: HTMLSelectElement; account: HTMLSelectElement }[] = [];
    const addBtn = document.createElement("button");
    addBtn.type = "button";
    addBtn.className = "dialog__btn";
    addBtn.textContent = t("fanout.addLane");

    const syncAdd = () => {
      addBtn.disabled = rows.length >= MAX_LANES;
    };
    const addLane = () => {
      if (rows.length >= MAX_LANES || init.agents.length === 0) return;
      const agent = select(
        init.agents.map((a) => ({ value: a.id, label: a.name })),
        init.agents[rows.length % init.agents.length].id,
      );
      agent.title = t("fanout.agent");
      const account = select(accountOptions, "");
      account.title = t("account.title");
      const rm = document.createElement("button");
      rm.type = "button";
      rm.className = "dialog__btn";
      rm.textContent = "✕";
      rm.title = t("fanout.removeLane");
      const row = document.createElement("div");
      row.className = "fanout__lane";
      row.append(agent, account, rm);
      const entry = { agent, account };
      rm.addEventListener("click", () => {
        rows.splice(rows.indexOf(entry), 1);
        row.remove();
        syncAdd();
      });
      rows.push(entry);
      lanesBox.appendChild(row);
      syncAdd();
    };
    addBtn.addEventListener("click", addLane);
    addLane();

    const prefix = document.createElement("input");
    prefix.type = "text";
    prefix.className = "dialog__input";
    prefix.value = "fanout";

    const prompt = document.createElement("textarea");
    prompt.className = "dialog__input fanout__prompt";
    prompt.rows = 6;

    const wsWrap = document.createElement("label");
    wsWrap.className = "dialog__check";
    const ws = document.createElement("input");
    ws.type = "checkbox";
    ws.checked = true;
    wsWrap.append(ws, document.createTextNode(t("fanout.newWorkspace")));

    const submitWrap = document.createElement("label");
    submitWrap.className = "dialog__check";
    const submit = document.createElement("input");
    submit.type = "checkbox";
    submit.checked = true;
    submitWrap.append(submit, document.createTextNode(t("fanout.submit")));

    const error = document.createElement("div");
    error.className = "fanout__error";
    error.setAttribute("role", "alert");
    if (init.agents.length === 0) error.textContent = t("fanout.err.noAgents");

    const buttons = document.createElement("div");
    buttons.className = "dialog__buttons";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "dialog__btn";
    cancel.textContent = t("dialog.cancel");
    cancel.addEventListener("click", () => done(null));
    const start = document.createElement("button");
    start.type = "button";
    start.className = "dialog__btn dialog__btn--primary";
    start.textContent = t("fanout.start");
    start.addEventListener("click", () => {
      if (init.agents.length === 0) {
        error.textContent = t("fanout.err.noAgents");
        return;
      }
      const lanes: Lane[] = rows.map((r) => ({ agent: r.agent.value, account: r.account.value }));
      const bad = validateLanes(lanes, prompt.value);
      if (bad) {
        error.textContent = t(ERR_KEY[bad]);
        return;
      }
      result = {
        repo: init.repo,
        base: base.value,
        prefix: prefix.value,
        prompt: prompt.value,
        lanes,
        newWorkspace: ws.checked,
        submit: submit.checked,
      };
      done("ok");
    });
    buttons.append(cancel, start);

    card.append(
      title,
      repo,
      field(t("fanout.base"), base),
      field(t("fanout.lanes"), lanesBox),
      addBtn,
      field(t("fanout.prefix"), prefix),
      field(t("fanout.prompt"), prompt),
      wsWrap,
      submitWrap,
      error,
      buttons,
    );
    return prompt;
  });
  return answer === "ok" ? result : null;
}
