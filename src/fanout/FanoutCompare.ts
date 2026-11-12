// Fan-out compare overlay: one row per lane of a fan-out run (status, diff
// size, commits ahead), with Review / Focus / Keep this one / Merge by hand.
// Runtime-only; rebuilt on every open. "Keep this one" asks the backend
// (git_keep_lane) to merge and clean up after a confirm dialog; "Merge by
// hand" only types the merge command and offers the existing removal flow.
//
// Everything from git (branch names, paths, commit subjects) reaches the DOM
// through textContent only.

import type { Uuid } from "../types";
import type { WorkspaceManager } from "../workspace/WorkspaceManager";
import { api, gitApi, type CommitInfo, type KeepReport, type WorktreeEntry } from "../ipc/bridge";
import { t, onLangChange } from "../i18n/i18n";
import { pushPopup, popPopup } from "../browser/popupBlur";
import { askChoice, askConfirm, open as openDialog } from "../ui/Dialog";
import { trapTab } from "../ui/trapTab";
import { openDiffReview } from "../diff/DiffReview";
import { commitLines, fill, gitErrorText, removeWorktreeFlow, showGitError } from "../git/worktreeFlow";
import {
  cleanupTargets,
  commitsAhead,
  diffStat,
  lanesOfRun,
  mergeCommand,
  runIds,
  type DiffStat,
  type LaneRef,
} from "./compareModel";
import {
  DEFAULT_CHOICE,
  cleanupRequest,
  combineReports,
  hasCleanup,
  keepPlan,
  keepRequest,
  mergeRequest,
  openSkips,
  pathsToVacate,
  refusal,
  reportLines,
  type KeepChoice,
} from "./keepModel";

/// Commits read per lane to list the ones above its base (the count itself
/// comes from the backend, exactly).
const LOG_LIMIT = 100;
const TOAST_MS = 20000;
const COMMIT_ASK =
  "Please commit all of your changes in this worktree with a clear commit message. Do not push.";

interface Row {
  ref: LaneRef;
  entry: WorktreeEntry;
  stat: DiffStat | null;
  ahead: { n: number; more: boolean } | null;
  commits: CommitInfo[];
  dirty: number;
  paneId: Uuid | null;
  agent: string;
  error: string | null;
}

let host: WorkspaceManager | null = null;
let mountEl: HTMLElement | null = null;
let current: Overlay | null = null;
let toast: HTMLElement | null = null;
let toastTimer: number | null = null;

export function mountFanoutCompare(container: HTMLElement, manager: WorkspaceManager): void {
  mountEl = container;
  host = manager;
}

/// Is the compare overlay on screen? Global shortcuts that act on the panes
/// behind it check this (with `isDiffReviewOpen`) and stand down.
export function isFanoutCompareOpen(): boolean {
  return current !== null;
}

/// Open the compare view for the repository of the focused pane.
export async function openFanoutCompare(): Promise<void> {
  if (!mountEl || !host) return;
  const notify = (body: string) => void api.notify(t("fanoutCmp.title"), body).catch(() => {});
  const cwd = await host.focusedCwd();
  if (!cwd) return notify(t("worktree.noCwd"));
  let repo: string;
  try {
    repo = await gitApi.repoRoot(cwd);
  } catch {
    return notify(t("worktree.notRepo"));
  }
  dismissFanoutNotice();
  current?.hide(false);
  current = new Overlay(repo, mountEl, host);
  current.show();
}

/// A small non-modal toast after a fan-out started, with a Compare button.
export function showFanoutNotice(lanes: number): void {
  if (!mountEl) return;
  dismissFanoutNotice();
  const box = document.createElement("div");
  box.className = "fcmp-toast";
  box.setAttribute("role", "status");
  const msg = document.createElement("span");
  msg.textContent = fill(t("fanoutCmp.noticeDone"), { n: lanes });
  const go = document.createElement("button");
  go.type = "button";
  go.className = "diff-btn diff-btn--primary";
  go.textContent = t("fanoutCmp.compareBtn");
  go.addEventListener("click", () => void openFanoutCompare());
  const x = document.createElement("button");
  x.type = "button";
  x.className = "diff-btn";
  x.textContent = "×";
  x.addEventListener("click", dismissFanoutNotice);
  box.append(msg, go, x);
  mountEl.appendChild(box);
  toast = box;
  toastTimer = window.setTimeout(dismissFanoutNotice, TOAST_MS);
}

