/// Keep Tab inside the modal `root`: wrap at either end.
export function trapTab(root: HTMLElement, ev: KeyboardEvent): void {
  const items = [
    ...root.querySelectorAll<HTMLElement>(
      'button, select, input, textarea, [tabindex]:not([tabindex="-1"])',
    ),
  ].filter((x) => !(x as HTMLButtonElement).disabled && x.offsetParent !== null);
  if (items.length === 0) {
    ev.preventDefault();
    return;
  }
  const first = items[0];
  const last = items[items.length - 1];
  const cur = document.activeElement;
  if (ev.shiftKey && (cur === first || cur === root || !root.contains(cur))) {
    ev.preventDefault();
    last.focus();
  } else if (!ev.shiftKey && (cur === last || !root.contains(cur))) {
    ev.preventDefault();
    first.focus();
  }
}
