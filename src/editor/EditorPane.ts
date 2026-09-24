// Editor pane: a text editor rendered by ymux itself (spec §3), replacing the
// `ycode` TUI. It implements `Pane` the way FilesPane does — no PTY — so
// SplitContainer and PaneGroup host it like any other leaf.
//
// What this file is for is **not losing an edit** (spec risk 1). Three
// mechanisms, each with its decisions in a pure, tested module:
//
//  - **The close guard** (`canClose`, closeGuard.ts). `Pane.dispose()` has no
//    veto, so every path that closes a pane asks this first; only an explicit
//    Discard or a save that actually landed lets it go.
//  - **Stamp-guarded saves.** Every write carries the content stamp the
//    buffer was loaded from; a file changed underneath (an agent rewrote it)
//    comes back `conflict` and the user picks Reload / Overwrite / Save as
//    copy. A save never silently wins.
//  - **External-change polling** (editorModel.ts `pollStep` /
//    `conflictDecision`) on window focus and pane focus: a clean buffer
//    follows the file silently, a dirty one gets a banner.
//
// Plus a local draft of a dirty buffer (draft.ts) as a safety net for what
// none of those can see. There is no autosave to the real file.
//
// CodeMirror itself is behind a dynamic import (`cmSetup.ts`); this module
// only imports it as a type, so the boot bundle does not carry the editor.

import type { Pane } from "../layout/Pane";
import type { Uuid } from "../types";
import type { SyntaxColors } from "../settings/types";
import type { EditorHandle } from "./cmSetup";
import { api, errorKind, fsApi, type ContentStamp } from "../ipc/bridge";
import { t, onLangChange } from "../i18n/i18n";
import { IS_MAC, shortcutLabel } from "../platform";
import { askChoice, askConfirm, askText } from "../ui/Dialog";
import { describeFsError } from "../files/FilesPane";
import {
  conflictDecision,
  copyCandidate,
  fileName,
  isDocDirty,
  languageForPath,
  openFileDecision,
  pollStep,
  type DocLike,
  type LangId,
} from "./editorModel";
import { closeDecision, closeResult, type CloseChoice } from "./closeGuard";
import { eolLabel, needsEolWarning, saveEol, type Eol } from "./eol";
import { draftOffer, encodeDraft, parseDraft, type Draft } from "./draft";

export interface EditorPaneOptions {
  id: Uuid;
  /// The file to open. Empty = untitled: the pane shows its empty state.
  filePath: string;
  title?: string | null;
  ownChrome?: boolean;
  fontSize: number;
  onFocus?: () => void;
  /// The pane now shows another file (viewer-tab reuse, save as copy).
  /// The host persists it as the spec's `file_path`.
  onPathChange?: (path: string) => void;
  /// The dirty state flipped (tab label marker).
  onDirtyChange?: () => void;
}

/// Quiet period before a dirty buffer's draft is written (spec §3.5).
const DRAFT_DELAY_MS = 2000;
/// Pane focus fires on every click inside the pane; the disk poll needs
/// far less than that.
const POLL_MIN_INTERVAL_MS = 750;

type Banner =
  | { kind: "changed"; sha: string }
  | { kind: "deleted" }
  | { kind: "draft"; draft: Draft }
  | { kind: "mixedEol" }
  | { kind: "readOnly" };

interface Loaded {
  eol: Eol;
  bom: boolean;
  stamp: ContentStamp;
  readOnly: boolean;
  lang: LangId | null;
}

let syntaxPromise: Promise<SyntaxColors | null> | null = null;

/// The user's ytheme syntax palette, loaded once per session.
function loadSyntax(): Promise<SyntaxColors | null> {
  if (!syntaxPromise) {
    syntaxPromise = api
      .loadSyntaxTheme()
      .then((th) => th.syntax ?? null)
      .catch(() => null);
  }
  return syntaxPromise;
}

function fill(template: string, vars: Record<string, string | number>): string {
  return template.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m));
}

const LANG_LABEL: Record<LangId, string> = {
  rust: "Rust",
  javascript: "JavaScript",
  jsx: "JSX",
  typescript: "TypeScript",
  tsx: "TSX",
  json: "JSON",
  html: "HTML",
  css: "CSS",
  markdown: "Markdown",
  python: "Python",
  yaml: "YAML",
};

