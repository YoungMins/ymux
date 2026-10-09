import { t, onLangChange } from "../i18n/i18n";
import { clampDockWidth, parseDockState, serializeDockState } from "../filedock/dockModel";
import { TokenMonitorPane } from "./TokenMonitor";

interface DockManager {
  focusActivePane(): void;
  refitActive(): void;
}
const STORAGE_KEY = "ymux.tokenDock";

export class TokenDock {
  readonly element = document.createElement("aside");
  private readonly pane = new TokenMonitorPane({ id: "token-dock", ownChrome: false });
  private state = parseDockState(null);
  private readonly cleanupLang: () => void;

  constructor(private readonly manager: DockManager) {
    try { this.state = parseDockState(localStorage.getItem(STORAGE_KEY)); }
    catch (error: unknown) { console.warn("Token sidebar preference unavailable", error); }
    this.element.className = "token-dock";
    this.element.style.width = `${this.state.width}px`;
    const header = document.createElement("header"); header.className = "token-dock__header";
    const title = document.createElement("strong");
    const close = document.createElement("button"); close.type = "button"; close.textContent = "×";
    close.onclick = () => this.setOpen(false);
    const translate = (): void => {
      title.textContent = t("usage.title"); this.element.setAttribute("aria-label", t("usage.title"));
      close.title = t("usage.close"); close.setAttribute("aria-label", t("usage.close"));
    };
    translate(); this.cleanupLang = onLangChange(translate);
    header.append(title, close);
    const resizer = document.createElement("div"); resizer.className = "token-dock__resizer";
    resizer.addEventListener("pointerdown", ev => this.resize(resizer, ev));
    this.element.append(resizer, header, this.pane.element);
    this.element.classList.toggle("token-dock--open", this.state.open);
  }
  start(): void { void this.pane.spawn(); }
  toggle(): void { this.setOpen(!this.state.open); }
  open(): void { this.setOpen(true); }
  dispose(): void { this.cleanupLang(); this.pane.dispose(); this.element.remove(); }
  private persist(): void {
    try { localStorage.setItem(STORAGE_KEY, serializeDockState(this.state)); }
    catch (error: unknown) { console.warn("Token sidebar preference could not be saved", error); }
  }
  private setOpen(open: boolean): void {
    const hadFocus = this.element.contains(document.activeElement);
    this.state = { ...this.state, open };
    this.element.classList.toggle("token-dock--open", open); this.persist();
    if (open) { this.pane.scheduleFit(); this.pane.focus(); }
    else if (hadFocus) this.manager.focusActivePane();
    requestAnimationFrame(() => this.manager.refitActive());
  }
  private resize(handle: HTMLElement, ev: PointerEvent): void {
    if (ev.button !== 0) return;
    ev.preventDefault(); handle.setPointerCapture(ev.pointerId);
    const move = (event: PointerEvent): void => {
      const bounds = this.element.getBoundingClientRect();
      const width = clampDockWidth(bounds.right - event.clientX, this.element.parentElement?.clientWidth ?? window.innerWidth);
      this.state = { ...this.state, width }; this.element.style.width = `${width}px`;
    };
    const end = (): void => {
      handle.removeEventListener("pointermove", move); handle.removeEventListener("pointerup", end); handle.removeEventListener("pointercancel", end);
      this.persist(); requestAnimationFrame(() => this.manager.refitActive());
    };
    handle.addEventListener("pointermove", move); handle.addEventListener("pointerup", end); handle.addEventListener("pointercancel", end);
  }
}
let instance: TokenDock | null = null;
export function mountTokenDock(parent: HTMLElement, manager: DockManager): TokenDock {
  instance?.dispose(); instance = new TokenDock(manager); parent.append(instance.element); instance.start(); return instance;
}
export function toggleTokenDock(): void { instance?.toggle(); }
export function openTokenDock(): void { instance?.open(); }
