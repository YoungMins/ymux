// Short-lived effects over the office floor: banners taking turns, an emoji
// rising over a character, falling paper, fireworks. Each removes itself;
// `sleep` is the view's, so they all stop when the view is disposed.

export class FloorEffects {
  private readonly queue: string[] = [];
  private busy = false;

  constructor(
    private readonly floor: HTMLElement,
    private readonly bannerEl: HTMLElement,
    private readonly sleep: (ms: number) => Promise<void>,
    private readonly alive: () => boolean,
  ) {}

  /// A banner over the floor; several wait their turn.
  banner(text: string): void {
    this.queue.push(text);
    if (this.busy) return;
    this.busy = true;
    const next = (): void => {
      const msg = this.queue.shift();
      if (msg === undefined || !this.alive()) {
        this.busy = false;
        this.bannerEl.hidden = true;
        return;
      }
      this.bannerEl.textContent = msg;
      this.bannerEl.hidden = false;
      // Restart the pop-in animation.
      this.bannerEl.style.animation = "none";
      void this.bannerEl.offsetWidth;
      this.bannerEl.style.animation = "";
      void this.sleep(2600).then(next);
    };
    next();
  }

  /// A short-lived emoji rising over `host` (a character).
  popOver(host: HTMLElement, glyph: string): void {
    const el = document.createElement("span");
    el.className = "office-pop";
    el.textContent = glyph;
    host.appendChild(el);
    void this.sleep(1600).then(() => el.remove());
  }

  /// `n` bits of paper falling over the floor.
  paperRain(n: number): void {
    const layer = document.createElement("div");
    layer.className = "office-rain";
    for (let i = 0; i < n; i++) {
      const bit = document.createElement("i");
      bit.style.left = `${Math.random() * 100}%`;
      bit.style.animationDelay = `${Math.random() * 0.8}s`;
      bit.style.setProperty("--hue", `${Math.floor(Math.random() * 360)}`);
      layer.appendChild(bit);
    }
    this.floor.appendChild(layer);
    void this.sleep(3200).then(() => layer.remove());
  }

  /// Bursts in the sky over the floor.
  fireworks(): void {
    const layer = document.createElement("div");
    layer.className = "office-fireworks";
    for (let i = 0; i < 6; i++) {
      const burst = document.createElement("span");
      burst.className = "office-firework";
      burst.style.left = `${10 + Math.random() * 80}%`;
      burst.style.top = `${8 + Math.random() * 30}%`;
      burst.style.animationDelay = `${i * 0.35}s`;
      burst.style.setProperty("--hue", `${Math.floor(Math.random() * 360)}`);
      layer.appendChild(burst);
    }
    this.floor.appendChild(layer);
    void this.sleep(3600).then(() => layer.remove());
  }
}
