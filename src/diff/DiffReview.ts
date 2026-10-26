// Full-window diff review overlay. Runtime-only (no pane kind, no config):
// one instance per repo root (the backend's spelling of it), created on first
// open and kept alive while hidden, so the comments and selection survive
// closing it. At most MAX_HIDDEN hidden instances are kept (least recently
// shown goes first, those holding comments last).
//
// Every piece of diff text, path and comment reaches the DOM through
// textContent only.

import type { Uuid } from "../types";
import type { WorkspaceManager } from "../workspace/WorkspaceManager";
import { errorKind, gitApi } from "../ipc/bridge";
import { t, onLangChange } from "../i18n/i18n";
import { pushPopup, popPopup } from "../browser/popupBlur";
import { askChoice, askConfirm } from "../ui/Dialog";
import { focusRefreshDue } from "../git/gitModel";
import { fill, gitErrorText } from "../git/worktreeFlow";
import type { DiffFile, DiffLine, DiffSummary, FileDiff, Hunk } from "./types";
import type { CommentSide, ReviewComment } from "./model-types";
import { cachedFile, excerpt, fileCacheKey, numberOn, reanchor, toSideBySide } from "./diffModel";
import { composePrompt, PROMPT_WARN, prefixLines } from "./reviewPrompt";
import { chooseTarget } from "./targetPick";
import { trapTab } from "../ui/trapTab";

/// Rows rendered per file before "Show more lines"; keeps a huge diff from
/// building tens of thousands of nodes.
const ROW_PAGE = 2000;
const VIEW_KEY = "ymux.diffReview.view";
/// Hidden instances kept alive (comments + caches) before the oldest is dropped.
const MAX_HIDDEN = 4;
/// Wait this long after an agent finishes before re-reading the diff.
const AGENT_DONE_REFRESH_MS = 1000;

type View = "unified" | "split";
type Anchor = { path: string; side: CommentSide; n: number };

let host: WorkspaceManager | null = null;
let mountEl: HTMLElement | null = null;
/// Bumped per open; a slower earlier root resolution must not win.
let openSeq = 0;
/// Least recently shown first.
const reviews = new Map<string, Review>();
/// Separates the root from a preset base in an instance key.
const BASE_SEP = "\u0000base:";

/// Wire the overlay to the app. Call once, after the manager exists.
export function mountDiffReview(container: HTMLElement, manager: WorkspaceManager): void {
  mountEl = container;
  host = manager;
}

/// Is a review overlay on screen? Global shortcuts that act on the panes
/// behind it check this and stand down.
export function isDiffReviewOpen(): boolean {
  for (const r of reviews.values()) if (r.isVisible) return true;
  return false;
}