const ICON = {
  save: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 2.5h8.5l2.5 2.5v8.5h-11z" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round"/><path d="M5 2.5v3.5h5.5V2.5M5 13.5V9.5h6v4" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round"/></svg>',
  undo: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M5.5 4 2.5 7l3 3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/><path d="M3 7h6.5a3.5 3.5 0 0 1 0 7H7" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>',
  redo: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M10.5 4l3 3-3 3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/><path d="M13 7H6.5a3.5 3.5 0 0 0 0 7H9" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>',
  find: '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.2" fill="none" stroke="currentColor" stroke-width="1.3"/><path d="M10.2 10.2 13.5 13.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>',
};

export class EditorPane implements Pane {
  readonly id: Uuid;
  readonly element: HTMLElement;

  private titleEl: HTMLElement | null = null;
  private readonly nameEl: HTMLElement;
  private readonly dirEl: HTMLElement;
  private readonly dotEl: HTMLElement;
  private readonly bannerEl: HTMLElement;
  private readonly body: HTMLElement;
  private readonly host: HTMLElement;
  private readonly overlay: HTMLElement;
  private readonly statusMsg: HTMLElement;
  private readonly statusPos: HTMLElement;
  private readonly statusEol: HTMLElement;
  private readonly statusLang: HTMLElement;
  private readonly buttons: { el: HTMLButtonElement; key: string; chord?: string }[] = [];
  private readonly saveBtn: HTMLButtonElement;

  private path: string;
  private title: string | null;
  private fontSize: number;
  private handle: EditorHandle | null = null;
  private handlePromise: Promise<EditorHandle> | null = null;
  private file: Loaded | null = null;
  /// The document as last loaded or saved — what "dirty" is measured against.
  private saved: DocLike | null = null;
  private dirty = false;
  /// The file vanished from disk. The buffer is then the only copy, so the
  /// pane counts as unsaved even if nothing was typed.
  private deletedOnDisk = false;
  /// `changed` banners the user answered "keep mine" to, by disk hash, so
  /// the same change does not re-prompt on every focus.
  private dismissedSha: string | null = null;
  private banner: Banner | null = null;
  /// Non-null while the pane cannot show an editable file.
  private problem: { text: string; retry: boolean } | null = null;
  private loadGen = 0;
  private spawned = false;
  private saving = false;
  private polling = false;
  private lastPoll = 0;
  private eolConfirmed = false;
  private draftTimer: number | null = null;
  /// A draft may exist on disk for this pane (so becoming clean deletes it).
  private draftOnDisk = false;
  private statusTimer: number | null = null;
  private disposed = false;
  private readonly cleanups: (() => void)[] = [];

