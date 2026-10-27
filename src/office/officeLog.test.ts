import { describe, expect, it } from "vitest";
import { LOG_LIMIT, MAIL_LIMIT, OfficeLog, snippetOf } from "./officeLog";

const msg = (to = "p", kind: "prompt" | "review" | "commit" = "prompt") => ({
  kind,
  to,
  label: "web-ui",
  agent: "claude",
  from: null,
  fromLabel: null,
});

describe("OfficeLog", () => {
  it("keeps notes newest first and drops the oldest past the limit", () => {
    let t = 0;
    const log = new OfficeLog(() => ++t);
    for (let i = 0; i < LOG_LIMIT + 5; i++) log.done(`p${i}`, `pane ${i}`, "claude");
    const list = log.list();
    expect(list).toHaveLength(LOG_LIMIT);
    expect(list[0].paneId).toBe(`p${LOG_LIMIT + 4}`);
    expect(list[0].at).toBeGreaterThan(list[1].at);
  });

  it("delivers a sealed letter, opens it when the agent starts, files it when the turn ends", () => {
    const log = new OfficeLog();
    log.message(msg(), "fix the login bug\nand add a test");
    expect(log.mailOf("p")).toMatchObject([{ kind: "prompt", snippet: "fix the login bug", opened: false }]);
    expect(log.list()[0]).toMatchObject({ kind: "prompt", paneId: "p", label: "web-ui" });
    log.agentWorking("p");
    expect(log.mailOf("p")[0].opened).toBe(true);
    // A second letter arrives mid-turn: it stays sealed when the turn ends.
    log.message(msg("p", "review"), "nit: rename");
    log.agentStopped("p");
    expect(log.mailOf("p")).toMatchObject([{ kind: "review", opened: false }]);
    log.agentWorking("p");
    log.agentStopped("p");
    expect(log.mailOf("p")).toEqual([]);
  });

  it("caps a mailbox and empties it when the pane goes away", () => {
    const log = new OfficeLog();
    for (let i = 0; i < MAIL_LIMIT + 3; i++) log.message(msg(), `letter ${i}`);
    expect(log.mailOf("p")).toHaveLength(MAIL_LIMIT);
    expect(log.mailOf("p")[0].snippet).toBe(`letter ${MAIL_LIMIT + 2}`);
    log.clearMail("p");
    expect(log.mailOf("p")).toEqual([]);
  });

  it("tells listeners once per change and only when something changed", () => {
    const log = new OfficeLog();
    let calls = 0;
    const off = log.onChange(() => calls++);
    log.message(msg(), "x");
    log.agentStopped("p"); // nothing opened yet: no change
    log.agentWorking("p");
    log.agentWorking("p"); // already open: no change
    log.done("p", "web-ui", "claude");
    expect(calls).toBe(3);
    off();
    log.clearMail("p");
    expect(calls).toBe(3);
  });

  it("snips the first non-empty line", () => {
    expect(snippetOf("\n\n   hello   world  \nsecond")).toBe("hello world");
    expect(snippetOf("x".repeat(100))).toHaveLength(60);
    expect(snippetOf("")).toBe("");
  });
});
