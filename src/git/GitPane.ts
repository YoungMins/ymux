// Git pane: ygit's log graph, branch list and checkout, plus the worktree
// list / add / remove that had no GUI (spec §4). A layout pane
// (`PaneKind::Git`) with no PTY, implementing `Pane` the way FilesPane and
// EditorPane do, so SplitContainer and PaneGroup need nothing new.
//
// Decisions live in pure modules: lanes in `graphLanes.ts`, branch items,
// checkout / removal plans and chips in `gitModel.ts`, keys in `keys.ts`.
// Every git call is one of `gitApi`'s fixed, argv-built commands.
//
// Hosting rules it shares with FilesPane:
//  - The log is virtualised (a fixed row pool drawn by arithmetic), so a
//    2,000-commit history is one array, not 2,000 nodes.
//  - Its scroll offset is pane state: re-parenting resets `scrollTop`
//    behind our back (rule 14), and `scheduleFit()` puts it back.
//  - Nothing loads while the pane is off screen. A followed `cd` only marks
//    it stale; it loads when shown.

import type { UnlistenFn } from "@tauri-apps/api/event";
import type { Pane } from "../layout/Pane";
import type { Uuid } from "../types";
import {
  api,
  errorKind,
  gitApi,
  onPaneCwd,
  type BranchList,
  type CommitInfo,
  type WorktreeEntry,
} from "../ipc/bridge";
import { t, onLangChange } from "../i18n/i18n";
import { IS_MAC } from "../platform";
import { askChoice } from "../ui/Dialog";
import { CwdFollow } from "../filedock/cwdFollow";
import { baseName } from "../files/fileModel";
import { assignLanes, type LaneLayout, type LaneRow } from "./graphLanes";
import {
  branchItems,
  checkoutPlan,
  checkoutRisk,
  formatCommitDate,
  refChips,
  type BranchItem,
} from "./gitModel";
import { gitKeyAction, type GitKeyAction } from "./keys";
import {
  changeLines,
  commitLines,
  fill,
  gitErrorText,
  promptWorktreeBranch,
  removeWorktreeFlow,
  showGitError,
} from "./worktreeFlow";

export interface GitPaneOptions {
  id: Uuid;
  /// Directory to show first (the spec's `cwd`). `null` waits for the
  /// followed pane to report one.
  dir: string | null;
  title?: string | null;
  ownChrome?: boolean;
  onFocus?: () => void;
  /// The repository shown changed: its root (or the plain directory when it
  /// is not in one). Persisted as the pane's `cwd`.
  onDirChange?: (dir: string) => void;
  /// The active pane changed (`WorkspaceManager.onActivePaneChange`).
  onActivePaneChange: (cb: () => void) => () => void;
  /// The pane this one should follow: the active pane when it is in the
  /// same workspace and is not this pane, else null.
  followTarget: () => Uuid | null;
  /// `Config.worktree_base_dir` ("" = sibling `.ymux-worktrees`).
  worktreeBaseDir: () => string;
  openTerminal: (dir: string) => void | Promise<void>;
}

type Section = "log" | "branches" | "worktrees";
const SECTIONS: Section[] = ["log", "branches", "worktrees"];

type LoadState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ready" }
  | { kind: "notRepo" }
  | { kind: "error"; message: string };

const ROW_H = 26;
const OVERSCAN = 6;
/// Commits per `git_log` page; the next page loads as the end scrolls in.
const PAGE = 300;
const LANE_W = 12;
/// Lanes drawn before the graph column stops growing (the rest clip).
const MAX_LANES = 16;

const ICON = {
  refresh:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M13 8a5 5 0 1 1-1.5-3.6M13 2.5v3h-3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  follow:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 1.5v3M8 11.5v3M1.5 8h3M11.5 8h3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/><circle cx="8" cy="8" r="2.6" fill="none" stroke="currentColor" stroke-width="1.3"/></svg>',
  pin: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6 2h4l-.6 4 2.6 2.2v1.3H4v-1.3L6.6 6z" fill="currentColor" fill-opacity=".25" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round"/><path d="M8 9.5V14" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>',
  worktree:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="4.5" cy="3.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.2"/><circle cx="4.5" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.2"/><path d="M4.5 5.1v5.8M4.5 8.5c0-2 7-1.5 7-4.5" fill="none" stroke="currentColor" stroke-width="1.2"/><path d="M11.5 9.5v5M9 12h5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>',
  terminal:
    '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="1.5" y="2.5" width="13" height="11" rx="1" fill="none" stroke="currentColor" stroke-width="1.1"/><path d="M4 6l2 2-2 2M7.5 10.5H11" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
};

interface RowEls {
  row: HTMLElement;
  graph: HTMLElement;
  hash: HTMLElement;
  text: HTMLElement;
  author: HTMLElement;
  date: HTMLElement;
}

