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

  it("running -> done on attention while focused, then idle", () => {
    const seen: string[] = [];
    const m = new PaneStatusMachine((s) => seen.push(s));
    m.onSubmit(0);
    m.onAttention(true); // focused
    expect(m.status).toBe("done");
    expect(seen).toContain("done");
  });

  it("attention when bell arrives while unfocused", () => {
    const m = new PaneStatusMachine(() => {});
    m.onSubmit(0);
    m.onAttention(false); // unfocused
    expect(m.status).toBe("attention");
  });

  it("attention clears to idle on focus", () => {
    const m = new PaneStatusMachine(() => {});
    m.onAttention(false);
    expect(m.status).toBe("attention");
    m.onFocus();
    expect(m.status).toBe("idle");
  });

  it("done clears to idle on focus", () => {
    const m = new PaneStatusMachine(() => {});
    m.onSubmit(0);
    m.onAttention(true); // focused -> done
    expect(m.status).toBe("done");
    m.onFocus();
    expect(m.status).toBe("idle");
  });

  it("running -> idle after idle timeout with no output", () => {
    const m = new PaneStatusMachine(() => {}, 4000);
    m.onSubmit(1000);
    m.tick(2000); // still running (within window)
    expect(m.status).toBe("running");
    m.tick(6000); // > 4000ms since last activity
    expect(m.status).toBe("idle");
  });

  it("output refreshes the running window", () => {
    const m = new PaneStatusMachine(() => {}, 4000);
    m.onSubmit(0);
    m.onOutput(3000);
    m.tick(6000); // 3000ms since last output < 4000
    expect(m.status).toBe("running");
  });
});
