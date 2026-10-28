// Little 8-bit sounds for the office games, made with Web Audio square
// waves (no audio files to ship). Quiet, short, and silent when the audio
// context cannot start.

export type Chime = "unlock" | "coin" | "boss" | "levelup" | "quest" | "party";

/// Notes as [frequency Hz, start s, length s].
const TUNES: Readonly<Record<Chime, readonly [number, number, number][]>> = {
  unlock: [[523, 0, 0.08], [659, 0.08, 0.08], [784, 0.16, 0.08], [1047, 0.24, 0.16]],
  coin: [[988, 0, 0.06], [1319, 0.06, 0.14]],
  boss: [[196, 0, 0.12], [147, 0.12, 0.12], [392, 0.26, 0.1], [523, 0.36, 0.2]],
  levelup: [[392, 0, 0.1], [523, 0.1, 0.1], [659, 0.2, 0.1], [784, 0.3, 0.1], [1047, 0.4, 0.25]],
  quest: [[659, 0, 0.08], [880, 0.08, 0.16]],
  party: [[523, 0, 0.08], [523, 0.1, 0.08], [784, 0.2, 0.08], [659, 0.3, 0.08], [1047, 0.4, 0.2]],
};

let ctx: AudioContext | null = null;

export function chime(kind: Chime, volume = 0.05): void {
  try {
    ctx ??= new AudioContext();
    const now = ctx.currentTime;
    for (const [f, start, len] of TUNES[kind]) {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = "square";
      osc.frequency.setValueAtTime(f, now + start);
      gain.gain.setValueAtTime(volume, now + start);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + start + len);
      osc.connect(gain);
      gain.connect(ctx.destination);
      osc.start(now + start);
      osc.stop(now + start + len);
    }
  } catch {
    /* no audio: the games are just quiet */
  }
}

/// How long a tune plays, in seconds (for tests and spacing).
export function tuneLength(kind: Chime): number {
  return Math.max(...TUNES[kind].map(([, start, len]) => start + len));
}