/// One row's graph as SVG markup. Built from numbers only — no commit text
/// ever reaches `innerHTML`.
function graphSvg(row: LaneRow, lanes: number, isHead: boolean): string {
  const w = lanes * LANE_W;
  const x = (lane: number) => lane * LANE_W + LANE_W / 2;
  const mid = ROW_H / 2;
  const seg = (from: number, to: number, y0: number, y1: number, color: number) => {
    const d =
      from === to
        ? `M${x(from)} ${y0}V${y1}`
        : `M${x(from)} ${y0}C${x(from)} ${(y0 + y1) / 2} ${x(to)} ${(y0 + y1) / 2} ${x(to)} ${y1}`;
    return `<path class="git-lane git-lane--c${color}" d="${d}"/>`;
  };
  let s = `<svg width="${w}" height="${ROW_H}" viewBox="0 0 ${w} ${ROW_H}" aria-hidden="true">`;
  for (const e of row.up) s += seg(e.from, e.to, 0, mid, e.color);
  for (const e of row.down) s += seg(e.from, e.to, mid, ROW_H, e.color);
  const cls = `git-node git-node--c${row.color}${isHead ? " git-node--head" : ""}`;
  s += `<circle class="${cls}" cx="${x(row.lane)}" cy="${mid}" r="${isHead ? 4.2 : 3.4}"/>`;
  return `${s}</svg>`;
}

export class GitPane implements Pane {
  readonly id: Uuid;
  readonly element: HTMLElement;

  private titleEl: HTMLElement | null = null;
  private readonly headEl: HTMLElement;
  private readonly repoEl: HTMLElement;
  private readonly pinBtn: HTMLButtonElement;
  private readonly logEl: HTMLElement;
  private readonly spacer: HTMLElement;
  private readonly overlay: HTMLElement;
  private readonly branchesEl: HTMLElement;
  private readonly worktreesEl: HTMLElement;
  private readonly statusCount: HTMLElement;
  private readonly statusMsg: HTMLElement;
  private readonly buttons: { el: HTMLButtonElement; key: string }[] = [];

  private dir: string | null;
  private root: string | null = null;
  /// The last directory handed to `onDirChange` (the spec's `cwd`).
  private persisted: string | null;
  private title: string | null;
  private state: LoadState = { kind: "idle" };
  private commits: CommitInfo[] = [];
  private layout: LaneLayout = { rows: [], width: 0 };
  private hasMore = false;
  private loadingMore = false;
  private branches: BranchList = { current: "", local: [], remote: [], held: {} };
  private items: BranchItem[] = [];
  private remotes = new Set<string>();
  private worktrees: WorktreeEntry[] = [];
  private section: Section = "log";
  private cursor: Record<Section, number> = { log: 0, branches: 0, worktrees: 0 };
  private pinned = false;
  private stale = true;
  private loadGen = 0;
  private scrollTop = 0;
  private rowPool: RowEls[] = [];
  private busy = false;
  private statusTimer: number | null = null;
  private disposed = false;
  private readonly cleanups: (() => void)[] = [];

  private readonly follow = new CwdFollow((dir) => {
    if (!this.pinned) this.navigate(dir);
  });
  private cwdUnlisten: UnlistenFn | null = null;
  private followGen = 0;