  constructor(private readonly opts: EditorPaneOptions) {
    this.id = opts.id;
    this.path = opts.filePath;
    this.title = opts.title ?? null;
    this.fontSize = opts.fontSize;

    this.element = document.createElement("div");
    this.element.className = "pane editor-pane";
    this.element.dataset.paneId = this.id;
    this.element.tabIndex = -1;

    // ── Toolbar: save, undo, redo │ file name (dirty dot) │ find
    const bar = document.createElement("div");
    bar.className = "editor__bar";
    this.saveBtn = this.makeButton(ICON.save, "editor.save", () => void this.save(), "Ctrl+S");
    const undo = this.makeButton(ICON.undo, "editor.undo", () => this.handle?.undo(), "Ctrl+Z");
    const redo = this.makeButton(ICON.redo, "editor.redo", () => this.handle?.redo(), "Ctrl+Y");
    const file = document.createElement("div");
    file.className = "editor__file";
    this.dotEl = document.createElement("span");
    this.dotEl.className = "editor__dot";
    this.dotEl.setAttribute("aria-hidden", "true");
    this.nameEl = document.createElement("span");
    this.nameEl.className = "editor__name";
    this.dirEl = document.createElement("span");
    this.dirEl.className = "editor__dir";
    file.append(this.dotEl, this.nameEl, this.dirEl);
    const find = this.makeButton(ICON.find, "editor.find", () => this.toggleSearch(), "Ctrl+F");
    const sep = () => {
      const s = document.createElement("span");
      s.className = "files__bar-sep";
      return s;
    };
    bar.append(this.saveBtn, undo, redo, sep(), file, sep(), find);

    this.bannerEl = document.createElement("div");
    this.bannerEl.className = "editor__banner";
    this.bannerEl.hidden = true;
    this.bannerEl.setAttribute("role", "status");

    this.body = document.createElement("div");
    this.body.className = "editor__body";
    this.host = document.createElement("div");
    this.host.className = "editor__host";
    this.overlay = document.createElement("div");
    this.overlay.className = "editor__overlay";
    this.body.append(this.host, this.overlay);

    const status = document.createElement("div");
    status.className = "editor__status";
    this.statusMsg = document.createElement("span");
    this.statusMsg.className = "editor__status-msg";
    this.statusMsg.setAttribute("aria-live", "polite");
    const facts = document.createElement("span");
    facts.className = "editor__status-facts";
    this.statusPos = document.createElement("span");
    this.statusEol = document.createElement("span");
    this.statusLang = document.createElement("span");
    facts.append(this.statusPos, this.statusEol, this.statusLang);
    status.append(this.statusMsg, facts);

    this.element.append(bar, this.bannerEl, this.body, status);
    if (opts.ownChrome !== false) this.buildTitle();

    this.element.addEventListener("focusin", () => {
      this.opts.onFocus?.();
      void this.checkDisk();
    });
    const onWinFocus = () => void this.checkDisk(true);
    window.addEventListener("focus", onWinFocus);
    this.cleanups.push(() => window.removeEventListener("focus", onWinFocus));
    this.cleanups.push(onLangChange(() => this.updateLang()));
    this.updateLang();
    this.renderChrome();
  }

  // ── Pane interface ────────────────────────────────────────────────────────

  /// The manager calls this on every pointerdown inside the pane. While the
  /// editor already has focus it must do nothing: re-focusing would fight
  /// CodeMirror's own selection handling (and an IME composition).
  focus(): void {
    if (this.handle) {
      if (!this.handle.hasFocus()) this.handle.focus();
      return;
    }
    const btn = this.overlay.querySelector<HTMLElement>("button");
    (btn ?? this.element).focus({ preventScroll: true });
  }

  /// Rule 14: a re-parent or un-hide resets `.cm-scroller`'s scrollTop and
  /// may have resized the box. One frame later: re-measure and put the
  /// scroll position back.
  scheduleFit(): void {
    requestAnimationFrame(() => this.handle?.restoreScroll());
  }

  async spawn(): Promise<void> {
    if (this.spawned) return;
    this.spawned = true;
    if (this.path) await this.load(this.path, { offerDraft: true });
    else this.renderChrome();
  }

  dispose(permanent = false): void {
    this.disposed = true;
    this.loadGen++;
    if (this.statusTimer !== null) clearTimeout(this.statusTimer);
    if (this.draftTimer !== null) {
      clearTimeout(this.draftTimer);
      this.draftTimer = null;
      // Not permanent (a shutdown path): the pending draft is the whole
      // point of drafts, so write it now rather than lose the last 2 s.
      if (!permanent && this.isDirty()) this.writeDraft();
    }
    // Permanent: the user closed this pane after the guard let it go —
    // there is nothing left to recover into.
    if (permanent) void api.deleteEditorDraft(this.id).catch(() => {});
    for (const c of this.cleanups) c();
    this.handle?.destroy();
    this.handle = null;
    this.element.remove();
  }

  // ── Host API ──────────────────────────────────────────────────────────────

  /// Unsaved work that closing would lose.
  isDirty(): boolean {
    if (!this.file || this.file.readOnly) return false;
    return this.dirty || this.deletedOnDisk;
  }

  hasPath(): boolean {
    return this.path !== "";
  }

  currentPath(): string {
    return this.path;
  }

  displayName(): string {
    return this.path ? fileName(this.path) : t("editor.untitled");
  }

