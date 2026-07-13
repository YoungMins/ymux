// Minimal Web Audio beep, used for lightweight alert cues without shipping
// an external audio asset.

/// Play a short (~120ms) sine tone as an audible alert. Fails silently (with
/// a console warning) if the Web Audio API is unavailable or blocked.
export function beep(): void {
  try {
    const ctx = new AudioContext();
    const oscillator = ctx.createOscillator();
    const gain = ctx.createGain();

    oscillator.type = "sine";
    oscillator.frequency.setValueAtTime(660, ctx.currentTime);

    const duration = 0.12;
    gain.gain.setValueAtTime(0.15, ctx.currentTime);
    gain.gain.exponentialRampToValueAtTime(0.0001, ctx.currentTime + duration);

    oscillator.connect(gain);
    gain.connect(ctx.destination);

    oscillator.start();
    oscillator.stop(ctx.currentTime + duration);
    oscillator.onended = () => {
      void ctx.close();
    };
  } catch (e) {
    console.warn("beep: failed to play tone", e);
  }
}