  constructor(private readonly opts: GitPaneOptions) {
    this.id = opts.id;
    this.dir = opts.dir;
    this.persisted = opts.dir;
    this.title = opts.title ?? null;

    this.element = document.createElement("div");
    this.element.className = "pane git-pane";
    this.element.dataset.paneId = this.id;
    this.element.tabIndex = -1;

    // ── Toolbar: branch chip, repository │ follow, refresh, terminal, new worktree
    const bar = document.createElement("div");
    bar.className = "files__bar git__bar";
    this.headEl = document.createElement("span");
    this.headEl.className = "git__head";
    this.repoEl = document.createElement("span");
    this.repoEl.className = "git__repo";
    const where = document.createElement("div");
    where.className = "git__where";
    where.append(this.headEl, this.repoEl);
    this.pinBtn = this.makeButton(ICON.follow, "git.follow", () => this.togglePin());
    const refresh = this.makeButton(ICON.refresh, "git.refresh", () => void this.load());
    const term = this.makeButton(ICON.terminal, "git.openTerminal", () => this.openTerminal());
    const add = this.makeButton(ICON.worktree, "git.newWorktree", () => void this.addWorktree());
    const sep = document.createElement("span");
    sep.className = "files__bar-sep";
    bar.append(where, sep, this.pinBtn, refresh, term, add);

    // ── Body: the log beside (or above) branches and worktrees
    const main = document.createElement("div");
    main.className = "files__main git__main";

    this.logEl = document.createElement("div");
    this.logEl.className = "git__log";
    this.logEl.tabIndex = 0;
    this.logEl.setAttribute("role", "listbox");
    this.spacer = document.createElement("div");
    this.spacer.className = "files__spacer";
    this.logEl.appendChild(this.spacer);
    this.overlay = document.createElement("div");
    this.overlay.className = "files__overlay";
    this.overlay.hidden = true;
    this.logEl.appendChild(this.overlay);

    const side = document.createElement("div");
    side.className = "git__side";
    this.branchesEl = document.createElement("div");
    this.branchesEl.className = "git__list git__branches";
    this.branchesEl.tabIndex = 0;
    this.branchesEl.setAttribute("role", "listbox");
    this.worktreesEl = document.createElement("div");
    this.worktreesEl.className = "git__list git__worktrees";
    this.worktreesEl.tabIndex = 0;
    this.worktreesEl.setAttribute("role", "listbox");
    side.append(this.branchesEl, this.worktreesEl);
    main.append(this.logEl, side);

    // ── Status line
    const status = document.createElement("div");
    status.className = "files__status";
    this.statusCount = document.createElement("span");
    this.statusCount.className = "files__status-count";
    this.statusMsg = document.createElement("span");
    this.statusMsg.className = "files__status-msg";
    this.statusMsg.setAttribute("aria-live", "polite");
    status.append(this.statusCount, this.statusMsg);

    this.element.append(bar, main, status);
    if (opts.ownChrome !== false) this.buildTitle();

    this.wire();
    this.follow.reset(this.dir);

    const onWinFocus = () => {
      if (this.isShown() && !this.busy && this.root) void this.load({ quiet: true });
    };
    window.addEventListener("focus", onWinFocus);
    this.cleanups.push(() => window.removeEventListener("focus", onWinFocus));
    const ro = new ResizeObserver(() => this.onResize());
    ro.observe(this.element);
    this.cleanups.push(() => ro.disconnect());
    this.cleanups.push(onLangChange(() => this.updateLang()));
    this.cleanups.push(opts.onActivePaneChange(() => void this.followActivePane()));
    this.updateLang();
  }

  // ── Pane interface ────────────────────────────────────────────────────────

  focus(): void {
    this.sectionEl(this.section).focus({ preventScroll: true });
  }

  scheduleFit(): void {
    requestAnimationFrame(() => this.onShown());
  }

  async spawn(): Promise<void> {
    this.stale = true;
    this.onShown();
    void this.followActivePane();
  }

  dispose(): void {
    this.disposed = true;
    this.loadGen++;
    this.followGen++;
    this.cwdUnlisten?.();
    if (this.statusTimer !== null) clearTimeout(this.statusTimer);
    for (const c of this.cleanups) c();
    this.element.remove();
  }

  // ── Host API ──────────────────────────────────────────────────────────────

  /// Show the repository containing `dir`. Loaded now if on screen.
  navigate(dir: string): void {
    this.dir = dir;
    this.stale = true;
    if (this.isShown()) void this.load();
  }

  setTitle(title: string | null): void {
    this.title = title && title.trim() ? title : null;
    this.updateTitle();
  }

  setOwnChrome(enabled: boolean): void {
    this.element.classList.toggle("pane--tab", !enabled);
    if (enabled && !this.titleEl) this.buildTitle();
    if (!enabled && this.titleEl) {
      this.titleEl.remove();
      this.titleEl = null;
    }
  }

  /// The tab label: the repository's folder name.
  label(): string {
    if (this.title) return this.title;
    const shown = this.root ?? this.dir;
    return shown ? baseName(shown) : t("git.title");
  }

  // ── Following the active pane ─────────────────────────────────────────────

  /// FileDock's pattern: re-point the cwd subscription at the pane to
  /// follow, with a generation counter so a slow, superseded subscription
  /// unlistens itself.
  private async followActivePane(): Promise<void> {
    const gen = ++this.followGen;
    this.cwdUnlisten?.();
    this.cwdUnlisten = null;
    const id = this.opts.followTarget();
    if (!id) {
      this.follow.activePaneChanged(null, null);
      return;
    }
    const unlisten = await onPaneCwd(id, (cwd) => this.follow.cwdChanged(id, cwd)).catch(
      () => null,
    );
    const cwd = await api.getPaneCwd(id).catch(() => null);
    if (gen !== this.followGen || this.disposed) {
      unlisten?.();
      return;
    }
    this.cwdUnlisten = unlisten;
    this.follow.activePaneChanged(id, cwd);
  }

  private togglePin(): void {
    this.pinned = !this.pinned;
    this.renderPin();
    if (!this.pinned) {
      // Catch up with wherever the followed pane is now.
      this.follow.reset(null);
      void this.followActivePane();
    }
    this.say(t(this.pinned ? "git.pinnedMsg" : "git.followingMsg"));
  }

  private renderPin(): void {
    this.pinBtn.innerHTML = this.pinned ? ICON.pin : ICON.follow;
    this.pinBtn.setAttribute("aria-pressed", String(this.pinned));
    this.pinBtn.title = t(this.pinned ? "git.pinned" : "git.follow");
  }

  // ── Loading ───────────────────────────────────────────────────────────────

