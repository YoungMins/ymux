import { t, onLangChange } from "../i18n/i18n";
import { pushPopup, popPopup } from "../browser/popupBlur";
import { formatWorkspaceLabel } from "../workspace/workspaceLabel";

const STORAGE_PREFIX = "ymux-notes-";

let backdrop: HTMLElement | null = null;
let modal: HTMLElement | null = null;
let titleEl: HTMLElement | null = null;
let closeBtn: HTMLButtonElement | null = null;
let textarea: HTMLTextAreaElement | null = null;
let keyHandler: ((ev: KeyboardEvent) => void) | null = null;
let langCleanup: (() => void) | null = null;

let currentWorkspaceId: number | null = null;
let currentWorkspaceName: string | null = null;
let currentWorkspacePosition = 0;
let isOpen = false;

const changeListeners = new Set<() => void>();

function storageKey(wsId: number): string {
  return `${STORAGE_PREFIX}${wsId}`;
}

export function hasNotes(workspaceId: number): boolean {
  try {
    const v = localStorage.getItem(storageKey(workspaceId));
    return !!v && v.trim().length > 0;
  } catch {
    return false;
  }
}

export function onNotesChange(cb: () => void): () => void {
  changeListeners.add(cb);
  return () => {
    changeListeners.delete(cb);
  };
}

export function mountNotesOverlay(container: HTMLElement): () => void {
  if (modal) return () => {};

  backdrop = document.createElement("div");
  backdrop.className = "notes-backdrop";
  backdrop.setAttribute("aria-hidden", "true");
  backdrop.addEventListener("click", hide);
  container.appendChild(backdrop);

  modal = document.createElement("div");
  modal.className = "notes-modal";
  modal.setAttribute("role", "dialog");
  modal.setAttribute("aria-modal", "true");
  modal.setAttribute("aria-labelledby", "notes-modal-title");

  const header = document.createElement("div");
  header.className = "notes-modal__header";

  titleEl = document.createElement("h2");
  titleEl.id = "notes-modal-title";
  titleEl.className = "notes-modal__title";
  titleEl.textContent = t("notes.title");
  header.appendChild(titleEl);

  closeBtn = document.createElement("button");
  closeBtn.type = "button";
  closeBtn.className = "notes-modal__close";
  closeBtn.title = t("notes.close");
  closeBtn.setAttribute("aria-label", t("notes.close"));
  closeBtn.textContent = "×";
  closeBtn.addEventListener("click", hide);
  header.appendChild(closeBtn);

  modal.appendChild(header);

  textarea = document.createElement("textarea");
  textarea.className = "notes-modal__textarea";
  textarea.placeholder = t("notes.placeholder");
  textarea.spellcheck = false;
  textarea.addEventListener("input", () => {
    if (currentWorkspaceId === null || !textarea) return;
    try {
      localStorage.setItem(storageKey(currentWorkspaceId), textarea.value);
    } catch {
      /* localStorage unavailable */
    }
    for (const cb of changeListeners) cb();
  });
  modal.appendChild(textarea);

  container.appendChild(modal);

  keyHandler = (ev: KeyboardEvent) => {
    if (
      ev.key === "Escape" &&
      modal &&
      modal.classList.contains("notes-modal--visible")
    ) {
      ev.preventDefault();
      hide();
    }
  };
  document.addEventListener("keydown", keyHandler);

  langCleanup = onLangChange(() => {
    if (closeBtn) {
      closeBtn.title = t("notes.close");
      closeBtn.setAttribute("aria-label", t("notes.close"));
    }
    if (textarea) textarea.placeholder = t("notes.placeholder");
    renderTitle();
  });

  return () => {
    if (isOpen) {
      isOpen = false;
      popPopup();
    }
    if (keyHandler) document.removeEventListener("keydown", keyHandler);
    langCleanup?.();
    modal?.remove();
    backdrop?.remove();
    modal = null;
    backdrop = null;
    titleEl = null;
    closeBtn = null;
    textarea = null;
    keyHandler = null;
    langCleanup = null;
    currentWorkspaceId = null;
    currentWorkspaceName = null;
  };
}

export function toggle(
  workspaceId: number,
  workspaceName?: string | null,
  position?: number,
): void {
  if (!modal) return;
  const visible = modal.classList.contains("notes-modal--visible");
  if (visible && currentWorkspaceId === workspaceId) {
    hide();
  } else {
    show(workspaceId, workspaceName ?? null, position ?? workspaceId);
  }
}

function show(workspaceId: number, workspaceName: string | null, position: number): void {
  if (!modal || !backdrop || !textarea) return;
  if (!isOpen) {
    isOpen = true;
    pushPopup();
  }
  currentWorkspaceId = workspaceId;
  currentWorkspaceName = workspaceName;
  currentWorkspacePosition = position;
  try {
    textarea.value = localStorage.getItem(storageKey(workspaceId)) ?? "";
  } catch {
    textarea.value = "";
  }
  renderTitle();
  backdrop.classList.add("notes-backdrop--visible");
  modal.classList.add("notes-modal--visible");
  backdrop.setAttribute("aria-hidden", "false");
  textarea.focus();
}

function hide(): void {
  if (!modal || !backdrop) return;
  if (!isOpen) return;
  isOpen = false;
  popPopup();
  backdrop.classList.remove("notes-backdrop--visible");
  modal.classList.remove("notes-modal--visible");
  backdrop.setAttribute("aria-hidden", "true");
}

function renderTitle(): void {
  if (!titleEl) return;
  if (currentWorkspaceId === null) {
    titleEl.textContent = t("notes.title");
    return;
  }
  const label = formatWorkspaceLabel(
    currentWorkspaceId,
    currentWorkspaceName,
    currentWorkspacePosition,
  );
  titleEl.textContent = `${t("notes.title")} — ${label}`;
}