/// Show the review for the repository containing `root`, creating it on
/// first use. The instance is keyed by the backend's spelling of the root so
/// the palette and the Git pane share one.
/// `base` (a branch or sha) opens a separate instance compared against that
/// base, so it never changes the base of the repository's ordinary review.
export async function openDiffReview(root: string, base?: string): Promise<void> {
  if (!mountEl || !host) return;
  const seq = ++openSeq;
  let canon = root;
  let resolved = true;
  try {
    canon = await gitApi.repoRoot(root);
  } catch {
    /* gone or not a repo: the instance itself reports it */
    resolved = false;
  }
  if (seq !== openSeq || !mountEl || !host) return;
  const rootKey = canon.normalize("NFC");
  const suffix = base ? BASE_SEP + base : "";
  let key = rootKey + suffix;
  if (!resolved && !reviews.has(key)) {
    // The raw spelling may be another spelling of a root that already has an
    // instance: reuse it (it reports the root gone) rather than add a twin.
    for (const [k, v] of reviews) {
      if (
        v.root.normalize("NFC") === rootKey &&
        k.endsWith(suffix) &&
        (suffix !== "" || !k.includes(BASE_SEP))
      ) {
        key = k;
      }
    }
  }
  let r = reviews.get(key);
  if (r) reviews.delete(key);
  else {
    r = new Review(canon, mountEl, host, () => reviews.delete(key));
    if (base) r.presetBase(base);
  }
  reviews.set(key, r);
  for (const other of reviews.values()) if (other !== r) other.hide(false);
  r.show();
  // Bound what hidden instances keep alive.
  const hidden = [...reviews.values()].filter((x) => !x.isVisible);
  while (hidden.length > MAX_HIDDEN) {
    const i = hidden.findIndex((x) => !x.hasComments);
    hidden.splice(i >= 0 ? i : 0, 1)[0].destroy();
  }
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

/// Where a click on `l` lands: deletions on the old side, everything else on
/// the new one. `noeol` markers carry no line.
function anchorOf(l: DiffLine): { side: CommentSide; n: number } | null {
  if (l.kind === "noeol") return null;
  if (l.kind === "del") return l.old_no === null ? null : { side: "old", n: l.old_no };
  return l.new_no === null ? null : { side: "new", n: l.new_no };
}

function findLine(hunks: Hunk[], side: CommentSide, n: number): DiffLine | null {
  for (const h of hunks) for (const l of h.lines) if (numberOn(l, side) === n) return l;
  return null;
}

class Review {
  private readonly el: HTMLElement;
  private readonly baseSel: HTMLSelectElement;
  private readonly unifiedBtn: HTMLButtonElement;
  private readonly splitBtn: HTMLButtonElement;
  private readonly crBox: HTMLInputElement;
  private readonly crLabel: HTMLSpanElement;
  private readonly baseLabelEl: HTMLSpanElement;
  private readonly refreshBtn: HTMLButtonElement;
  private readonly closeBtn: HTMLButtonElement;
  private readonly titleEl: HTMLElement;
  private readonly listEl: HTMLElement;
  private readonly diffEl: HTMLElement;
  private readonly clistEl: HTMLElement;
  private readonly countBtn: HTMLButtonElement;
  private readonly noticeEl: HTMLElement;
  private readonly submitBox: HTMLInputElement;
  private readonly submitLabel: HTMLSpanElement;
  private readonly copyBtn: HTMLButtonElement;
  private readonly sendBtn: HTMLButtonElement;

  private visible = false;
  private popped = false;
  private prevFocus: HTMLElement | null = null;
  private summary: DiffSummary | null = null;
  private selected: string | null = null;
  private cache = new Map<string, FileDiff>();
  private fileErr = new Map<string, string>();
  private loadError: string | null = null;
  private loading = false;
  private refreshAgain = false;
  private refreshTok = 0;
  private lastRefresh: number | null = null;
  private base = "";
  private view: View = "unified";
  private rowLimit = ROW_PAGE;
  private comments: ReviewComment[] = [];
  private anchor: Anchor | null = null;
  private composer: HTMLElement | null = null;
  private composerDirty = false;
  private pendingRender = false;
  private listOpen = false;
  private lastTarget: Uuid | null = null;
  private noticeTimer: number | null = null;
  private branches: string[] = [];
  private branchesLoaded = false;
  private sending = false;
  /// The root no longer resolves (a removed worktree): shown once, then the
  /// instance is dropped when it is closed.
  private gone = false;
  /// `ignoreCr` the current summary (and so every cached file diff) was read with.
  private summaryIgnoreCr = false;
  private readonly unsubLang: () => void;
  private unsubTree: (() => void) | null = null;
  private leadStatus = new Map<string, string>();
  private agentTimer: number | null = null;

  constructor(
    readonly root: string,
    container: HTMLElement,
    private readonly mgr: WorkspaceManager,
    private readonly onDrop: () => void,
  ) {
    try {
      if (localStorage.getItem(VIEW_KEY) === "split") this.view = "split";
    } catch {
      /* storage unavailable */
    }

    this.el = el("div", "diff-review");
    this.el.tabIndex = -1;
    this.el.setAttribute("role", "dialog");
    this.el.setAttribute("aria-modal", "true");

    const head = el("div", "diff-head");
    this.titleEl = el("span", "diff-head__title");
    const rootEl = el("span", "diff-head__root", root);
    rootEl.title = root;
    this.baseLabelEl = el("span", "diff-head__lbl");
    this.baseSel = el("select", "diff-select");
    this.baseSel.addEventListener("change", () => {
      this.base = this.baseSel.value;
      void this.refresh();
    });
    const seg = el("span", "diff-seg");
    this.unifiedBtn = el("button", "diff-btn");
    this.splitBtn = el("button", "diff-btn");
    this.unifiedBtn.type = this.splitBtn.type = "button";
    this.unifiedBtn.addEventListener("click", () => void this.setView("unified"));
    this.splitBtn.addEventListener("click", () => void this.setView("split"));
    seg.append(this.unifiedBtn, this.splitBtn);
    const crWrap = el("label", "diff-check");
    this.crBox = el("input");
    this.crBox.type = "checkbox";
    this.crBox.addEventListener("change", () => void this.refresh());
    this.crLabel = el("span");
    crWrap.append(this.crBox, this.crLabel);
    this.refreshBtn = el("button", "diff-btn");
    this.refreshBtn.type = "button";
    this.refreshBtn.addEventListener("click", () => void this.refresh());
    this.closeBtn = el("button", "diff-btn diff-btn--close", "×");
    this.closeBtn.type = "button";
    this.closeBtn.addEventListener("click", () => this.hide(true));
    head.append(
      this.titleEl,
      rootEl,
      el("span", "diff-spacer"),
      this.baseLabelEl,
      this.baseSel,
      seg,
      crWrap,
      this.refreshBtn,
      this.closeBtn,
    );

    const body = el("div", "diff-body");
    this.listEl = el("div", "diff-files");
    this.diffEl = el("div", "diff-view");
    this.diffEl.addEventListener("click", (ev) => this.onViewClick(ev));
    body.append(this.listEl, this.diffEl);

    this.clistEl = el("div", "diff-clist");
    this.clistEl.hidden = true;
    const foot = el("div", "diff-foot");
    this.countBtn = el("button", "diff-btn");
    this.countBtn.type = "button";
    this.countBtn.addEventListener("click", () => {
      this.listOpen = !this.listOpen;
      this.renderFooter();
    });
    this.noticeEl = el("span", "diff-notice");
    this.noticeEl.setAttribute("aria-live", "polite");
    const subWrap = el("label", "diff-check");
    this.submitBox = el("input");
    this.submitBox.type = "checkbox";
    this.submitLabel = el("span");
    subWrap.append(this.submitBox, this.submitLabel);
    this.copyBtn = el("button", "diff-btn");
    this.copyBtn.type = "button";
    this.copyBtn.addEventListener("click", () => void this.copyOnly());
    this.sendBtn = el("button", "diff-btn diff-btn--primary");
    this.sendBtn.type = "button";
    this.sendBtn.addEventListener("click", () => void this.send());
    foot.append(this.countBtn, this.noticeEl, el("span", "diff-spacer"), subWrap, this.copyBtn, this.sendBtn);

    this.el.append(head, body, this.clistEl, foot);
    container.appendChild(this.el);

    this.el.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape") {
        ev.preventDefault();
        ev.stopPropagation();
        this.hide(true);
      } else if (ev.key === "Tab") {
        this.trapTab(ev);
      }
    });
    this.unsubLang = onLangChange(() => this.applyLabels());
    this.applyLabels();
  }

  show(): void {
    if (this.visible) {
      this.el.focus();
      return;
    }
    this.visible = true;
    this.prevFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (!this.popped) {
      this.popped = true;
      pushPopup();
    }
    this.el.classList.add("diff-review--visible");
    window.addEventListener("focus", this.onWindowFocus);
    this.unsubTree?.();
    this.leadStatus = this.leadStatuses();
    this.unsubTree = this.mgr.onTreeChange(() => this.onAgentsChanged());
    this.el.focus();
    if (this.gone) return;
    void this.loadBranches();
    void this.refresh();
  }

  get isVisible(): boolean {
    return this.visible;
  }

  get hasComments(): boolean {
    return this.comments.length > 0;
  }

  /// Tear the instance down for good: DOM, listeners, timers.
  destroy(): void {
    this.hide(false);
    this.refreshTok++;
    this.unsubLang();
    if (this.noticeTimer !== null) clearTimeout(this.noticeTimer);
    this.noticeTimer = null;
    this.el.remove();
    this.onDrop();
  }

  private trapTab(ev: KeyboardEvent): void {
    trapTab(this.el, ev);
  }

  private leadStatuses(): Map<string, string> {
    const m = new Map<string, string>();
    for (const [id, a] of Object.entries(this.mgr.agents)) {
      if (a.lead) m.set(id, a.lead.status);
    }
    return m;
  }

  /// An agent that was working has finished: its edits are on disk, so
  /// re-read the diff (debounced). Only the pane last sent to counts once
  /// there is one.
  private onAgentsChanged(): void {
    const next = this.leadStatuses();
    let finished = false;
    for (const [id, status] of next) {
      if (this.lastTarget !== null && id !== this.lastTarget) continue;
      if (this.leadStatus.get(id) === "working" && (status === "done" || status === "idle")) finished = true;
    }
    this.leadStatus = next;
    if (!finished || !this.visible || this.gone) return;
    if (this.agentTimer !== null) clearTimeout(this.agentTimer);
    this.agentTimer = window.setTimeout(() => {
      this.agentTimer = null;
      if (!this.visible || this.gone) return;
      // A refresh already in flight may predate the agent's last edits.
      if (this.loading) this.refreshAgain = true;
      else void this.refresh();
    }, AGENT_DONE_REFRESH_MS);
  }

  hide(restoreFocus: boolean): void {
    if (!this.visible) return;
    this.visible = false;
    if (this.popped) {
      this.popped = false;
      popPopup();
    }
    this.el.classList.remove("diff-review--visible");
    window.removeEventListener("focus", this.onWindowFocus);
    this.unsubTree?.();
    this.unsubTree = null;
    if (this.agentTimer !== null) clearTimeout(this.agentTimer);
    this.agentTimer = null;
    if (restoreFocus) this.prevFocus?.focus();
    this.prevFocus = null;
    if (this.gone) this.destroy();
  }

  private readonly onWindowFocus = (): void => {
    if (!this.visible || this.gone) return;
    const now = Date.now();
    if (!focusRefreshDue(now, this.lastRefresh, this.loading)) return;
    void this.refresh();
  };

  // ── Labels ───────────────────────────────────────────────────────────────

  private applyLabels(): void {
    this.titleEl.textContent = t("diff.title");
    this.baseLabelEl.textContent = t("diff.base");
    this.unifiedBtn.textContent = t("diff.unified");
    this.splitBtn.textContent = t("diff.split");
    this.crLabel.textContent = t("diff.ignoreCr");
    this.refreshBtn.textContent = t("diff.refresh");
    this.closeBtn.title = t("diff.close");
    this.closeBtn.setAttribute("aria-label", t("diff.close"));
    this.el.setAttribute("aria-label", t("diff.title"));
    this.submitLabel.textContent = t("diff.submit");
    this.copyBtn.textContent = t("diff.copy");
    this.sendBtn.textContent = t("diff.send");
    this.renderBaseOptions();
    this.syncViewButtons();
    this.renderFooter();
    this.renderList();
    this.renderFile();
  }

  private syncViewButtons(): void {
    this.unifiedBtn.classList.toggle("diff-btn--on", this.view === "unified");
    this.splitBtn.classList.toggle("diff-btn--on", this.view === "split");
  }

  /// An open comment box with text would be lost by a re-render: ask first.
  private async keepDraft(): Promise<boolean> {
    if (!this.composer || !this.composerDirty) return true;
    return askConfirm(t("diff.draftDiscard"), t("editor.discard"));
  }

  private async setView(v: View): Promise<void> {
    if (this.view === v) return;
    if (!(await this.keepDraft())) return;
    this.view = v;
    try {
      localStorage.setItem(VIEW_KEY, v);
    } catch {
      /* storage unavailable */
    }
    this.syncViewButtons();
    this.renderFile();
  }

  private notice(text: string): void {
    if (this.noticeTimer !== null) clearTimeout(this.noticeTimer);
    this.noticeEl.textContent = text;
    this.noticeTimer = window.setTimeout(() => {
      this.noticeEl.textContent = "";
      this.noticeTimer = null;
    }, 8000);
  }

  // ── Base ─────────────────────────────────────────────────────────────────

  /// Set the base of a fresh instance before `show()` reads the diff.
  presetBase(base: string): void {
    this.base = base;
    this.renderBaseOptions();
  }

  private async loadBranches(): Promise<void> {
    try {
      const b = await gitApi.branches(this.root);
      this.branches = b.local.filter((x) => x !== b.current);
    } catch {
      this.branches = [];
    }
    this.branchesLoaded = true;
    this.renderBaseOptions();
  }

  private renderBaseOptions(): void {
    const opts: HTMLOptionElement[] = [];
    const head = el("option", undefined, t("diff.baseHead"));
    head.value = "";
    opts.push(head);
    for (const b of this.branches) {
      const o = el("option", undefined, fill(t("diff.baseBranch"), { branch: b }));
      o.value = b;
      opts.push(o);
    }
    // A chosen base that has since disappeared stays listed until changed. A
    // preset sha is listed at once (it is never in the branch list).
    const sha = /^[0-9a-f]{7,64}$/i.test(this.base);
    if (this.base && !this.branches.includes(this.base) && (this.branchesLoaded || sha)) {
      const o = el(
        "option",
        undefined,
        sha
          ? fill(t("diff.baseSha"), { sha: this.base.slice(0, 7) })
          : fill(t("diff.baseBranch"), { branch: this.base }),
      );
      o.value = this.base;
      opts.push(o);
    }
    this.baseSel.replaceChildren(...opts);
    this.baseSel.value = this.base;
  }

  // ── Loading ──────────────────────────────────────────────────────────────

  async refresh(): Promise<void> {
    const tok = ++this.refreshTok;
    this.loading = true;
    this.lastRefresh = Date.now();
    this.refreshBtn.disabled = true;
    if (!this.summary) this.renderFile();
    const ignoreCr = this.crBox.checked;
    try {
      const sum = await gitApi.diffFiles(this.root, this.base, ignoreCr);
      if (tok !== this.refreshTok) return;
      this.loadError = null;
      this.summary = sum;
      this.summaryIgnoreCr = ignoreCr;
      this.cache.clear();
      this.fileErr.clear();
      const paths = new Set(sum.files.map((f) => f.path));
      if (!this.selected || !paths.has(this.selected)) this.selected = sum.files[0]?.path ?? null;
      // Fresh hunks for the selected file and for every file that carries a
      // comment, so the comments can be re-anchored.
      const want = new Set<string>(this.comments.filter((c) => paths.has(c.path)).map((c) => c.path));
      if (this.selected) want.add(this.selected);
      await Promise.all([...want].map((p) => this.fetchFile(sum, p, ignoreCr)));
      if (tok !== this.refreshTok) return;
      this.comments = this.comments.map((c) => {
        const fd = cachedFile(this.cache, sum, this.summaryIgnoreCr, c.path);
        if (!paths.has(c.path)) return { ...c, stale: true };
        return fd ? reanchor(c, fd.hunks) : c;
      });
    } catch (e) {
      if (tok !== this.refreshTok) return;
      if (errorKind(e) === "not_a_repo") {
        // The folder or worktree is gone (or no longer a repository): say so
        // once; the instance is dropped when closed.
        this.gone = true;
        this.loadError = t("diff.rootGone");
      } else {
        this.loadError = fill(t("diff.error"), { msg: gitErrorText(e) });
      }
      this.summary = null;
    } finally {
      if (tok === this.refreshTok) {
        this.loading = false;
        this.refreshBtn.disabled = this.gone;
      }
    }
    if (tok !== this.refreshTok) return;
    this.renderList();
    this.renderFooter();
    this.renderFileKeepingScroll();
    this.rerunIfAsked();
  }

  private rerunIfAsked(): void {
    if (!this.refreshAgain) return;
    this.refreshAgain = false;
    if (this.visible && !this.gone) void this.refresh();
  }

  /// Cache key of a file's diff: the base commit and the whitespace mode it
  /// was read with, plus the path.
  private fileKey(sum: DiffSummary, path: string): string {
    return fileCacheKey(sum, this.summaryIgnoreCr, path);
  }

  private async fetchFile(sum: DiffSummary, path: string, ignoreCr: boolean): Promise<void> {
    const f = sum.files.find((x) => x.path === path);
    if (!f) return;
    const key = this.fileKey(sum, path);
    let fd: FileDiff | null = null;
    let err: string | null = null;
    try {
      fd = await gitApi.diffFile(this.root, sum.base_rev, f.path, f.orig, f.untracked, ignoreCr);
    } catch (e) {
      err = fill(t("diff.error"), { msg: gitErrorText(e) });
    }
    // A newer summary (other base, other mode, a refresh) owns the cache now.
    if (this.summary !== sum || ignoreCr !== this.summaryIgnoreCr) return;
    if (fd) this.cache.set(key, fd);
    else if (err) this.fileErr.set(key, err);
  }

  private async select(path: string): Promise<void> {
    if (this.selected === path) return;
    if (!(await this.keepDraft())) return;
    this.selected = path;
    this.rowLimit = ROW_PAGE;
    this.anchor = null;
    this.renderList();
    const sum = this.summary;
    const key = sum ? this.fileKey(sum, path) : "";
    if (sum && !this.cache.has(key) && !this.fileErr.has(key)) {
      this.renderFile();
      await this.fetchFile(sum, path, this.summaryIgnoreCr);
      if (this.selected !== path || this.summary !== sum) return;
    }
    this.renderFile();
    this.diffEl.scrollTop = 0;
  }

  // ── Rendering: file list ─────────────────────────────────────────────────

  private renderList(): void {
    const sum = this.summary;
    const rows: HTMLElement[] = [];
    if (sum) {
      for (const f of sum.files) rows.push(this.fileRow(f));
      if (sum.truncated) rows.push(el("div", "diff-msg", t("diff.filesTruncated")));
    }
    this.listEl.replaceChildren(...rows);
  }

  private fileRow(f: DiffFile): HTMLElement {
    const row = el("div", "diff-file");
    row.classList.toggle("diff-file--sel", f.path === this.selected);
    row.title = f.orig ? `${f.orig} → ${f.path}` : f.path;
    const badge = f.untracked ? "N" : f.orig ? "R" : f.binary ? "B" : "M";
    const b = el("span", `diff-badge diff-badge--${f.untracked ? "new" : f.orig ? "ren" : "mod"}`, badge);
    if (f.untracked) b.title = t("diff.untracked");
    else if (f.orig) b.title = fill(t("diff.renamedFrom"), { path: f.orig });
    const slash = f.path.lastIndexOf("/");
    const name = el("span", "diff-file__name", f.path.slice(slash + 1));
    const dir = el("span", "diff-file__dir", slash >= 0 ? f.path.slice(0, slash + 1) : "");
    const path = el("span", "diff-file__path");
    path.append(name, dir);
    const stat = el("span", "diff-file__stat");
    if (f.binary) stat.textContent = "bin";
    else if (f.counts_unknown) stat.textContent = "?";
    else {
      stat.append(el("span", "diff-plus", `+${f.added}`), document.createTextNode(" "), el("span", "diff-minus", `−${f.deleted}`));
    }
    const n = this.comments.filter((c) => c.path === f.path).length;
    row.append(b, path);
    if (n > 0) row.append(el("span", "diff-file__cmt", String(n)));
    row.append(stat);
    row.addEventListener("click", () => void this.select(f.path));
    return row;
  }

  // ── Rendering: the diff ──────────────────────────────────────────────────

  private renderFileKeepingScroll(): void {
    const top = this.diffEl.scrollTop;
    if (this.composer && this.composerDirty) {
      this.pendingRender = true;
      return;
    }
    this.renderFile();
    this.diffEl.scrollTop = top;
  }

  private renderFile(): void {
    this.composer = null;
    this.composerDirty = false;
    this.pendingRender = false;
    const out: HTMLElement[] = [];
    const msg = (text: string) => this.diffEl.replaceChildren(el("div", "diff-msg", text));
    if (this.loadError) return msg(this.loadError);
    const sum = this.summary;
    if (!sum) return msg(t("diff.loading"));
    if (sum.files.length === 0) return msg(t("diff.noChanges"));
    const f = sum.files.find((x) => x.path === this.selected);
    if (!f) return msg(t("diff.selectFile"));
    const err = this.fileErr.get(this.fileKey(sum, f.path));
    if (err) return msg(err);
    const fd = cachedFile(this.cache, sum, this.summaryIgnoreCr, f.path);
    if (!fd) return msg(t("diff.loading"));

    out.push(el("div", "diff-fhead", f.orig ? `${f.orig} → ${f.path}` : f.path));
    if (fd.binary) out.push(el("div", "diff-msg", t("diff.binary")));
    if (fd.truncated) out.push(el("div", "diff-msg diff-msg--warn", t("diff.truncated")));
    out.push(el("div", "diff-msg diff-hint", t("diff.commentHint")));

    const marks = new Set<string>();
    const notes = new Map<string, ReviewComment[]>();
    for (const c of this.comments) {
      if (c.path !== f.path) continue;
      const end = c.endLine ?? c.line;
      for (let n = c.line; n <= end; n++) marks.add(`${c.side}:${n}`);
      const k = `${c.side}:${end}`;
      notes.set(k, [...(notes.get(k) ?? []), c]);
    }

    let rows = 0;
    let capped = false;
    const noteRows = (keys: (string | null)[]): HTMLElement[] => {
      const res: HTMLElement[] = [];
      for (const k of new Set(keys)) {
        if (!k) continue;
        for (const c of notes.get(k) ?? []) res.push(this.noteEl(c));
      }
      return res;
    };
    outer: for (let h = 0; h < fd.hunks.length; h++) {
      const hunk = fd.hunks[h];
      if (rows >= this.rowLimit) {
        capped = true;
        break;
      }
      out.push(
        el("div", "diff-hunk", `@@ -${hunk.old_start},${hunk.old_lines} +${hunk.new_start},${hunk.new_lines} @@ ${hunk.header}`),
      );
      if (this.view === "unified") {
        for (let li = 0; li < hunk.lines.length; li++) {
          if (rows++ >= this.rowLimit) {
            capped = true;
            break outer;
          }
          const l = hunk.lines[li];
          const a = anchorOf(l);
          const row = el("div", `diff-line diff-line--${l.kind}`);
          row.append(
            el("span", "diff-no", l.old_no === null ? "" : String(l.old_no)),
            el("span", "diff-no", l.new_no === null ? "" : String(l.new_no)),
            el("span", "diff-mark", l.kind === "add" ? "+" : l.kind === "del" ? "-" : l.kind === "noeol" ? "" : " "),
            el("span", "diff-text", l.text),
          );
          if (a) {
            row.dataset.h = String(h);
            row.dataset.side = a.side;
            row.dataset.n = String(a.n);
            if (marks.has(`${a.side}:${a.n}`)) row.classList.add("diff-line--commented");
          }
          out.push(row, ...noteRows([a ? `${a.side}:${a.n}` : null]));
        }
      } else {
        for (const r of toSideBySide(hunk)) {
          if (rows++ >= this.rowLimit) {
            capped = true;
            break outer;
          }
          const row = el("div", "diff-srow");
          const keys: (string | null)[] = [];
          for (const [which, l] of [["left", r.left], ["right", r.right]] as const) {
            const cell = el("div", "diff-cell");
            if (l) {
              cell.classList.add(`diff-cell--${l.kind}`);
              const a = anchorOf(l);
              const shown = which === "left" ? l.old_no : l.new_no;
              cell.append(el("span", "diff-no", shown === null ? "" : String(shown)), el("span", "diff-text", l.text));
              if (a) {
                cell.dataset.h = String(h);
                cell.dataset.side = a.side;
                cell.dataset.n = String(a.n);
                keys.push(`${a.side}:${a.n}`);
                if (marks.has(`${a.side}:${a.n}`)) cell.classList.add("diff-cell--commented");
              }
            } else {
              cell.classList.add("diff-cell--empty");
            }
            row.appendChild(cell);
          }
          out.push(row, ...noteRows(keys));
        }
      }
    }
    if (capped) {
      const more = el("button", "diff-btn diff-more", t("diff.showMore"));
      more.type = "button";
      more.addEventListener("click", () => {
        this.rowLimit += ROW_PAGE;
        this.renderFileKeepingScroll();
      });
      out.push(more);
    }
    this.diffEl.replaceChildren(...out);
  }

  private noteEl(c: ReviewComment): HTMLElement {
    const n = el("div", "diff-note");
    n.append(el("span", "diff-note__text", c.text));
    const rm = el("button", "diff-btn diff-btn--x", "×");
    rm.type = "button";
    rm.title = t("diff.commentRemove");
    rm.dataset.rm = c.id;
    n.appendChild(rm);
    return n;
  }

  // ── Comments ─────────────────────────────────────────────────────────────

  private onViewClick(ev: MouseEvent): void {
    const target = ev.target;
    if (!(target instanceof HTMLElement)) return;
    const rm = target.closest<HTMLElement>("[data-rm]");
    if (rm?.dataset.rm) {
      this.removeComment(rm.dataset.rm);
      return;
    }
    if (target.closest(".diff-compose")) return;
    const line = target.closest<HTMLElement>("[data-n]");
    if (!line || !this.selected) return;
    const side = line.dataset.side === "old" ? "old" : "new";
    const n = Number(line.dataset.n);
    if (!Number.isFinite(n)) return;
    let start = n;
    let end = n;
    if (ev.shiftKey && this.anchor && this.anchor.path === this.selected && this.anchor.side === side) {
      start = Math.min(this.anchor.n, n);
      end = Math.max(this.anchor.n, n);
    } else {
      this.anchor = { path: this.selected, side, n };
    }
    this.openComposer(line, side, start, end);
  }

  private openComposer(at: HTMLElement, side: CommentSide, start: number, end: number): void {
    const path = this.selected;
    const fd = cachedFile(this.cache, this.summary, this.summaryIgnoreCr, path);
    if (!path || !fd) return;
    this.composer?.remove();
    const box = el("div", "diff-compose");
    const where = el("div", "diff-compose__where", `${path}:${side === "old" ? "old " : ""}L${end > start ? `${start}-${end}` : start}`);
    const ta = el("textarea", "diff-compose__text");
    ta.rows = 3;
    ta.placeholder = t("diff.commentPlaceholder");
    ta.spellcheck = false;
    ta.addEventListener("input", () => {
      this.composerDirty = ta.value.trim().length > 0;
    });
    const add = el("button", "diff-btn diff-btn--primary", t("diff.commentAdd"));
    const cancel = el("button", "diff-btn", t("diff.commentCancel"));
    add.type = cancel.type = "button";
    const close = () => {
      box.remove();
      this.composer = null;
      this.composerDirty = false;
      if (this.pendingRender) this.renderFileKeepingScroll();
    };
    // What the reviewer saw when the box opened; re-resolved on commit.
    const seen = findLine(fd.hunks, side, start);
    let ex: string[] = [];
    for (const h of fd.hunks) {
      const e = excerpt(h, start, 3, 40, { side, endLine: end });
      if (e.length) {
        ex = prefixLines(e);
        break;
      }
    }
    const commit = () => {
      const text = ta.value.trim();
      if (!text) return;
      if (!seen) return close();
      let c: ReviewComment = { id: crypto.randomUUID(), path, side, line: start, text, lineText: seen.text };
      if (end > start) c.endLine = end;
      if (ex.length) c.excerpt = ex;
      // The diff may have been re-read while the box was open (a refresh
      // keeps it): find the line again, or mark the comment stale.
      const cur = cachedFile(this.cache, this.summary, this.summaryIgnoreCr, path);
      if (cur !== fd) c = cur ? reanchor(c, cur.hunks) : { ...c, stale: true };
      this.comments.push(c);
      this.composerDirty = false;
      this.renderList();
      this.renderFooter();
      this.renderFileKeepingScroll();
    };
    add.addEventListener("click", commit);
    cancel.addEventListener("click", close);
    ta.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape") {
        ev.preventDefault();
        ev.stopPropagation();
        close();
      } else if (ev.key === "Enter" && (ev.ctrlKey || ev.metaKey)) {
        ev.preventDefault();
        commit();
      }
    });
    const btns = el("div", "diff-compose__btns");
    btns.append(add, cancel);
    box.append(where, ta, btns);
    const row = at.closest(".diff-line, .diff-srow") ?? at;
    row.after(box);
    this.composer = box;
    ta.focus();
  }

  private removeComment(id: string): void {
    this.comments = this.comments.filter((c) => c.id !== id);
    this.renderList();
    this.renderFooter();
    this.renderFileKeepingScroll();
  }

  // ── Rendering: footer ────────────────────────────────────────────────────

  private commentWhere(c: ReviewComment): string {
    const range = c.endLine !== undefined && c.endLine !== c.line ? `${c.line}-${c.endLine}` : `${c.line}`;
    return `${c.path}:${c.side === "old" ? "old " : ""}L${range}`;
  }

  private renderFooter(): void {
    const n = this.comments.length;
    this.countBtn.textContent = fill(t("diff.comments"), { n });
    this.countBtn.disabled = n === 0;
    this.copyBtn.disabled = n === 0;
    this.sendBtn.disabled = n === 0 || this.sending;
    if (n === 0) this.listOpen = false;
    this.clistEl.hidden = !this.listOpen;
    if (!this.listOpen) return;
    const items = this.comments.map((c) => {
      const item = el("div", "diff-citem");
      const w = el("span", "diff-citem__where", this.commentWhere(c));
      item.append(w, el("span", "diff-citem__text", c.text));
      if (c.stale) item.append(el("span", "diff-citem__stale", t("diff.stale")));
      const rm = el("button", "diff-btn diff-btn--x", "×");
      rm.type = "button";
      rm.title = t("diff.commentRemove");
      rm.addEventListener("click", (ev) => {
        ev.stopPropagation();
        this.removeComment(c.id);
      });
      item.appendChild(rm);
      item.addEventListener("click", () => void this.select(c.path));
      return item;
    });
    this.clistEl.replaceChildren(...items);
  }

  // ── Send ─────────────────────────────────────────────────────────────────

  private prompt(comments: ReviewComment[] = this.comments): string {
    return composePrompt(this.root, this.base || "HEAD", comments);
  }

  private async copyText(text: string): Promise<boolean> {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch (e) {
      console.warn("clipboard write failed:", e);
      return false;
    }
  }

  private async copyOnly(): Promise<void> {
    if (this.comments.length === 0) return;
    this.notice(t((await this.copyText(this.prompt())) ? "diff.copied" : "diff.copyFailed"));
  }

  private async copyWithReason(text: string, reasonKey: string, target: string): Promise<void> {
    const ok = await this.copyText(text);
    this.notice(ok ? fill(t(reasonKey), { target }) : t("diff.copyFailed"));
  }

  private async send(): Promise<void> {
    if (this.sending || this.comments.length === 0) return;
    this.sending = true;
    this.sendBtn.disabled = true;
    // The Enter can wait several seconds for a fresh scan; show that.
    this.sendBtn.textContent = t("diff.sending");
    try {
      await this.doSend();
    } finally {
      this.sending = false;
      this.sendBtn.textContent = t("diff.send");
      this.renderFooter();
    }
  }

  private async doSend(): Promise<void> {
    // Exactly these comments are in the prompt, so exactly these are removed
    // once it is sent; ones added while the dialogs were open stay.
    const batch = [...this.comments];
    const text = this.prompt(batch);
    const bytes = new TextEncoder().encode(text).length;
    if (bytes > PROMPT_WARN) {
      const ok = await askConfirm(fill(t("diff.bigPrompt"), { kb: Math.ceil(bytes / 1024) }), t("diff.sendAnyway"));
      if (!ok) return;
    }
    let ids: Uuid[];
    try {
      ids = await gitApi.agentPanesForRoot(this.root);
    } catch (e) {
      this.notice(fill(t("diff.error"), { msg: gitErrorText(e) }));
      return;
    }
    const pick = chooseTarget(ids, this.lastTarget);
    if (pick.kind === "none") return this.copyWithReason(text, "diff.noAgent", "");
    // The agent kind the reviewer chose; the send is refused if it changed.
    const kinds = new Map<Uuid, string | undefined>();
    for (const p of pick.kind === "one" ? [pick.id] : pick.ids) kinds.set(p, this.mgr.leadKindOf(p));
    let id: Uuid;
    if (pick.kind === "one") id = pick.id;
    else {
      const ans = await askChoice(
        t("diff.pickTitle"),
        null,
        pick.ids.map((p, i) => ({ id: p, label: this.mgr.paneChoiceLabel(p), primary: i === 0 })),
      );
      if (!ans) return;
      id = ans.id;
    }
    const label = this.mgr.paneChoiceLabel(id);
    const kind = kinds.get(id);
    let res: Awaited<ReturnType<WorkspaceManager["sendToPane"]>>;
    try {
      res = kind === undefined ? "gone" : await this.mgr.sendToPane(id, text, this.submitBox.checked, kind, { from: null, kind: "review" });
    } catch (e) {
      console.warn("sendToPane failed:", e);
      res = "failed";
    }
    if (res === "sent" || res === "pasted") {
      this.lastTarget = id;
      const sent = new Set(batch.map((c) => c.id));
      this.comments = this.comments.filter((c) => !sent.has(c.id));
      this.renderList();
      this.renderFooter();
      this.renderFileKeepingScroll();
      // "pasted": the text is in the agent's input, Enter was not pressed
      // (the box was off, or the Enter was withheld because the agent changed).
      const key =
        res === "sent" ? "diff.sent" : this.submitBox.checked ? "diff.unsent" : "diff.pasted";
      this.notice(fill(t(key), { n: batch.length, target: label }));
      this.hide(false);
      return;
    }
    const key = res === "waiting" ? "diff.waiting" : res === "no-paste" ? "diff.noPaste" : "diff.gone";
    await this.copyWithReason(text, key, label);
  }
}
