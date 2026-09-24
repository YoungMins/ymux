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
  closeState,
  conflictDecision,
  copyCandidate,
  draftFileAction,
  fileName,
  isDocDirty,
  languageForPath,
  openFileDecision,
  pollStep,
  writeArgsFor,
  type DocLike,
  type LangId,
} from "./editorModel";
import { closeDecision, closeResult, type CloseChoice } from "./closeGuard";
import { eolLabel, needsEolWarning, saveEol, type Eol } from "./eol";
import { draftDisposition, encodeDraft, parseDraft, type DiskView, type Draft } from "./draft";

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
  /// A recovered draft offered and not yet answered. Its own banner row,
  /// above any other, and until answered nothing — an edit, a save, an
  /// agent's rewrite reloading the buffer, closing the pane — may overwrite
  /// or delete the draft file (`draftFileAction`, `closeState`). `restore`
  /// puts it into the loaded buffer; `rescue` writes it to a file the user
  /// picks, for when the buffer cannot take it (the read failed, or the file
  /// is read-only now).
  private pendingDraft: { draft: Draft; mode: "restore" | "rescue" } | null = null;
  /// The pane has looked for its draft (spawn's first load, success or not).
  /// Until then a draft on disk is one nobody has been offered: it is never
  /// overwritten or deleted.
  private draftChecked = false;
  /// Looking for the draft failed (IO): leave whatever is there alone.
  private draftUnreadable = false;
  /// Non-null while the pane cannot show an editable file.
  private problem: { text: string; retry: boolean } | null = null;
  private loadGen = 0;
  private spawned: Promise<void> | null = null;
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
    this.bannerEl.className = "editor__banners";
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
    if (!this.spawned) {
      this.spawned = (async () => {
        // The draft is looked for whatever the load does: a pane whose file
        // is gone is exactly the one whose draft is the only copy.
        if (this.path) await this.load(this.path, { offerDraft: true });
        else this.renderChrome();
        this.draftChecked = true;
      })();
    }
    return this.spawned;
  }

  private draftUnanswered(): boolean {
    return this.pendingDraft !== null || !this.draftChecked || this.draftUnreadable;
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
      if (!permanent && draftFileAction(this.draftUnanswered(), this.dirty, this.deletedOnDisk) === "write") {
        void this.writeDraft();
      }
    }
    // Permanent: the user closed this pane after the guard let it go —
    // there is nothing left to recover into. Unless the pane never got as
    // far as looking for its draft: one nobody was offered is not deleted
    // here (the startup sweep removes it once its pane is gone from the
    // config).
    if (permanent && this.draftChecked && !this.draftUnreadable) {
      void api.deleteEditorDraft(this.id).catch(() => {});
    }
    for (const c of this.cleanups) c();
    this.handle?.destroy();
    this.handle = null;
    this.element.remove();
  }

  // ── Host API ──────────────────────────────────────────────────────────────

  private closeState(): { unsaved: boolean; savable: boolean } {
    return closeState({
      loaded: this.file !== null,
      readOnly: this.file?.readOnly ?? false,
      dirty: this.dirty,
      deletedOnDisk: this.deletedOnDisk,
      pendingDraft: this.pendingDraft !== null,
      hasPath: this.path !== "",
    });
  }

  /// Unsaved work that closing would lose — including a recovered draft not
  /// yet restored or discarded.
  isDirty(): boolean {
    return this.closeState().unsaved;
  }

  /// Whether a close prompt may offer Save (false for a pending draft: Save
  /// would write the buffer, not the draft).
  canSaveOnClose(): boolean {
    return this.closeState().savable;
  }

  currentPath(): string {
    return this.path;
  }

  displayName(): string {
    return this.path ? fileName(this.path) : t("editor.untitled");
  }

  /// The close guard (spec §3.5). Resolves false to cancel the close.
  async canClose(): Promise<boolean> {
    const choices = closeDecision(this.isDirty(), this.canSaveOnClose());
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
    // A draft nobody was offered (not looked for yet, or unreadable) is not
    // this call's to delete; an offered one being discarded is.
    if (!this.pendingDraft && (!this.draftChecked || this.draftUnreadable)) return;
    this.draftOnDisk = false;
    if (this.pendingDraft) {
      this.pendingDraft = null;
      this.renderBanner();
      this.renderChrome();
      this.opts.onDirtyChange?.();
    }
    await api.deleteEditorDraft(this.id).catch(() => {});
  }

  /// Show `path` in this pane (the viewer tab's reuse, spec §3.7). Asks
  /// before replacing a dirty buffer; resolves false if the user kept it.
  async openFile(path: string): Promise<boolean> {
    // Let the first load finish looking for a draft before deciding: a draft
    // not yet offered must count as unsaved work, not be discarded unseen.
    await this.spawn();
    const decision = openFileDecision(this.path || null, this.isDirty(), path, this.file !== null);
    if (decision === "focus") {
      this.focus();
      return true;
    }
    if (decision === "retry") {
      await this.load(this.path, { offerDraft: true });
      this.focus();
      return true;
    }
    if (decision === "ask") {
      const choices = closeDecision(true, this.canSaveOnClose());
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
      const gone =
        this.deletedOnDisk &&
        (await fsApi.stat(this.path).then(
          () => false,
          (e) => errorKind(e) === "not_found",
        ));
      return await this.write(this.path, gone ? null : f.stamp);
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
    const args =
      expect === null
        ? writeArgsFor({ path, text: doc.toString(), eol: f.eol, bom: f.bom, stamp: f.stamp, goneOnDisk: true })
        : writeArgsFor({ path, text: doc.toString(), eol: f.eol, bom: f.bom, stamp: expect, goneOnDisk: false });
    const eol = args.eol;
    let stamp: ContentStamp;
    try {
      stamp = await fsApi.writeText(args);
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
    // write left the buffer dirty; its draft stays. A pending recovered
    // draft is not this buffer's and is left for the user to answer.)
    if (draftFileAction(this.draftUnanswered(), this.dirty, this.deletedOnDisk) === "delete") void this.discardDraft();
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
      if (o.offerDraft) await this.offerDraft(path, null, gen);
      return;
    }
    let h: EditorHandle;
    try {
      h = await this.ensureHandle();
    } catch (e) {
      if (gen !== this.loadGen || this.disposed) return;
      this.problem = { text: String(e), retry: true };
      this.renderChrome();
      if (o.offerDraft) await this.offerDraft(path, null, gen);
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
    if (o.offerDraft) await this.offerDraft(path, { text: tf.text, editable: !tf.truncated }, gen);
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
    // An agent's rewrite reloading a clean buffer must not take a pending
    // recovered draft with it.
    if (draftFileAction(this.draftUnanswered(), false) === "delete") void this.discardDraft();
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
          // The buffer just became the only copy: draft it now, not on the
          // next keystroke.
          if (draftFileAction(this.draftUnanswered(), this.dirty, true) === "write") void this.writeDraft();
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
    const action = draftFileAction(this.draftUnanswered(), this.dirty, this.deletedOnDisk);
    if (action === "write") this.scheduleDraft();
    else if (action === "delete" && (this.draftOnDisk || this.draftTimer !== null)) {
      void this.discardDraft();
    }
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
      if (draftFileAction(this.draftUnanswered(), this.dirty, this.deletedOnDisk) === "write") void this.writeDraft();
    }, DRAFT_DELAY_MS);
  }

  /// Write the draft now if one is waiting in its debounce (the window is
  /// closing without the guard's answer, or the page is unloading).
  flushDraft(): Promise<void> {
    if (this.draftTimer === null) return Promise.resolve();
    clearTimeout(this.draftTimer);
    this.draftTimer = null;
    if (draftFileAction(this.draftUnanswered(), this.dirty, this.deletedOnDisk) !== "write") {
      return Promise.resolve();
    }
    return this.writeDraft();
  }

  private writeDraft(): Promise<void> {
    const h = this.handle;
    const f = this.file;
    if (!h || !f || !this.path || this.pendingDraft) return Promise.resolve();
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
    return api.saveEditorDraft(this.id, encodeDraft(draft)).catch((e) => {
      console.warn("editor: draft save failed", e);
    });
  }

  /// Look for this pane's draft and offer it. `disk` is what the load found:
  /// `null` when the read failed, which is when the draft matters most.
  private async offerDraft(path: string, disk: DiskView | null, gen: number): Promise<void> {
    let blob: string;
    try {
      blob = await api.loadEditorDraft(this.id);
    } catch (e) {
      // Could not even look: treat whatever is there as unoffered, so no
      // path in this session deletes or overwrites it.
      console.warn("editor: draft read failed", e);
      this.draftUnreadable = true;
      return;
    }
    if (gen !== this.loadGen || this.disposed) return;
    const draft = parseDraft(blob);
    const disposition = draftDisposition(draft, path, disk);
    if (draft && (disposition === "restore" || disposition === "rescue")) {
      this.draftOnDisk = true;
      this.pendingDraft = { draft, mode: disposition };
      this.renderBanner();
      this.renderChrome();
      this.opts.onDirtyChange?.();
    } else {
      if (this.pendingDraft) {
        this.pendingDraft = null;
        this.renderBanner();
        this.renderChrome();
        this.opts.onDirtyChange?.();
      }
      // Identical to the disk (nothing to lose), or not a draft at all.
      if (blob) void api.deleteEditorDraft(this.id).catch(() => {});
    }
  }

  /// Write a draft the buffer cannot take (`rescue`) to a file the user
  /// picks — by default its own path, recreating a deleted file. Never
  /// overwrites an existing file without asking. On success the pane opens
  /// that file if it had nothing usable open.
  private async rescueDraft(draft: Draft): Promise<void> {
    const typed = await askText(t("editor.saveDraftAs"), draft.path);
    if (typed === null || !typed.trim()) return;
    const target = typed.trim();
    const exists = await fsApi.stat(target).then(
      () => true,
      (e) => errorKind(e) !== "not_found",
    );
    if (exists) {
      const ok = await askConfirm(fill(t("editor.overwriteFile"), { name: fileName(target) }), t("editor.overwrite"));
      if (!ok) return;
    }
    try {
      await fsApi.writeText({
        path: target,
        text: draft.text,
        eol: saveEol(draft.eol),
        bom: draft.bom,
        expect: null,
      });
    } catch (e) {
      this.say(`${fileName(target)}: ${describeFsError(e)}`, true);
      return;
    }
    if (this.disposed) return;
    this.pendingDraft = null;
    this.draftOnDisk = false;
    await api.deleteEditorDraft(this.id).catch(() => {});
    this.say(fill(t("editor.savedCopy"), { name: fileName(target) }));
    this.renderBanner();
    this.renderChrome();
    this.opts.onDirtyChange?.();
    if (!this.file || this.file.readOnly) {
      this.path = target;
      this.opts.onPathChange?.(target);
      await this.load(target, { offerDraft: false });
    }
  }

  /// Put a recovered draft into the buffer as one undoable edit. The draft's
  /// base stamp becomes the save guard: if the file moved on since, the next
  /// save hits `conflict` and asks instead of overwriting.
  private restoreDraft(draft: Draft): void {
    const h = this.handle;
    if (!h || !this.file) return;
    this.file = { ...this.file, eol: draft.eol, bom: draft.bom, stamp: draft.base };
    // Answered: from here the draft is an ordinary draft of this buffer,
    // rewritten by the next edit's debounce.
    this.pendingDraft = null;
    this.renderBanner();
    h.replaceAll(draft.text);
    this.refreshDirty();
    this.renderChrome();
    this.opts.onDirtyChange?.();
  }

  // ── Rendering ─────────────────────────────────────────────────────────────

  private setBanner(b: Banner | null): void {
    this.banner = b;
    this.renderBanner();
  }

  /// Up to two rows: a pending recovered draft (always first — it is the
  /// only copy of those edits), then the file-state banner.
  private renderBanner(): void {
    this.bannerEl.replaceChildren();
    const draft = this.pendingDraft;
    const b = this.banner;
    this.bannerEl.hidden = !draft && !b;
    const row = (warn: boolean) => {
      const el = document.createElement("div");
      el.className = warn ? "editor__banner editor__banner--warn" : "editor__banner";
      const text = document.createElement("span");
      text.className = "editor__banner-text";
      const actions = document.createElement("span");
      actions.className = "editor__banner-actions";
      el.append(text, actions);
      this.bannerEl.appendChild(el);
      const button = (key: string, onClick: () => void, primary = false) => {
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = primary ? "editor__banner-btn editor__banner-btn--primary" : "editor__banner-btn";
        btn.textContent = t(key);
        btn.addEventListener("click", onClick);
        actions.appendChild(btn);
      };
      return { text, button };
    };
    if (draft) {
      const r = row(true);
      if (draft.mode === "restore") {
        r.text.textContent = t("editor.draftFound");
        r.button("editor.discard", () => void this.discardDraft());
        r.button("editor.restore", () => this.restoreDraft(draft.draft), true);
      } else {
        // The buffer cannot take it: the draft is the only copy of those
        // edits, so Discard asks, and Save as… is the way out.
        // A draft for another file names that file in full: it is not the
        // one this pane shows.
        const foreign = draft.draft.path.normalize("NFC") !== this.path.normalize("NFC");
        r.text.textContent = fill(t("editor.draftRescue"), {
          name: foreign ? draft.draft.path : fileName(draft.draft.path),
        });
        r.text.title = draft.draft.path;
        r.button("editor.discard", () => {
          void askConfirm(t("editor.discardDraftConfirm"), t("editor.discard")).then((ok) => {
            if (ok) void this.discardDraft();
          });
        });
        r.button("editor.saveDraftAsButton", () => void this.rescueDraft(draft.draft), true);
      }
    }
    if (!b) return;
    const name = this.displayName();
    switch (b.kind) {
      case "changed": {
        const r = row(true);
        r.text.textContent = fill(t("editor.changedOnDisk"), { name });
        r.button("editor.reload", () => void this.reloadFromDisk());
        r.button(
          "editor.keepMine",
          () => {
            this.dismissedSha = b.sha;
            this.setBanner(null);
          },
          true,
        );
        break;
      }
      case "deleted": {
        const r = row(true);
        r.text.textContent = fill(t("editor.deletedOnDisk"), { name });
        r.button("editor.save", () => void this.save(), true);
        break;
      }
      case "mixedEol": {
        const r = row(false);
        r.text.textContent = t("editor.mixedEol");
        r.button("editor.dismiss", () => this.setBanner(null));
        break;
      }
      case "readOnly": {
        const r = row(false);
        r.text.textContent = t("editor.readOnlyLarge");
        break;
      }
    }
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
      // Retry looks for the draft again: a load that now succeeds offers it
      // for Restore instead of letting the first keystroke overwrite it.
      if (this.problem.retry) btn("files.retry", () => void this.load(this.path, { offerDraft: true }));
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