  /// The close guard (spec §3.5). Resolves false to cancel the close.
  async canClose(): Promise<boolean> {
    const choices = closeDecision(this.isDirty(), this.hasPath());
    if (!choices) return true;
    const answer = await this.askClose(choices);
    if (answer === "save") return closeResult(answer, [await this.save()]);
    return closeResult(answer);
  }

  /// The buffer was explicitly discarded by a multi-pane close prompt (the
  /// window): drop the draft so it is not offered back next launch.
  async discardDraft(): Promise<void> {
    if (this.draftTimer !== null) clearTimeout(this.draftTimer);
    this.draftTimer = null;
    this.draftOnDisk = false;
    await api.deleteEditorDraft(this.id).catch(() => {});
  }

  /// Show `path` in this pane (the viewer tab's reuse, spec §3.7). Asks
  /// before replacing a dirty buffer; resolves false if the user kept it.
  async openFile(path: string): Promise<boolean> {
    const decision = openFileDecision(this.path || null, this.isDirty(), path);
    if (decision === "focus" && this.file) {
      this.focus();
      return true;
    }
    if (decision === "ask") {
      const choices = closeDecision(true, this.hasPath());
      const answer = choices ? await this.askClose(choices) : "discard";
      const ok =
        answer === "save" ? closeResult(answer, [await this.save()]) : closeResult(answer);
      if (!ok) return false;
    }
    await this.discardDraft();
    this.path = path;
    this.opts.onPathChange?.(path);
    await this.load(path, { offerDraft: false });
    this.focus();
    return true;
  }

  /// Ctrl+F (main.ts routes it here): CodeMirror's search panel.
  toggleSearch(): void {
    if (!this.handle || this.problem) return;
    this.handle.openSearch();
  }