  private isShown(): boolean {
    return this.element.isConnected && this.element.getClientRects().length > 0;
  }

  private onShown(): void {
    if (this.disposed || !this.isShown()) return;
    this.onResize();
    if (this.stale) {
      void this.load();
      return;
    }
    if (this.logEl.scrollTop !== this.scrollTop) this.logEl.scrollTop = this.scrollTop;
    this.renderRows();
  }

  private onResize(): void {
    const w = this.element.clientWidth;
    if (w === 0) return;
    this.element.classList.toggle("git-pane--narrow", w < 620);
    this.element.classList.toggle("git-pane--tiny", w < 400);
    if (this.stale && this.dir && !this.disposed && this.state.kind !== "loading") {
      void this.load();
      return;
    }
    this.renderRows();
  }

  /// Everything the pane shows, for `this.dir`. A newer call wins (each
  /// takes a generation), so a slow repository can never paint over the one
  /// the user moved on to.
  private async load(opts: { quiet?: boolean } = {}): Promise<void> {
    const dir = this.dir;
    if (!dir || this.disposed) {
      this.state = { kind: "idle" };
      this.render();
      return;
    }
    const gen = ++this.loadGen;
    this.stale = false;
    // A repository already on screen stays there until the answer lands:
    // following a `cd` must not flash "Reading…" over the log.
    if (!opts.quiet && this.state.kind !== "ready") {
      this.state = { kind: "loading" };
      this.renderOverlay();
    }
    let root: string;
    try {
      root = await gitApi.repoRoot(dir);
    } catch (e) {
      if (gen !== this.loadGen || this.disposed) return;
      this.setRoot(null, dir);
      this.state =
        errorKind(e) === "not_a_repo" ? { kind: "notRepo" } : { kind: "error", message: gitErrorText(e) };
      this.commits = [];
      this.layout = { rows: [], width: 0 };
      this.items = [];
      this.worktrees = [];
      this.render();
      return;
    }
    try {
      const [commits, branches, worktrees] = await Promise.all([
        gitApi.log(root, PAGE, 0),
        gitApi.branches(root),
        gitApi.worktrees(root),
      ]);
      if (gen !== this.loadGen || this.disposed) return;
      // Same producer (git), so this is only "reset the cursors or not";
      // no path decision rides on it (rule 15).
      const sameRepo = this.root === root;
      this.setRoot(root, dir);
      this.commits = commits;
      this.hasMore = commits.length === PAGE;
      this.layout = assignLanes(commits);
      this.branches = branches;
      this.items = branchItems(branches);
      this.remotes = new Set(branches.remote);
      this.worktrees = worktrees;
      if (!sameRepo) {
        this.cursor = { log: 0, branches: Math.max(0, this.items.findIndex((i) => i.current)), worktrees: 0 };
        this.scrollTop = 0;
        this.logEl.scrollTop = 0;
      }
      this.clampCursors();
      this.state = { kind: "ready" };
      this.render();
    } catch (e) {
      if (gen !== this.loadGen || this.disposed) return;
      this.state = { kind: "error", message: gitErrorText(e) };
      this.render();
    }
  }

  private setRoot(root: string | null, dir: string): void {
    this.root = root;
    const keep = root ?? dir;
    if (keep !== this.persisted) {
      this.persisted = keep;
      this.opts.onDirChange?.(keep);
    }
    this.updateTitle();
  }

  private async loadMore(): Promise<void> {
    if (!this.root || !this.hasMore || this.loadingMore) return;
    this.loadingMore = true;
    const gen = this.loadGen;
    try {
      const next = await gitApi.log(this.root, PAGE, this.commits.length);
      if (gen !== this.loadGen || this.disposed) return;
      this.commits = this.commits.concat(next);
      this.hasMore = next.length === PAGE;
      // Lanes over every loaded page: a lane left open at the end of the
      // last page continues into this one.
      this.layout = assignLanes(this.commits);
      this.renderRows();
      this.renderStatus();
    } catch (e) {
      this.say(gitErrorText(e), true);
    } finally {
      this.loadingMore = false;
    }
  }

  private clampCursors(): void {
    const max: Record<Section, number> = {
      log: this.commits.length,
      branches: this.items.length,
      worktrees: this.worktrees.length,
    };
    for (const s of SECTIONS) this.cursor[s] = Math.max(0, Math.min(this.cursor[s], max[s] - 1));
  }

  // ── Rendering ─────────────────────────────────────────────────────────────

  private render(): void {
    this.renderHead();
    this.renderOverlay();
    this.renderRows();
    this.renderBranches();
    this.renderWorktrees();
    this.renderStatus();
    this.updateTitle();
  }

