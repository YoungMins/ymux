// The office shrunk into a corner: the frame's class, its place and size
// (CSS variables from `miniPlace`), and the drags that move it by its head
// and resize it by the corner grip. OfficeView decides what else entering
// or leaving mini changes.

import { t } from "../i18n/i18n";
import { clampMini, loadMini, movedMini, resizedMini, saveMini, type MiniPlace } from "./miniPlace";
import { followPointer, swallowNextClick } from "./pointerDrag";

type Step = (start: MiniPlace, dx: number, dy: number, w: number, h: number) => MiniPlace;

export class MiniFrame {
  private onNow = false;
  private place: MiniPlace = loadMini();
  private cancelDrag: (() => void) | null = null;

  constructor(
    private readonly element: HTMLElement,
    head: HTMLElement,
  ) {
    const grip = document.createElement("div");
    grip.className = "office__mini-grip";
    grip.title = t("fun.miniMove");
    element.appendChild(grip);
    head.addEventListener("pointerdown", (ev) => {
      if ((ev.target as HTMLElement | null)?.closest("button, select, input")) return;
      this.drag(ev, movedMini);
    });
    grip.addEventListener("pointerdown", (ev) => {
      ev.stopPropagation();
      this.drag(ev, resizedMini);
    });
  }

  get on(): boolean {
    return this.onNow;
  }

  set(on: boolean): void {
    this.onNow = on;
    this.element.classList.toggle("office--mini", on);
    this.element.title = on ? t("fun.miniMove") : "";
    this.apply();
  }

  dispose(): void {
    this.cancelDrag?.();
  }

  /// The window the frame sits in.
  private area(): [number, number] {
    const host = this.element.offsetParent as HTMLElement | null;
    return [host?.clientWidth ?? window.innerWidth, host?.clientHeight ?? window.innerHeight];
  }

  private drag(ev: PointerEvent, step: Step): void {
    if (!this.onNow || ev.button !== 0) return;
    const start = this.place;
    const [w, h] = this.area();
    const x0 = ev.clientX;
    const y0 = ev.clientY;
    let moved = false;
    ev.preventDefault();
    this.cancelDrag = followPointer(
      (e) => {
        const dx = e.clientX - x0;
        const dy = e.clientY - y0;
        if (!moved && Math.abs(dx) + Math.abs(dy) < 4) return;
        moved = true;
        this.place = step(start, dx, dy, w, h);
        this.apply();
      },
      (e) => {
        this.cancelDrag = null;
        if (!moved) return;
        saveMini(this.place);
        // The click that ends a drag is not a "bring it back".
        if (e) swallowNextClick();
      },
    );
  }

  private apply(): void {
    const s = this.element.style;
    if (!this.onNow) {
      for (const k of ["--mini-r", "--mini-b", "--mini-scale"]) s.removeProperty(k);
      return;
    }
    const [w, h] = this.area();
    this.place = clampMini(this.place, w, h);
    s.setProperty("--mini-r", `${this.place.right}px`);
    s.setProperty("--mini-b", `${this.place.bottom}px`);
    s.setProperty("--mini-scale", String(this.place.scale));
  }
}