  setFontSize(px: number): void {
    this.fontSize = px;
    this.handle?.setFontSize(px);
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

  // ── Save ──────────────────────────────────────────────────────────────────

  /// Write the buffer. Resolves true only when the bytes are on disk.
  async save(): Promise<boolean> {
    const h = this.handle;
    const f = this.file;
    if (!h || !f || !this.path || f.readOnly || this.saving) return false;
    if (needsEolWarning(f.eol) && !this.eolConfirmed) {
      const ok = await askConfirm(t("editor.mixedEolConfirm"), t("editor.convertAndSave"));
      if (!ok) return false;
      this.eolConfirmed = true;
    }
    this.saving = true;
    try {
      // A file deleted under the buffer: the stamp can never match again,
      // so if it is still gone, recreate it outright. If something put it
      // back meanwhile, the stamped write below reports the conflict.
      let expect: ContentStamp | null = f.stamp;
      if (this.deletedOnDisk) {
        const gone = await fsApi.stat(this.path).then(
          () => false,
          (e) => errorKind(e) === "not_found",
        );
        if (gone) expect = null;
      }
      return await this.write(this.path, expect);
    } finally {
      this.saving = false;
    }
  }

  /// One write attempt of the current buffer to `path`, guarded by
  /// `expect`. On `conflict`, asks and follows the answer.
  private async write(path: string, expect: ContentStamp | null): Promise<boolean> {
    const h = this.handle;
    const f = this.file;
    if (!h || !f) return false;
    // Snapshot before the await: typing during the write must stay dirty.
    const doc = h.doc();
    const eol = saveEol(f.eol);
    let stamp: ContentStamp;
    try {
      stamp = await fsApi.writeText({ path, text: doc.toString(), eol, bom: f.bom, expect });
    } catch (e) {
      if (errorKind(e) === "conflict") return this.resolveConflict();
      this.say(`${fileName(path)}: ${describeFsError(e)}`, true);
      return false;
    }
    if (this.disposed) return true;
    if (path !== this.path) {
      this.path = path;
      this.opts.onPathChange?.(path);
    }
    this.file = { ...f, eol, stamp };
    this.saved = doc;
    this.deletedOnDisk = false;
    this.dismissedSha = null;
    if (this.banner && this.banner.kind !== "readOnly") this.setBanner(null);
    this.refreshDirty();
    // Saved: the draft has nothing left to protect. (Typing during the
    // write left the buffer dirty; its draft stays and is rescheduled.)
    if (!this.dirty) void this.discardDraft();
    this.say(t("editor.saved"));
    return true;
  }

  /// The file changed on disk since the buffer was loaded (spec §3.6).
  private async resolveConflict(): Promise<boolean> {
    const name = this.displayName();
    const ans = await askChoice(
      fill(t("editor.conflict"), { name }),
      t("editor.conflictDetail"),
      [
        { id: "reload", label: t("editor.reload") },
        { id: "overwrite", label: t("editor.overwrite") },
        { id: "copy", label: t("editor.saveCopy"), primary: true },
        { id: "cancel", label: t("dialog.cancel") },
      ],
    );
    switch (ans?.id) {
      case "reload":
        await this.reloadFromDisk();
        // The edits were discarded on request; nothing was saved.
        return false;
      case "overwrite":
        return this.write(this.path, null);
      case "copy":
        return this.saveCopy();
      default:
        return false;
    }
  }

  /// Write the buffer beside the original under a free "(copy)" name, and
  /// carry on editing the copy.
  private async saveCopy(): Promise<boolean> {
    for (let n = 1; n <= 50; n++) {
      const candidate = copyCandidate(this.path, n);
      try {
        await fsApi.stat(candidate);
        continue; // taken
      } catch (e) {
        if (errorKind(e) !== "not_found") {
          this.say(describeFsError(e), true);
          return false;
        }
      }
      const ok = await this.write(candidate, null);
      if (ok) this.say(fill(t("editor.savedCopy"), { name: fileName(candidate) }));
      return ok;
    }
    return false;
  }

  // ── Load / reload ─────────────────────────────────────────────────────────

  private async ensureHandle(): Promise<EditorHandle> {
    if (this.handle) return this.handle;
    if (!this.handlePromise) {
      this.handlePromise = (async () => {
        const [{ createEditor }, syntax] = await Promise.all([import("./cmSetup"), loadSyntax()]);
        const handle = createEditor(this.host, {
          isMac: IS_MAC,
          fontSize: this.fontSize,
          syntax,
          onDocChange: () => this.onDocChange(),
          onSelection: () => this.renderPosition(),
          onSave: () => void this.save(),
        });
        this.handle = handle;
        return handle;
      })();
    }
    return this.handlePromise;
  }

  private async load(path: string, o: { offerDraft: boolean }): Promise<void> {
    const gen = ++this.loadGen;
    this.problem = null;
    this.setBanner(null);
    this.renderChrome();
    this.say(t("editor.loading"));
    let tf;
    try {
      tf = await fsApi.readText(path);
    } catch (e) {
      if (gen !== this.loadGen || this.disposed) return;
      this.file = null;
      this.saved = null;
      this.dirty = false;
      const kind = errorKind(e);
      this.problem = {
        text: kind === "not_utf8" ? t("editor.notText") : describeFsError(e),
        retry: kind !== "not_utf8",
      };
      this.say("");
      this.renderChrome();
      this.opts.onDirtyChange?.();
      return;
    }
    let h: EditorHandle;
    try {
      h = await this.ensureHandle();
    } catch (e) {
      if (gen !== this.loadGen || this.disposed) return;
      this.problem = { text: String(e), retry: true };
      this.renderChrome();
      return;
    }
    if (gen !== this.loadGen || this.disposed) return;
    const lang = languageForPath(path);
    this.file = { eol: tf.eol, bom: tf.bom, stamp: tf.stamp, readOnly: tf.truncated, lang };
    this.eolConfirmed = false;
    this.deletedOnDisk = false;
    this.dismissedSha = null;
    h.load(tf.text, { readOnly: tf.truncated, lang });
    this.saved = h.doc();
    this.dirty = false;
    this.say("");
    if (tf.truncated) this.setBanner({ kind: "readOnly" });
    else if (needsEolWarning(tf.eol)) this.setBanner({ kind: "mixedEol" });
    this.renderChrome();
    this.opts.onDirtyChange?.();
    if (o.offerDraft && !tf.truncated) await this.offerDraft(path, tf.text, gen);
  }

  /// Re-read the file, dropping the buffer. Keeps the cursor line.
  private async reloadFromDisk(): Promise<void> {
    const h = this.handle;
    if (!h || !this.path) return;
    let tf;
    try {
      tf = await fsApi.readText(this.path);
    } catch (e) {
      this.say(describeFsError(e), true);
      return;
    }
    if (this.disposed) return;
    this.applyReload(tf.text, tf.eol, tf.bom, tf.stamp, tf.truncated);
  }

  private applyReload(text: string, eol: Eol, bom: boolean, stamp: ContentStamp, truncated: boolean): void {
    const h = this.handle;
    if (!h || !this.file) return;
    const cursor = h.cursor();
    h.load(text, { readOnly: truncated, lang: this.file.lang, cursor });
    this.file = { ...this.file, eol, bom, stamp, readOnly: truncated };
    this.saved = h.doc();
    this.dirty = false;
    this.deletedOnDisk = false;
    this.dismissedSha = null;
    this.setBanner(truncated ? { kind: "readOnly" } : null);
    void this.discardDraft();
    this.renderChrome();
    this.opts.onDirtyChange?.();
  }

  // ── External changes (spec §3.6) ──────────────────────────────────────────

  /// The focus poll. Cheap when nothing changed: one stat.
  private async checkDisk(force = false): Promise<void> {
    const f = this.file;
    if (!f || !this.path || !this.handle || this.polling || this.saving || this.disposed) return;
    const now = Date.now();
    if (!force && now - this.lastPoll < POLL_MIN_INTERVAL_MS) return;
    this.lastPoll = now;
    this.polling = true;
    try {
      let stat: { modified_ms: number } | null;
      try {
        stat = await fsApi.stat(this.path);
      } catch (e) {
        if (errorKind(e) !== "not_found") return;
        stat = null;
      }
      const step = pollStep(stat, f.stamp.modified_ms);
      if (step === "unchanged" && !this.deletedOnDisk) return;
      if (step === "missing") {
        if (!this.deletedOnDisk) {
          this.deletedOnDisk = true;
          this.setBanner({ kind: "deleted" });
          this.renderChrome();
          this.opts.onDirtyChange?.();
        }
        return;
      }
      let tf;
      try {
        tf = await fsApi.readText(this.path);
      } catch {
        return; // became unreadable (binary, huge): leave the buffer alone
      }
      if (this.disposed || this.file !== f || this.saving) return;
      const decision = conflictDecision(tf.stamp.sha256, f.stamp.sha256, this.dirty);
      if (decision === "none") {
        // Same bytes (a touch, or the file came back): adopt the new mtime.
        this.file = { ...f, stamp: tf.stamp };
        if (this.deletedOnDisk) {
          this.deletedOnDisk = false;
          this.setBanner(null);
          this.renderChrome();
          this.opts.onDirtyChange?.();
        }
      } else if (decision === "reload") {
        this.applyReload(tf.text, tf.eol, tf.bom, tf.stamp, tf.truncated);
        this.say(fill(t("editor.reloaded"), { name: this.displayName() }));
      } else if (decision === "prompt" && tf.stamp.sha256 !== this.dismissedSha) {
        this.setBanner({ kind: "changed", sha: tf.stamp.sha256 });
      }
    } finally {
      this.polling = false;
    }
  }

  // ── Dirty tracking and drafts ─────────────────────────────────────────────

  private onDocChange(): void {
    this.refreshDirty();
    if (this.dirty) this.scheduleDraft();
    else if (this.draftOnDisk || this.draftTimer !== null) void this.discardDraft();
  }

  private refreshDirty(): void {
    const h = this.handle;
    const was = this.dirty;
    this.dirty = !!(h && this.saved && isDocDirty(this.saved, h.doc()));
    if (was !== this.dirty) {
      this.renderChrome();
      this.opts.onDirtyChange?.();
    }
  }

  private scheduleDraft(): void {
    if (this.draftTimer !== null) clearTimeout(this.draftTimer);
    this.draftTimer = window.setTimeout(() => {
      this.draftTimer = null;
      if (this.isDirty()) this.writeDraft();
    }, DRAFT_DELAY_MS);
  }

  private writeDraft(): void {
    const h = this.handle;
    const f = this.file;
    if (!h || !f || !this.path) return;
    const draft: Draft = {
      v: 1,
      path: this.path,
      text: h.text(),
      eol: f.eol,
      bom: f.bom,
      base: f.stamp,
      savedAt: Date.now(),
    };
    this.draftOnDisk = true;
    void api.saveEditorDraft(this.id, encodeDraft(draft)).catch((e) => {
      console.warn("editor: draft save failed", e);
    });
  }

  private async offerDraft(path: string, diskText: string, gen: number): Promise<void> {
    const blob = await api.loadEditorDraft(this.id).catch(() => "");
    if (gen !== this.loadGen || this.disposed) return;
    const draft = parseDraft(blob);
    if (draftOffer(draft, path, diskText) === "offer" && draft) {
      this.draftOnDisk = true;
      this.setBanner({ kind: "draft", draft });
    } else if (blob) {
      void api.deleteEditorDraft(this.id).catch(() => {});
    }
  }

  /// Put a recovered draft into the buffer as one undoable edit. The draft's
  /// base stamp becomes the save guard: if the file moved on since, the next
  /// save hits `conflict` and asks instead of overwriting.
  private restoreDraft(draft: Draft): void {
    const h = this.handle;
    if (!h || !this.file) return;
    this.file = { ...this.file, eol: draft.eol, bom: draft.bom, stamp: draft.base };
    this.setBanner(null);
    h.replaceAll(draft.text);
    this.refreshDirty();
    this.renderChrome();
  }

  // ── Rendering ─────────────────────────────────────────────────────────────

  private setBanner(b: Banner | null): void {
    this.banner = b;
    this.renderBanner();
  }

  private renderBanner(): void {
    const b = this.banner;
    this.bannerEl.replaceChildren();
    this.bannerEl.hidden = !b;
    this.bannerEl.className = "editor__banner";
    if (!b) return;
    const text = document.createElement("span");
    text.className = "editor__banner-text";
    const actions = document.createElement("span");
    actions.className = "editor__banner-actions";
    const name = this.displayName();
    const button = (key: string, onClick: () => void, primary = false) => {
      const el = document.createElement("button");
      el.type = "button";
      el.className = primary ? "editor__banner-btn editor__banner-btn--primary" : "editor__banner-btn";
      el.textContent = t(key);
      el.addEventListener("click", onClick);
      actions.appendChild(el);
    };
    switch (b.kind) {
      case "changed":
        this.bannerEl.classList.add("editor__banner--warn");
        text.textContent = fill(t("editor.changedOnDisk"), { name });
        button("editor.reload", () => void this.reloadFromDisk());
        button(
          "editor.keepMine",
          () => {
            this.dismissedSha = b.sha;
            this.setBanner(null);
          },
          true,
        );
        break;
      case "deleted":
        this.bannerEl.classList.add("editor__banner--warn");
        text.textContent = fill(t("editor.deletedOnDisk"), { name });
        button("editor.save", () => void this.save(), true);
        break;
      case "draft":
        text.textContent = t("editor.draftFound");
        button("editor.discard", () => {
          void this.discardDraft();
          this.setBanner(null);
        });
        button("editor.restore", () => this.restoreDraft(b.draft), true);
        break;
      case "mixedEol":
        text.textContent = t("editor.mixedEol");
        button("editor.dismiss", () => this.setBanner(null));
        break;
      case "readOnly":
        text.textContent = t("editor.readOnlyLarge");
        break;
    }
    this.bannerEl.append(text, actions);
  }

  /// Everything that depends on the file / dirty / problem state.
  private renderChrome(): void {
    const dirty = this.isDirty();
    this.element.classList.toggle("editor-pane--dirty", dirty);
    this.dotEl.title = dirty ? t("editor.unsaved") : "";
    this.nameEl.textContent = this.displayName();
    const cut = Math.max(this.path.lastIndexOf("/"), this.path.lastIndexOf("\\"));
    this.dirEl.textContent = cut > 0 ? this.path.slice(0, cut) : "";
    this.nameEl.parentElement!.title = this.path;
    this.saveBtn.disabled = !this.file || this.file.readOnly;
    this.host.hidden = !this.file;
    this.renderOverlay();
    this.renderFacts();
    this.updateTitle();
  }

  private renderOverlay(): void {
    this.overlay.replaceChildren();
    const show = !this.file && (this.problem !== null || !this.path);
    this.overlay.hidden = !show;
    if (!show) return;
    const head = document.createElement("p");
    head.className = "editor__overlay-title";
    const row = document.createElement("div");
    row.className = "files__overlay-actions";
    const btn = (key: string, onClick: () => void) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "files__overlay-btn";
      b.textContent = t(key);
      b.addEventListener("click", onClick);
      row.appendChild(b);
    };
    if (this.problem) {
      this.overlay.classList.add("editor__overlay--error");
      head.textContent = fill(t("editor.cantOpen"), { name: this.displayName() });
      const why = document.createElement("p");
      why.textContent = this.problem.text;
      if (this.problem.retry) btn("files.retry", () => void this.load(this.path, { offerDraft: false }));
      btn("editor.openFile", () => void this.promptOpen());
      this.overlay.append(head, why, row);
      return;
    }
    this.overlay.classList.remove("editor__overlay--error");
    head.textContent = t("editor.noFile");
    btn("editor.openFile", () => void this.promptOpen());
    this.overlay.append(head, row);
  }

