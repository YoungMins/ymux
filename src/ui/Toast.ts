/// A minimal non-blocking toast: a short line in the bottom-right corner that
/// disappears after a few seconds. With `onClick` the whole toast is a button.
/// Up to `MAX_TOASTS` stack (newest at the bottom), so a notice is not wiped
/// by the next one. A sticky toast (`ms = STICKY_TOAST`) stays until it is
/// clicked, and is the last to be evicted when the stack is full.

/// `ms` value: no timeout, dismissed by a click only.
export const STICKY_TOAST = 0;
const MAX_TOASTS = 4;

interface Entry {
  el: HTMLElement;
  timer: number | null;
  sticky: boolean;
}

const stack: Entry[] = [];
let host: HTMLElement | null = null;

function container(): HTMLElement {
  if (!host || !host.isConnected) {
    host = document.createElement("div");
    host.className = "toast-stack";
    document.body.appendChild(host);
  }
  return host;
}

export function showToast(text: string, onClick?: () => void, ms = 5000): void {
  const sticky = ms === STICKY_TOAST;
  // A sticky toast is always a button: the click is how it is dismissed.
  const el = document.createElement(onClick || sticky ? "button" : "div");
  el.className = "toast";
  el.setAttribute("role", "status");
  el.textContent = text;
  const entry: Entry = { el, timer: null, sticky };
  if (el instanceof HTMLButtonElement) {
    el.type = "button";
    el.addEventListener("click", () => {
      dismiss(entry);
      onClick?.();
    });
  }
  while (stack.length >= MAX_TOASTS) {
    dismiss(stack.find((e) => !e.sticky) ?? stack[0]);
  }
  container().appendChild(el);
  stack.push(entry);
  if (!sticky) entry.timer = window.setTimeout(() => dismiss(entry), ms);
}

function dismiss(entry: Entry): void {
  const i = stack.indexOf(entry);
  if (i < 0) return;
  stack.splice(i, 1);
  if (entry.timer !== null) window.clearTimeout(entry.timer);
  entry.el.remove();
}