  private renderHead(): void {
    this.headEl.replaceChildren();
    this.headEl.classList.remove("git__head--detached");
    if (this.state.kind !== "ready") {
      this.headEl.hidden = true;
    } else {
      this.headEl.hidden = false;
      const cur = this.worktrees.find((w) => w.current);
      if (this.branches.current) {
        this.headEl.textContent = this.branches.current;
      } else if (cur?.detached) {
        this.headEl.classList.add("git__head--detached");
        this.headEl.textContent = fill(t("git.detachedAt"), { hash: (cur?.head ?? "").slice(0, 7) });
      } else {
        this.headEl.textContent = t("git.noBranch");
      }
    }
    const shown = this.root ?? this.dir ?? "";
    this.repoEl.textContent = shown ? baseName(shown) : t("git.title");
    this.repoEl.title = shown;
  }

  private renderOverlay(): void {
    this.overlay.replaceChildren();
    this.overlay.classList.remove("files__overlay--error");
    const s = this.state;
    const p = (text: string, cls?: string) => {
      const el = document.createElement("p");
      if (cls) el.className = cls;
      el.textContent = text;
      return el;
    };
    if (s.kind === "ready" && this.commits.length > 0) {
      this.overlay.hidden = true;
      return;
    }
    this.overlay.hidden = false;
    switch (s.kind) {
      case "idle":
        this.overlay.append(p(t("git.noDir")));
        return;
      case "loading":
        this.overlay.append(p(t("git.loading")));
        return;
      case "notRepo":
        this.overlay.append(p(t("git.notRepo"), "files__overlay-title"), p(this.dir ?? ""), p(t("git.noDir")));
        return;
      case "error":
        this.overlay.classList.add("files__overlay--error");
        this.overlay.append(p(t("git.loadFailed"), "files__overlay-title"), p(s.message));
        return;
      case "ready":
        this.overlay.append(p(t("git.noCommits")));
    }
  }

  private laneCount(): number {
    return Math.max(1, Math.min(this.layout.width, MAX_LANES));
  }

  private renderRows(): void {
    const n = this.state.kind === "ready" ? this.commits.length : 0;
    this.spacer.style.height = `${n * ROW_H}px`;
    const h = this.logEl.clientHeight;
    if (h === 0) return;
    const top = this.logEl.scrollTop;
    const first = Math.max(0, Math.floor(top / ROW_H) - OVERSCAN);
    const last = Math.min(n, Math.ceil((top + h) / ROW_H) + OVERSCAN);
    const need = Math.max(0, last - first);
    while (this.rowPool.length < need) this.rowPool.push(this.makeRow());
    const lanes = this.laneCount();
    const graphW = `${lanes * LANE_W}px`;
    const focused = document.activeElement === this.logEl;
    this.logEl.classList.toggle("git__log--focused", focused);
    for (let k = 0; k < this.rowPool.length; k++) {
      const els = this.rowPool[k];
      const i = first + k;
      if (k >= need) {
        els.row.hidden = true;
        continue;
      }
      const c = this.commits[i];
      const lane = this.layout.rows[i];
      els.row.hidden = false;
      els.row.style.transform = `translateY(${i * ROW_H}px)`;
      els.row.dataset.index = String(i);
      els.row.classList.toggle("git__row--cursor", this.section === "log" && i === this.cursor.log);
      els.row.classList.toggle("git__row--sel", i === this.cursor.log);
      const chips = refChips(c.refs, this.remotes);
      const isHead = chips.some((x) => x.kind === "head" || x.kind === "detached");
      els.graph.style.width = graphW;
      els.graph.innerHTML = lane ? graphSvg(lane, lanes, isHead) : "";
      els.hash.textContent = c.short;
      els.text.replaceChildren();
      for (const chip of chips) {
        const b = document.createElement("span");
        b.className = `git__chip git__chip--${chip.kind}`;
        b.textContent = chip.kind === "detached" ? "HEAD" : chip.name;
        els.text.appendChild(b);
      }
      const subj = document.createElement("span");
      subj.className = "git__subject";
      subj.textContent = c.subject;
      els.text.appendChild(subj);
      els.row.title = `${c.short}  ${c.author}\n${c.subject}`;
      els.author.textContent = c.author;
      els.date.textContent = formatCommitDate(c.date_ms);
    }
    if (this.hasMore && last >= n - OVERSCAN * 3) void this.loadMore();
  }

  private makeRow(): RowEls {
    const row = document.createElement("div");
    row.className = "git__row";
    row.setAttribute("role", "option");
    const graph = document.createElement("span");
    graph.className = "git__graph";
    const hash = document.createElement("span");
    hash.className = "git__hash";
    const text = document.createElement("span");
    text.className = "git__text";
    const author = document.createElement("span");
    author.className = "git__author";
    const date = document.createElement("span");
    date.className = "git__date";
    row.append(graph, hash, text, author, date);
    this.spacer.appendChild(row);
    return { row, graph, hash, text, author, date };
  }