  private renderFacts(): void {
    const f = this.file;
    this.statusEol.textContent = f ? `${eolLabel(f.eol)}${f.bom ? "  UTF-8 BOM" : "  UTF-8"}` : "";
    this.statusLang.textContent = f ? (f.lang ? LANG_LABEL[f.lang] : t("editor.plainText")) : "";
    this.renderPosition();
  }

  private renderPosition(): void {
    if (!this.handle || !this.file) {
      this.statusPos.textContent = "";
      return;
    }
    const c = this.handle.cursor();
    this.statusPos.textContent = fill(t("editor.cursor"), { line: c.line, col: c.col + 1 });
  }

  private say(text: string, error = false): void {
    if (this.statusTimer !== null) clearTimeout(this.statusTimer);
    this.statusMsg.textContent = text;
    this.statusMsg.classList.toggle("editor__status-msg--error", error);
    if (!error && text && text !== t("editor.loading")) {
      this.statusTimer = window.setTimeout(() => {
        this.statusMsg.textContent = "";
      }, 2500);
    }
  }

  private updateTitle(): void {
    if (this.titleEl) {
      const label = this.title || this.displayName();
      this.titleEl.textContent = this.isDirty() ? `${label} ●` : label;
    }
  }

  private buildTitle(): void {
    this.titleEl = document.createElement("div");
    this.titleEl.className = "pane-title";
    this.element.insertBefore(this.titleEl, this.element.firstChild);
    this.updateTitle();
  }

