// Pointer drags that follow the pointer over the whole window (a drag can
// leave the element it started on), shared by the office's drags: board
// notes, furniture, and the mini office.

/// Follow the pointer until it is released. `end` gets the release, or null
/// when the drag was cut short (pointercancel, the window losing focus, or
/// the returned cancel being called), which must drop nothing.
export function followPointer(move: (ev: PointerEvent) => void, end: (ev: PointerEvent | null) => void): () => void {
  let done = false;
  const stop = (ev: PointerEvent | null): void => {
    if (done) return;
    done = true;
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("pointercancel", cancel);
    window.removeEventListener("blur", cancel);
    end(ev);
  };
  const up = (ev: PointerEvent): void => stop(ev);
  const cancel = (): void => stop(null);
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  window.addEventListener("pointercancel", cancel);
  window.addEventListener("blur", cancel);
  return cancel;
}

/// Swallow the click a drag's pointerup produces, and only that one: a
/// release with no click (outside the window) must not eat a later click.
export function swallowNextClick(): void {
  const swallow = (c: MouseEvent): void => {
    c.stopPropagation();
    c.preventDefault();
  };
  window.addEventListener("click", swallow, { capture: true, once: true });
  setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
}