  private renderBranches(): void {
    const el = this.branchesEl;
    el.replaceChildren(this.header("git.branches", this.items.length));
    let lastKind: BranchItem["kind"] | null = null;
    this.items.forEach((item, i) => {
      if (item.kind !== lastKind && item.kind === "remote") {
        const sub = document.createElement("div");
        sub.className = "git__subhead";
        sub.textContent = t("git.remote");
        el.appendChild(sub);
      }
      lastKind = item.kind;
      const row = this.listRow("branches", i);
      if (item.current) row.classList.add("git__item--current");
      const mark = document.createElement("span");
      mark.className = "git__mark";
      mark.textContent = item.current ? "●" : item.heldBy ? "◆" : "";
      const name = document.createElement("span");
      name.className = "git__name";
      name.textContent = item.name;
      row.append(mark, name);
      if (item.heldBy) {
        const tag = document.createElement("span");
        tag.className = "git__tag";
        tag.textContent = t("git.inWorktree");
        row.appendChild(tag);
        row.title = fill(t("git.heldBy"), { path: item.heldBy });
      } else {
        row.title = item.name;
      }
      el.appendChild(row);
    });
    if (this.items.length === 0 && this.state.kind === "ready") el.appendChild(this.empty("git.noBranches"));
  }

  private renderWorktrees(): void {
    const el = this.worktreesEl;
    el.replaceChildren(this.header("git.worktrees", this.worktrees.length));
    this.worktrees.forEach((w, i) => {
      const row = this.listRow("worktrees", i);
      row.classList.add("git__item--two");
      if (w.current) row.classList.add("git__item--current");
      const mark = document.createElement("span");
      mark.className = "git__mark";
      mark.textContent = w.current ? "●" : "";
      const body = document.createElement("span");
      body.className = "git__wt";
      const name = document.createElement("span");
      name.className = "git__name";
      name.textContent = w.branch || fill(t("git.detachedAt"), { hash: w.head.slice(0, 7) });
      const path = document.createElement("span");
      path.className = "git__path";
      path.textContent = w.path;
      body.append(name, path);
      row.append(mark, body);
      for (const [flag, key] of [
        [w.main, "git.wtMain"],
        [w.locked, "git.wtLocked"],
        [w.prunable, "git.wtMissing"],
      ] as const) {
        if (!flag) continue;
        const tag = document.createElement("span");
        tag.className = "git__tag";
        tag.textContent = t(key);
        row.appendChild(tag);
      }
      row.title = w.path;
      el.appendChild(row);
    });
    if (this.worktrees.length === 0 && this.state.kind === "ready") el.appendChild(this.empty("git.noWorktrees"));
  }

  private header(key: string, count: number): HTMLElement {
    const h = document.createElement("div");
    h.className = "git__header";
    const label = document.createElement("span");
    label.textContent = t(key);
    const n = document.createElement("span");
    n.className = "git__count";
    n.textContent = this.state.kind === "ready" ? String(count) : "";
    h.append(label, n);
    return h;
  }

  private empty(key: string): HTMLElement {
    const e = document.createElement("div");
    e.className = "git__empty";
    e.textContent = t(key);
    return e;
  }

  private listRow(section: Section, i: number): HTMLElement {
    const row = document.createElement("div");
    row.className = "git__item";
    row.setAttribute("role", "option");
    row.dataset.index = String(i);
    const sel = this.cursor[section] === i;
    row.classList.toggle("git__item--sel", sel);
    row.classList.toggle("git__item--cursor", sel && this.section === section);
    row.setAttribute("aria-selected", String(sel));
    return row;
  }

  private renderStatus(): void {
    if (this.state.kind !== "ready") {
      this.statusCount.textContent = "";
      return;
    }
    const commits = `${this.commits.length}${this.hasMore ? "+" : ""}`;
    this.statusCount.textContent = fill(t("git.counts"), {
      commits,
      branches: this.branches.local.length,
      worktrees: this.worktrees.length,
    });
  }

  private say(text: string, error = false): void {
    if (this.statusTimer !== null) clearTimeout(this.statusTimer);
    this.statusMsg.textContent = text;
    this.statusMsg.classList.toggle("files__status-msg--error", error);
    if (!error && text) {
      this.statusTimer = window.setTimeout(() => {
        this.statusMsg.textContent = "";
      }, 2500);
    }
  }

  private updateTitle(): void {
    if (this.titleEl) this.titleEl.textContent = this.label();
  }

  private buildTitle(): void {
    this.titleEl = document.createElement("div");
    this.titleEl.className = "pane-title";
    this.element.insertBefore(this.titleEl, this.element.firstChild);
    this.updateTitle();
  }

  private updateLang(): void {
    for (const { el, key } of this.buttons) el.title = t(key);
    this.renderPin();
    this.render();
  }

  private makeButton(icon: string, key: string, onClick: () => void): HTMLButtonElement {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "files__btn";
    b.tabIndex = -1;
    b.innerHTML = icon;
    b.title = t(key);
    b.addEventListener("mousedown", (ev) => ev.preventDefault());
    b.addEventListener("click", () => {
      this.focus();
      onClick();
    });
    this.buttons.push({ el: b, key });
    return b;
  }

