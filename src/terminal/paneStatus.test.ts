import { describe, it, expect } from "vitest";
import { PaneStatusMachine } from "./paneStatus";

describe("PaneStatusMachine", () => {
  it("starts idle", () => {
    const m = new PaneStatusMachine(() => {});
    expect(m.status).toBe("idle");
  });

  it("idle -> running on submit", () => {
    const m = new PaneStatusMachine(() => {});
    m.onSubmit(0);
    expect(m.status).toBe("running");
  });

  it("running -> done on attention while watched", () => {
    const seen: string[] = [];
    const m = new PaneStatusMachine((s) => seen.push(s));
    m.onSubmit(0);
    m.onAttention(true, 0);
    expect(m.status).toBe("done");
    expect(seen).toContain("done");
  });

  it("attention when bell arrives while not watched", () => {
    const m = new PaneStatusMachine(() => {});
    m.onSubmit(0);
    m.onAttention(false, 0);
    expect(m.status).toBe("attention");
  });

  it("attention clears to idle on focus", () => {
    const m = new PaneStatusMachine(() => {});
    m.onAttention(false, 0);
    expect(m.status).toBe("attention");
    m.onFocus();
    expect(m.status).toBe("idle");
  });

  it("done clears to idle on focus", () => {
    const m = new PaneStatusMachine(() => {});
    m.onSubmit(0);
    m.onAttention(true, 0);
    expect(m.status).toBe("done");
    m.onFocus();
    expect(m.status).toBe("idle");
  });

  it("output refreshes the running window", () => {
    const m = new PaneStatusMachine(() => {}, 4000);
    m.onSubmit(0);
    m.onOutput(3000);
    m.tick(6000, false); // 3000ms since last output < 4000
    expect(m.status).toBe("running");
  });

  // ── Quiet-inferred completion ────────────────────────────────────
  // The signal most CLIs never send. Without this a command that simply
  // stops printing went straight back to `idle`, so the workspace row lost
  // its tint entirely and "finished" was never shown.

  it("running -> done once it goes quiet, not idle", () => {
    const m = new PaneStatusMachine(() => {}, 4000);
    m.onSubmit(1000);
    m.tick(2000, false); // still inside the window
    expect(m.status).toBe("running");
    m.tick(6000, false);
    expect(m.status).toBe("done");
  });

  it("holds an unwatched done indefinitely — it is an unread marker", () => {
    const m = new PaneStatusMachine(() => {}, 4000, 8000);
    m.onSubmit(0);
    m.tick(5000, false);
    expect(m.status).toBe("done");
    m.tick(60_000, false);
    expect(m.status).toBe("done");
    m.onFocus(); // the user finally looks at it
    expect(m.status).toBe("idle");
  });

  it("expires a watched done after the hold so green is not a resting state", () => {
    const m = new PaneStatusMachine(() => {}, 4000, 8000);
    m.onSubmit(0);
    m.tick(5000, true);
    expect(m.status).toBe("done");
    m.tick(12_000, true); // 7000ms held < 8000
    expect(m.status).toBe("done");
    m.tick(13_100, true); // > 8000ms held
    expect(m.status).toBe("idle");
  });

  it("keeps a done that stopped being watched before the hold elapsed", () => {
    const m = new PaneStatusMachine(() => {}, 4000, 8000);
    m.onSubmit(0);
    m.tick(5000, true);
    expect(m.status).toBe("done");
    // The user looked away before the hold ran out — it becomes unread again.
    m.tick(60_000, false);
    expect(m.status).toBe("done");
  });

  it("expires a bell-driven done too, so it does not stick while watched", () => {
    const m = new PaneStatusMachine(() => {}, 4000, 8000);
    m.onSubmit(0);
    m.onAttention(true, 0);
    expect(m.status).toBe("done");
    m.tick(9000, true);
    expect(m.status).toBe("idle");
  });

  it("never expires attention on a tick — only focus clears it", () => {
    const m = new PaneStatusMachine(() => {}, 4000, 8000);
    m.onAttention(false, 0);
    m.tick(60_000, true);
    expect(m.status).toBe("attention");
  });

  it("revokes a quiet-inferred done when output resumes", () => {
    const m = new PaneStatusMachine(() => {}, 4000);
    m.onSubmit(0);
    m.tick(5000, false);
    expect(m.status).toBe("done"); // just a long silent step, as it turns out
    m.onOutput(6000);
    expect(m.status).toBe("running");
    m.tick(7000, false);
    expect(m.status).toBe("running");
  });

  it("does not revoke a bell-driven done when the TUI repaints afterwards", () => {
    const m = new PaneStatusMachine(() => {}, 4000);
    m.onSubmit(0);
    m.onAttention(true, 0); // an explicit "I finished" signal
    m.onOutput(1000); // trailing spinner/prompt repaint
    expect(m.status).toBe("done");
  });
});