function dismissFanoutNotice(): void {
  if (toastTimer !== null) window.clearTimeout(toastTimer);
  toastTimer = null;
  toast?.remove();
  toast = null;
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

class Overlay {
  private readonly el: HTMLElement;
  private readonly titleEl: HTMLElement;
  private readonly runLabel: HTMLElement;
  private readonly runSel: HTMLSelectElement;
  private readonly refreshBtn: HTMLButtonElement;
  private readonly closeBtn: HTMLButtonElement;
  private readonly listEl: HTMLElement;
  private readonly noticeEl: HTMLElement;
  private readonly unsubLang: () => void;
  private prevFocus: HTMLElement | null = null;
  private visible = false;
  private tok = 0;
  private main = "";
  private mainBranch = "";
  private refs: LaneRef[] = [];
  private runId = "";
  private rows: Row[] = [];
  private empty = false;
  private loading = false;
  private loadError: string | null = null;

  constructor(
    private readonly repo: string,
    container: HTMLElement,
    private readonly mgr: WorkspaceManager,
  ) {
    this.el = el("div", "diff-review fcmp");
    this.el.tabIndex = -1;
    this.el.setAttribute("role", "dialog");
    this.el.setAttribute("aria-modal", "true");

    const head = el("div", "diff-head");
    this.titleEl = el("span", "diff-head__title");
    const rootEl = el("span", "diff-head__root", repo);
    rootEl.title = repo;
    this.runLabel = el("span", "diff-head__lbl");
    this.runSel = el("select", "diff-select");
    this.runSel.addEventListener("change", () => {
      this.runId = this.runSel.value;
      void this.load();
    });
    this.refreshBtn = el("button", "diff-btn");
    this.refreshBtn.type = "button";
    this.refreshBtn.addEventListener("click", () => void this.load());
    this.closeBtn = el("button", "diff-btn diff-btn--close", "×");
    this.closeBtn.type = "button";
    this.closeBtn.addEventListener("click", () => this.hide(true));
    head.append(
      this.titleEl,
      rootEl,
      el("span", "diff-spacer"),
      this.runLabel,
      this.runSel,
      this.refreshBtn,
      this.closeBtn,
    );

    this.listEl = el("div", "fcmp-list");
    const foot = el("div", "diff-foot");
    this.noticeEl = el("span", "diff-notice");
    this.noticeEl.setAttribute("aria-live", "polite");
    foot.append(this.noticeEl);
    this.el.append(head, this.listEl, foot);
    container.appendChild(this.el);

    this.el.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape") {
        ev.preventDefault();
        ev.stopPropagation();
        this.hide(true);
      } else if (ev.key === "Tab") {
        trapTab(this.el, ev);
      }
    });
    this.unsubLang = onLangChange(() => this.applyLabels());
    this.applyLabels();
  }

  show(): void {
    this.visible = true;
    this.prevFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    pushPopup();
    this.el.classList.add("diff-review--visible");
    this.el.focus();
    void this.load();
  }

  hide(restoreFocus: boolean): void {
    if (!this.visible) return;
    this.visible = false;
    this.tok++;
    this.unsubLang();
    popPopup();
    this.el.remove();
    if (current === this) current = null;
    if (restoreFocus) this.prevFocus?.focus();
    this.prevFocus = null;
  }

  private applyLabels(): void {
    this.titleEl.textContent = t("fanoutCmp.title");
    this.runLabel.textContent = t("fanoutCmp.run");
    this.refreshBtn.textContent = t("diff.refresh");
    this.closeBtn.title = t("diff.close");
    this.closeBtn.setAttribute("aria-label", t("diff.close"));
    this.render();
  }

  private say(msg: string): void {
    this.noticeEl.textContent = msg;
  }

  // ── Loading ──────────────────────────────────────────────────────────────

  private async load(): Promise<void> {
    const tok = ++this.tok;
    this.loading = true;
    this.loadError = null;
    this.render();
    try {
      const entries = await gitApi.worktrees(this.repo);
      if (tok !== this.tok) return;
      this.main = entries.find((e) => e.main)?.path ?? this.repo;
      this.mainBranch = entries.find((e) => e.main)?.branch ?? "";
      const refs: LaneRef[] = [];
      const metas = await Promise.all(
        entries.map((e) =>
          e.main || e.bare || !e.branch ? null : gitApi.laneMeta(e.path).catch(() => null),
        ),
      );
      entries.forEach((e, i) => {
        const m = metas[i];
        if (m && m.fanout && m.base) refs.push({ path: e.path, branch: e.branch, base: m.base, runId: m.fanout });
      });
      if (tok !== this.tok) return;
      this.refs = refs;
      const runs = runIds(refs);
      if (!runs.includes(this.runId)) this.runId = runs[0] ?? "";
      this.empty = refs.length === 0;
      const byPath = new Map(entries.map((e) => [e.path, e]));
      const rows = await Promise.all(
        lanesOfRun(refs, this.runId).map((ref) => this.buildRow(ref, byPath.get(ref.path)!)),
      );
      if (tok !== this.tok) return;
      this.rows = rows;
    } catch (e) {
      if (tok !== this.tok) return;
      this.loadError = gitErrorText(e);
      this.rows = [];
    }
    this.loading = false;
    this.render();
  }

  private async buildRow(ref: LaneRef, entry: WorktreeEntry): Promise<Row> {
    const [diff, log, count, status, panes] = await Promise.allSettled([
      gitApi.diffFiles(ref.path, ref.base, false),
      gitApi.log(ref.path, LOG_LIMIT, 0),
      gitApi.commitsAhead(ref.path, ref.base),
      gitApi.workStatus(ref.path, false),
      this.mgr.terminalPanesWithin(ref.path),
    ]);
    const exact = count.status === "fulfilled" ? count.value : null;
    const ahead =
      log.status === "fulfilled" || exact !== null
        ? commitsAhead(log.status === "fulfilled" ? log.value : [], ref.base, exact)
        : null;
    const pane =
      panes.status === "fulfilled" ? (panes.value.find((p) => p.hasAgent) ?? panes.value[0]) : undefined;
    return {
      ref,
      entry,
      stat: diff.status === "fulfilled" ? diffStat(diff.value.files) : null,
      ahead,
      commits: log.status === "fulfilled" && ahead ? log.value.slice(0, ahead.n) : [],
      dirty: status.status === "fulfilled" ? status.value.changes.length : 0,
      paneId: pane?.id ?? null,
      agent: pane ? (this.mgr.leadKindOf(pane.id) ?? "") : "",
      error:
        diff.status === "rejected" ? gitErrorText(diff.reason) : null,
    };
  }

  // ── Rendering ────────────────────────────────────────────────────────────

  private render(): void {
    const opts = runIds(this.refs).map((id) => {
      const o = el("option", undefined, id);
      o.value = id;
      return o;
    });
    this.runSel.replaceChildren(...opts);
    this.runSel.value = this.runId;
    this.runSel.disabled = opts.length < 2;

    const kids: HTMLElement[] = [];
    if (this.loadError) kids.push(el("div", "fcmp-msg", this.loadError));
    else if (this.loading && this.rows.length === 0) kids.push(el("div", "fcmp-msg", t("diff.loading")));
    else if (this.empty) kids.push(el("div", "fcmp-msg", t("fanoutCmp.empty")));
    for (const r of this.rows) kids.push(this.renderRow(r));
    this.listEl.replaceChildren(...kids);
  }

  private renderRow(r: Row): HTMLElement {
    const row = el("div", "fcmp-row");
    const name = el("div", "fcmp-row__name", r.ref.branch);
    name.title = r.ref.path;
    const status = this.statusText(r);
    const bits: string[] = [];
    if (r.error) bits.push(r.error);
    else if (r.stat) {
      const more = r.stat.partial ? "+" : "";
      bits.push(
        fill(t("fanoutCmp.files"), { n: r.stat.files, a: `${r.stat.added}${more}`, d: `${r.stat.deleted}${more}` }),
      );
    }
    if (r.ahead) {
      bits.push(fill(t("fanoutCmp.commits"), { n: `${r.ahead.n}${r.ahead.more ? "+" : ""}` }));
    }
    if (r.dirty > 0) bits.push(`${t("fanoutCmp.dirty")} (${r.dirty})`);
    const meta = el("div", "fcmp-row__meta", [status, ...bits].join("  ·  "));

    const actions = el("div", "fcmp-row__actions");
    const btn = (label: string, fn: () => void, primary = false, off = false) => {
      const b = el("button", primary ? "diff-btn diff-btn--primary" : "diff-btn", label);
      b.type = "button";
      b.disabled = off;
      b.addEventListener("click", fn);
      actions.appendChild(b);
    };
    btn(t("fanoutCmp.review"), () => {
      this.hide(false);
      void openDiffReview(r.ref.path, r.ref.base);
    });
    btn(t("fanoutCmp.focus"), () => {
      const id = r.paneId;
      if (!id) return;
      this.hide(false);
      void this.mgr.focusPane(id);
    }, false, r.paneId === null);
    btn(t("fanoutKeep.btn"), () => void this.keepThis(r), true);
    btn(t("fanoutCmp.keep"), () => void this.keep(r));
    row.append(name, meta, actions);
    return row;
  }

  private statusText(r: Row): string {
    if (!r.paneId) return t("fanoutCmp.noPane");
    const st = this.mgr.paneStatus.get(r.paneId) ?? "idle";
    return r.agent ? `${r.agent} · ${st}` : st;
  }

  // ── Keep this one: merge + clean up (backend: git_keep_lane) ─────────────

  private async keepThis(r: Row): Promise<void> {
    const lanes = this.refs.filter((l) => l.runId === r.ref.runId);
    // The base is whatever the base checkout has out right now; the backend
    // refuses if that has moved by the time it runs.
    const base = this.mainBranch;
    if (!base) {
      await this.showReport(refusal("base-branch"), this.main);
      return;
    }
    const choice = await this.askKeep(r, lanes, base);
    if (!choice || !this.visible) return;
    this.say(t("fanoutKeep.checking"));
    let pre: KeepReport;
    try {
      pre = await gitApi.keepLane(keepRequest(this.repo, base, lanes, r.ref, choice, true));
    } catch (e) {
      this.say("");
      await showGitError(t("fanoutKeep.title2"), e);
      return;
    }
    this.say("");
    if (pre.outcome !== "ok") {
      if (pre.outcome === "refused" && pre.reason === "lane-dirty" && r.paneId) {
        // The existing way out: ask the lane's agent to commit.
        const a = await askChoice(
          t("fanoutKeep.title2"),
          reportLines(pre, base).map((l) => fill(t(l.key), l.vars)).join("\n"),
          [
            { id: "commit", label: t("fanoutCmp.askCommit"), primary: true },
            { id: "close", label: t("diff.close") },
          ],
        );
        if (a?.id === "commit") await this.askCommit(r);
        return;
      }
      await this.showReport(pre, base);
      return;
    }
    // Step 1: the merge alone. If it fails nothing has been closed or removed.
    this.say(t("fanoutKeep.merging"));
    let merge: KeepReport;
    try {
      merge = await gitApi.keepLane(mergeRequest(this.repo, base, lanes, r.ref, choice));
    } catch (e) {
      this.say("");
      await showGitError(t("fanoutKeep.title2"), e);
      return;
    }
    this.say("");
    if (merge.outcome !== "merged") {
      await this.showReport(merge, base);
      if (this.visible) void this.load();
      return;
    }
    // Step 2: close the terminals in the worktrees about to go (quietly: the
    // keep dialog already confirmed everything), then remove what is merged.
    // A terminal that stays open keeps its lane; the merge stands. None of
    // this needs the overlay: if it was closed meanwhile the merge has still
    // happened, so the steps and the report (its own dialog) still run.
    const open: string[] = [];
    for (const dir of pathsToVacate(lanes, r.ref, choice)) {
      if (!(await this.closeTerminalsIn(dir))) open.push(dir);
    }
    let cleanup: KeepReport | null = null;
    const creq = cleanupRequest(this.repo, base, lanes, r.ref, choice, open);
    if (hasCleanup(creq)) {
      this.say(t("fanoutKeep.cleaning"));
      try {
        cleanup = await gitApi.keepLane(creq);
      } catch (e) {
        cleanup = { ...refusal(""), outcome: "failed", detail: gitErrorText(e) };
      }
      this.say("");
    }
    await this.showReport(
      combineReports(merge, cleanup, openSkips(lanes, r.ref, choice, open), pre.conflict_check_unavailable),
      base,
    );
    if (this.visible) void this.load();
  }

  /// Closes every terminal working inside `dir`. False when one stayed open.
  private async closeTerminalsIn(dir: string): Promise<boolean> {
    for (const p of await this.mgr.terminalPanesWithin(dir)) {
      if (!(await this.mgr.closePaneQuietly(p.id))) return false;
    }
    return (await this.mgr.terminalPanesWithin(dir)).length === 0;
  }

  /// The "what will happen" dialog. Null when cancelled.
  private askKeep(r: Row, lanes: LaneRef[], base: string): Promise<KeepChoice | null> {
    const plan = keepPlan(lanes, r.ref, base, this.main, DEFAULT_CHOICE);
    let picked: KeepChoice | null = null;
    return openDialog((card, done) => {
      const msg = el("div", "dialog__message", fill(t("fanoutKeep.title"), { branch: plan.branch }));
      const lines = [
        fill(t("fanoutKeep.step1"), { branch: plan.branch, base: plan.base, path: plan.keptPath }),
        "",
      ];
      if (plan.removals.length > 0) {
        lines.push(t("fanoutKeep.step2"));
        for (const x of plan.removals) lines.push(`  ${x.branch}  (${x.path})`);
      } else {
        lines.push(t("fanoutKeep.noOthers"));
      }
      card.append(msg, el("div", "dialog__detail", lines.join("\n")));

      const check = (label: string, on: boolean): HTMLInputElement => {
        const wrap = el("label", "dialog__check");
        const box = el("input");
        box.type = "checkbox";
        box.checked = on;
        wrap.append(box, document.createTextNode(label));
        card.appendChild(wrap);
        return box;
      };
      const removeKept = check(t("fanoutKeep.removeKept"), DEFAULT_CHOICE.removeKept);
      const discard =
        plan.removals.length > 0 ? check(t("fanoutKeep.discard"), DEFAULT_CHOICE.discardDirty) : null;
      card.appendChild(el("div", "dialog__detail", t("fanoutKeep.closeNote")));

      const row = el("div", "dialog__buttons");
      const btn = (label: string, primary: boolean, fn: () => void): HTMLButtonElement => {
        const b = el("button", primary ? "dialog__btn dialog__btn--primary" : "dialog__btn", label);
        b.addEventListener("click", fn);
        row.appendChild(b);
        return b;
      };
      btn(t("dialog.cancel"), false, () => done(null));
      btn(t("fanoutKeep.goMerge"), false, () => {
        picked = { mode: "merge-only", removeKept: false, discardDirty: false };
        done("go");
      });
      const go = btn(t("fanoutKeep.goClean"), true, () => {
        picked = {
          mode: "cleanup",
          removeKept: removeKept.checked,
          discardDirty: discard?.checked ?? false,
        };
        done("go");
      });
      card.appendChild(row);
      return go;
    }).then((a) => (a === "go" ? picked : null));
  }

  private async showReport(rep: KeepReport, base: string): Promise<void> {
    const lines = reportLines(rep, base).map((l) => {
      const mark = l.tone === "ok" ? "✓" : l.tone === "warn" ? "–" : "✗";
      return `${mark} ${fill(t(l.key), l.vars)}`;
    });
    const title = rep.outcome === "merged" || rep.outcome === "cleaned" ? t("fanoutKeep.doneTitle") : t("fanoutKeep.title2");
    await askChoice(title, lines.join("\n"), [{ id: "ok", label: t("dialog.ok"), primary: true }]);
  }

  // ── Keep (by hand) ───────────────────────────────────────────────────────

  /// Guidance only: nothing here merges or deletes without the user pressing
  /// Enter in a terminal or confirming a dialog.
  private async keep(r: Row): Promise<void> {
    for (;;) {
      const cmd = mergeCommand(r.ref.branch, "unknown") ?? `git merge --no-ff ${r.ref.branch}`;
      let body = fill(t("fanoutCmp.keepBody"), { main: this.main, cmd });
      if (r.dirty > 0) body = `${t("fanoutCmp.dirtyNote")}\n\n${body}`;
      const choices = [];
      if (r.dirty > 0 && r.paneId) choices.push({ id: "commit", label: t("fanoutCmp.askCommit") });
      choices.push({ id: "type", label: t("fanoutCmp.typeMerge"), primary: true });
      if (cleanupTargets(this.refs.filter((l) => l.runId === r.ref.runId), r.ref.path).length > 0) {
        choices.push({ id: "cleanup", label: t("fanoutCmp.cleanup") });
      }
      choices.push({ id: "close", label: t("diff.close") });
      const a = await askChoice(fill(t("fanoutCmp.keepTitle"), { branch: r.ref.branch }), body, choices);
      if (!this.visible || !a || a.id === "close") return;
      if (a.id === "commit") {
        if (await this.askCommit(r)) return;
      } else if (a.id === "type") {
        if (await this.typeMerge(r)) return;
      } else if (a.id === "cleanup") {
        await this.cleanup(r);
        return;
      }
    }
  }

  /// Types a commit request into the lane's agent WITHOUT submitting it.
  private async askCommit(r: Row): Promise<boolean> {
    const res = r.paneId ? await this.mgr.sendToPane(r.paneId, COMMIT_ASK, false, undefined, { from: null, kind: "commit" }) : "gone";
    if (res !== "pasted" && res !== "sent") {
      this.say(t("fanoutCmp.commitNotSent"));
      return false;
    }
    this.hide(false);
    void api.notify(t("fanoutCmp.title"), t("fanoutCmp.commitAsked")).catch(() => {});
    return true;
  }

  /// Types the merge command (no Enter) into a plain terminal of the main
  /// worktree, opening one when there is none.
  private async typeMerge(r: Row): Promise<boolean> {
    const lanePanes = new Set<Uuid>();
    for (const l of this.refs) for (const p of await this.mgr.terminalPanesWithin(l.path)) lanePanes.add(p.id);
    let pane = (await this.mgr.terminalPanesWithin(this.main)).find((p) => !p.hasAgent && !lanePanes.has(p.id));
    if (!pane) {
      const id = await this.mgr.splitFocused("horizontal", this.main);
      if (!id) return this.typeFailed();
      // The new shell is not ready for typing until its first prompt.
      await this.mgr.waitForFirstPrompt(id);
      pane = { id, hasAgent: false, shell: this.mgr.paneShellFamily(id) };
    }
    const cmd = mergeCommand(r.ref.branch, pane.shell);
    if (cmd === null) return this.typeFailed();
    const typed = await this.mgr.typeIntoShell(pane.id, cmd);
    if (typed !== "ok") return this.typeFailed(typed);
    this.hide(false);
    void api.notify(t("fanoutCmp.title"), t("fanoutCmp.typed")).catch(() => {});
    return true;
  }

  private typeFailed(why?: "busy" | "no-paste" | "control" | "gone" | "failed"): boolean {
    this.say(
      t(
        why === "busy"
          ? "fanoutCmp.typeBusy"
          : why === "no-paste"
            ? "fanoutCmp.typeNoPaste"
            : "fanoutCmp.typeFailed",
      ),
    );
    return false;
  }

  /// Remove every other lane of the run (each through the worktree removal
  /// flow and its own confirmation), then ask separately about their branches.
  private async cleanup(keep: Row): Promise<void> {
    const gone: Row[] = [];
    for (const row of this.rows) {
      if (row.ref.path === keep.ref.path) continue;
      if (!cleanupTargets(this.refs, keep.ref.path).some((l) => l.path === row.ref.path)) continue;
      const out = await removeWorktreeFlow(row.entry, () => this.mgr.livePanes());
      if (!this.visible) return;
      if (out === "removed") gone.push(row);
    }
    if (gone.length > 0) await this.deleteBranches(gone);
    if (this.visible) void this.load();
  }

  private async deleteBranches(gone: Row[]): Promise<void> {
    const list = gone
      .map((g) => {
        const head = g.ahead && g.ahead.n > 0
          ? fill(t("fanoutCmp.branchCommits"), { branch: g.ref.branch, n: `${g.ahead.n}${g.ahead.more ? "+" : ""}` })
          : g.ref.branch;
        return g.commits.length > 0 ? `${head}\n${commitLines(g.commits, false)}` : head;
      })
      .join("\n\n");
    if (!(await askConfirm(fill(t("fanoutCmp.deleteConfirm"), { list }), t("fanoutCmp.deleteOk")))) return;
    let deleted = 0;
    const refused: string[] = [];
    for (const g of gone) {
      try {
        await gitApi.branchDelete(this.main, g.ref.branch);
        deleted++;
      } catch (e) {
        refused.push(fill(t("fanoutCmp.kept"), { branch: g.ref.branch, reason: gitErrorText(e) }));
      }
    }
    const lines = [fill(t("fanoutCmp.deleted"), { n: deleted }), ...refused];
    await askChoice(t("fanoutCmp.title"), lines.join("\n"), [
      { id: "ok", label: t("dialog.ok"), primary: true },
    ]);
  }
}