  private updateLang(): void {
    for (const { el, key, chord } of this.buttons) {
      el.title = chord ? `${t(key)} (${shortcutLabel(chord)})` : t(key);
      el.setAttribute("aria-label", t(key));
    }
    this.renderBanner();
    this.renderChrome();
  }

  private makeButton(icon: string, key: string, onClick: () => void, chord?: string): HTMLButtonElement {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "files__btn";
    b.tabIndex = -1;
    b.innerHTML = icon;
    // Keep the caret in the editor: a toolbar click is not a place to leave it.
    b.addEventListener("mousedown", (ev) => ev.preventDefault());
    b.addEventListener("click", () => {
      onClick();
      if (key !== "editor.find") this.handle?.focus();
    });
    this.buttons.push({ el: b, key, chord });
    return b;
  }

  // ── Dialogs ───────────────────────────────────────────────────────────────

  private async askClose(choices: CloseChoice[]): Promise<CloseChoice | null> {
    const labels: Record<CloseChoice, string> = {
      save: t("editor.save"),
      discard: t("editor.dontSave"),
      cancel: t("dialog.cancel"),
    };
    const ans = await askChoice(
      fill(t("editor.closePrompt"), { name: this.displayName() }),
      this.path || null,
      choices.map((id) => ({ id, label: labels[id], primary: id === "save" })),
    );
    return (ans?.id as CloseChoice | undefined) ?? null;
  }

  private async promptOpen(): Promise<void> {
    const typed = await askText(t("editor.openPrompt"), this.path);
    if (typed === null || !typed.trim()) return;
    await this.openFile(typed.trim());
  }
}