  // ── Input ─────────────────────────────────────────────────────────────────

  private sectionEl(s: Section): HTMLElement {
    return s === "log" ? this.logEl : s === "branches" ? this.branchesEl : this.worktreesEl;
  }

  private wire(): void {
    this.element.addEventListener("focusin", (ev) => {
      this.opts.onFocus?.();
      for (const s of SECTIONS) {
        if (ev.target === this.sectionEl(s) && this.section !== s) {
          this.section = s;
          this.renderCursorOnly();
        }
      }
      this.logEl.classList.toggle("git__log--focused", ev.target === this.logEl);
    });
    this.element.addEventListener("focusout", () => {
      requestAnimationFrame(() =>
        this.logEl.classList.toggle("git__log--focused", document.activeElement === this.logEl),
      );
    });
    for (const s of SECTIONS) {
      this.sectionEl(s).addEventListener("keydown", (ev) => this.onKey(ev));
    }
    this.logEl.addEventListener("scroll", () => {
      this.scrollTop = this.logEl.scrollTop;
      this.renderRows();
    });
    this.logEl.addEventListener("mousedown", (ev) => {
      const row = (ev.target as HTMLElement).closest<HTMLElement>(".git__row");
      if (!row?.dataset.index) return;
      this.section = "log";
      this.cursor.log = Number(row.dataset.index);
      this.renderCursorOnly();
    });
    for (const s of ["branches", "worktrees"] as const) {
      const el = this.sectionEl(s);
      el.addEventListener("mousedown", (ev) => {
        const row = (ev.target as HTMLElement).closest<HTMLElement>(".git__item");
        if (!row?.dataset.index) return;
        this.section = s;
        this.cursor[s] = Number(row.dataset.index);
        this.renderCursorOnly();
      });
      el.addEventListener("dblclick", (ev) => {
        if ((ev.target as HTMLElement).closest(".git__item")) void this.run({ kind: "activate" });
      });
    }
  }

  private renderCursorOnly(): void {
    this.renderRows();
    this.renderBranches();
    this.renderWorktrees();
  }

  private onKey(ev: KeyboardEvent): void {
    const action = gitKeyAction(ev, IS_MAC);
    if (!action) return;
    ev.preventDefault();
    this.say("");
    void this.run(action);
  }

  private count(s: Section): number {
    return s === "log" ? this.commits.length : s === "branches" ? this.items.length : this.worktrees.length;
  }

  private moveTo(i: number): void {
    const s = this.section;
    const n = this.count(s);
    if (n === 0) return;
    this.cursor[s] = Math.max(0, Math.min(n - 1, i));
    if (s === "log") {
      const h = this.logEl.clientHeight;
      const top = this.cursor.log * ROW_H;
      if (top < this.logEl.scrollTop) this.logEl.scrollTop = top;
      else if (top + ROW_H > this.logEl.scrollTop + h) this.logEl.scrollTop = top + ROW_H - h;
      this.scrollTop = this.logEl.scrollTop;
      this.renderRows();
    } else {
      this.renderCursorOnly();
      this.sectionEl(s)
        .querySelector(".git__item--sel")
        ?.scrollIntoView({ block: "nearest" });
    }
  }

  private async run(a: GitKeyAction): Promise<void> {
    const s = this.section;
    const page =
      s === "log" ? Math.max(1, Math.floor(this.logEl.clientHeight / ROW_H) - 1) : 10;
    switch (a.kind) {
      case "move":
        return this.moveTo(this.cursor[s] + a.delta);
      case "page":
        return this.moveTo(this.cursor[s] + a.dir * page);
      case "edge":
        return this.moveTo(a.end ? this.count(s) - 1 : 0);
      case "section": {
        const next = SECTIONS[(SECTIONS.indexOf(s) + a.dir + SECTIONS.length) % SECTIONS.length];
        this.section = next;
        this.renderCursorOnly();
        this.sectionEl(next).focus({ preventScroll: true });
        return;
      }
      case "activate":
        if (s === "branches") return this.checkoutSelected();
        if (s === "worktrees") return this.viewSelectedWorktree();
        return;
      case "refresh":
        return this.load();
      case "newWorktree":
        return this.addWorktree();
      case "remove":
        if (s === "worktrees") return this.removeSelectedWorktree();
        return;
      case "terminal":
        return this.openTerminal();
      case "pin":
        return this.togglePin();
      case "copy":
        return this.copySelected();
    }
  }

  // ── Actions ───────────────────────────────────────────────────────────────

  private selectedWorktree(): WorktreeEntry | null {
    return this.worktrees[this.cursor.worktrees] ?? null;
  }

  private openTerminal(): void {
    const w = this.section === "worktrees" ? this.selectedWorktree() : null;
    const dir = w?.path ?? this.root ?? this.dir;
    if (dir) void this.opts.openTerminal(dir);
  }

  private async copySelected(): Promise<void> {
    let text: string | null = null;
    if (this.section === "log") text = this.commits[this.cursor.log]?.hash ?? null;
    else if (this.section === "branches") text = this.items[this.cursor.branches]?.name ?? null;
    else text = this.selectedWorktree()?.path ?? null;
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      this.say(fill(t("git.copied"), { text }));
    } catch {
      /* clipboard refused: nothing to say that helps */
    }
  }

  /// Show another worktree in this pane — and stop following, or the next
  /// focus change would take the user straight back.
  private viewWorktree(path: string): void {
    this.pinned = true;
    this.renderPin();
    this.navigate(path);
  }

  private viewSelectedWorktree(): void {
    const w = this.selectedWorktree();
    if (w && !w.current && !w.prunable) this.viewWorktree(w.path);
  }

  private async checkoutSelected(): Promise<void> {
    const item = this.items[this.cursor.branches];
    const root = this.root;
    if (!item || !root || this.busy) return;
    const plan = checkoutPlan(item, this.branches);
    if (plan.kind === "noop") {
      this.say(fill(t("git.alreadyOn"), { branch: plan.branch }));
      return;
    }
    if (plan.kind === "held") {
      const answer = await askChoice(fill(t("git.heldTitle"), { branch: plan.branch }), plan.path, [
        { id: "cancel", label: t("dialog.cancel") },
        { id: "view", label: t("git.viewWorktree"), primary: true },
      ]);
      if (answer?.id === "view") this.viewWorktree(plan.path);
      return;
    }
    this.busy = true;
    try {
      let status;
      try {
        status = await gitApi.workStatus(root, false);
      } catch (e) {
        await showGitError(t("git.checkoutFailed"), e);
        return;
      }
      const risk = checkoutRisk(status);
      const title =
        plan.kind === "track"
          ? fill(t("git.checkoutTrackTitle"), { branch: plan.branch, remote: plan.remote })
          : fill(t("git.checkoutTitle"), { branch: plan.branch });
      const parts: string[] = [];
      if (plan.kind === "local" && plan.fromRemote) {
        parts.push(fill(t("git.checkoutLocalNotRemote"), { branch: plan.branch, remote: plan.fromRemote }));
      }
      if (risk.stranded.length) {
        parts.push(
          `${fill(t("git.checkoutStranded"), { n: risk.stranded.length })}\n${commitLines(risk.stranded, risk.moreStranded)}`,
        );
      }
      if (risk.carried.length) {
        parts.push(
          `${fill(t("git.checkoutCarried"), { n: risk.carried.length, branch: plan.branch })}\n${changeLines(risk.carried)}`,
        );
      }
      const answer = await askChoice(title, parts.length ? parts.join("\n\n") : null, [
        { id: "cancel", label: t("dialog.cancel"), primary: risk.loses },
        {
          id: "go",
          label: t(risk.loses ? "git.checkoutLeave" : "git.checkoutBtn"),
          primary: !risk.loses,
        },
      ]);
      if (answer?.id !== "go") return;
      try {
        if (plan.kind === "track") await gitApi.checkoutTrack(root, plan.remote);
        else await gitApi.checkout(root, plan.branch);
      } catch (e) {
        await showGitError(fill(t("git.checkoutFailedBranch"), { branch: plan.branch }), e);
        return;
      }
      this.say(fill(t("git.checkedOut"), { branch: plan.branch }));
    } finally {
      this.busy = false;
    }
    await this.load({ quiet: true });
    this.cursor.branches = Math.max(0, this.items.findIndex((i) => i.current));
    this.renderCursorOnly();
  }

  private async addWorktree(): Promise<void> {
    const root = this.root;
    if (!root || this.busy) return;
    const branch = await promptWorktreeBranch("");
    if (!branch) return;
    this.busy = true;
    let path: string;
    try {
      path = await gitApi.worktreeAdd(root, branch, this.opts.worktreeBaseDir());
    } catch (e) {
      await showGitError(t("worktree.addFailed"), e);
      return;
    } finally {
      this.busy = false;
    }
    this.say(fill(t("git.worktreeAdded"), { path }));
    await this.load({ quiet: true });
    const i = this.worktrees.findIndex((w) => w.branch === branch);
    if (i >= 0) {
      this.section = "worktrees";
      this.cursor.worktrees = i;
      this.renderCursorOnly();
      this.worktreesEl.focus({ preventScroll: true });
    }
  }

  private async removeSelectedWorktree(): Promise<void> {
    const w = this.selectedWorktree();
    if (!w || this.busy) return;
    this.busy = true;
    try {
      const outcome = await removeWorktreeFlow(w);
      if (outcome === "removed") this.say(fill(t("git.removed"), { path: w.path }));
    } finally {
      this.busy = false;
    }
    await this.load({ quiet: true });
    this.focus();
  }
}
