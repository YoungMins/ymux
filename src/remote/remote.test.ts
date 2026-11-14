import { describe, it, expect, vi } from "vitest";
import { newPane, paneNode } from "../layout/LayoutTree";
import type { AgentSnapshot, LayoutNode, PaneSpec, RemotePane } from "../types";
import type { PaneStatus } from "../terminal/paneStatus";
import type { AgentPhase } from "../workspace/promptQueue";
import {
  buildPaneList,
  countdown,
  errorKey,
  labelMatches,
  mapSendResult,
  shouldPublish,
  statusLine,
  type RemoteListInput,
} from "./remoteModel";
import { bumpInput, inputGeneration } from "./inputGen";
import { MenuIds, agentKey } from "./menuId";
import { handleRequest, type RemoteHost, type RemoteTerminal } from "./remoteOps";
import { createPublisher } from "./remotePublisher";

function split(a: LayoutNode, b: LayoutNode): LayoutNode {
  return { kind: "split", direction: "horizontal", ratio: 0.5, a, b };
}

function term(id: string): PaneSpec {
  return { ...newPane("bash"), id };
}

function input(over: Partial<RemoteListInput> & { specs: PaneSpec[]; agents?: AgentSnapshot }): RemoteListInput {
  const nodes = over.specs.map(paneNode);
  const root = nodes.slice(1).reduce((acc, n) => split(acc, n), nodes[0]);
  return {
    workspaces: [{ label: "1: build", root }],
    agents: over.agents ?? {},
    paneStatus: over.paneStatus ?? (() => undefined),
    title: over.title ?? ((id) => `t-${id}`),
    since: over.since ?? (() => null),
    queued: over.queued ?? (() => 0),
    live: over.live ?? (() => true),
  };
}

const claude = (status: "working" | "waiting" | "done" | "idle", source: "hook" | "process" = "hook") => ({
  lead: { kind: "claude", status, source, tool: null },
  subagents: [],
});

describe("buildPaneList", () => {
  it("reports each lead's status, kind, since and queue length", () => {
    const list = buildPaneList(
      input({
        specs: [term("a"), term("b"), term("c")],
        agents: { a: claude("working"), b: claude("done") },
        since: (id) => (id === "a" ? 1234 : null),
        queued: (id) => (id === "b" ? 2 : 0),
      }),
    );
    expect(list).toEqual<RemotePane[]>([
      { pane: "a", ws: "1: build", title: "t-a", kind: "claude", status: "working", since_ms: 1234, queued: 0 },
      { pane: "b", ws: "1: build", title: "t-b", kind: "claude", status: "done", since_ms: 0, queued: 2 },
      { pane: "c", ws: "1: build", title: "t-c", kind: "", status: "none", since_ms: 0, queued: 0 },
    ]);
  });

  it("treats a pane flagged attention as waiting, and a process lead by the pane's status", () => {
    const ps: Record<string, PaneStatus> = { a: "attention", b: "running", c: "done" };
    const list = buildPaneList(
      input({
        specs: [term("a"), term("b"), term("c")],
        agents: { a: claude("working"), b: claude("idle", "process"), c: claude("idle", "process") },
        paneStatus: (id) => ps[id],
      }),
    );
    expect(list.map((p) => p.status)).toEqual(["waiting", "working", "done"]);
  });

  it("leaves out non-terminal panes and unhydrated terminals without an agent", () => {
    const browser: PaneSpec = { ...term("br"), pane_kind: "browser" };
    const list = buildPaneList(
      input({
        specs: [term("a"), browser, term("cold"), term("coldAgent")],
        agents: { coldAgent: claude("idle") },
        live: (id) => id === "a",
      }),
    );
    expect(list.map((p) => p.pane)).toEqual(["a", "coldAgent"]);
  });
});

describe("result tables", () => {
  it("maps sendToPane results onto the protocol's", () => {
    expect(mapSendResult("sent")).toBe("sent");
    expect(mapSendResult("waiting")).toBe("blocked");
    expect(mapSendResult("gone")).toBe("gone");
    expect(mapSendResult("pasted")).toBe("failed");
    expect(mapSendResult("no-paste")).toBe("failed");
    expect(mapSendResult("failed")).toBe("failed");
  });

  it("matches a clipped label only as a prefix of a long one", () => {
    expect(labelMatches("Yes", "Yes")).toBe(true);
    expect(labelMatches("Yes, and don't ask again", "Yes")).toBe(false);
    const long = "Yes, allow this command to run in the sandbox for the rest of the session";
    expect(labelMatches(long, `${long.slice(0, 40)}…`)).toBe(true);
    expect(labelMatches(long, "Yes…")).toBe(false);
  });
});

describe("status line", () => {
  it("maps error codes to message keys", () => {
    for (const c of ["bad_token", "key_mismatch", "version", "bad_sig", "limit", "origin_mismatch", "secret_store"]) {
      expect(errorKey(c)).toBe(`err.${c}`);
    }
    expect(statusLine({ state: "error", error: "origin_mismatch" })).toMatchObject({
      key: "err.origin_mismatch",
      tone: "error",
    });
    expect(errorKey("dns failure")).toBe("err.other");
    expect(errorKey("")).toBe("err.other");
  });

  it("picks key and tone per state", () => {
    expect(statusLine({ state: "off", error: "" })).toMatchObject({ key: "state.off", tone: "" });
    expect(statusLine({ state: "connecting", error: "" }).key).toBe("state.connecting");
    expect(statusLine({ state: "online", error: "" })).toMatchObject({ key: "state.online", tone: "ok" });
    expect(statusLine({ state: "error", error: "bad_token" })).toMatchObject({ key: "err.bad_token", tone: "error" });
    expect(statusLine({ state: "error", error: " boom " })).toMatchObject({ key: "err.other", detail: "boom" });
    expect(statusLine({ state: "unavailable", error: "" }).key).toBe("state.unavailable");
  });

  it("publishes only while a server is, or is about to be, there", () => {
    expect(shouldPublish("online")).toBe(true);
    expect(shouldPublish("connecting")).toBe(true);
    for (const s of ["off", "error", "unavailable", undefined] as const) expect(shouldPublish(s)).toBe(false);
  });

  it("formats the pairing countdown", () => {
    expect(countdown(125_000, 0)).toBe("2:05");
    expect(countdown(1000, 5000)).toBe("0:00");
  });
});

// ── Ops ────────────────────────────────────────────────────────────────────

const MENU = ["Do you want to proceed?", "❯ 1. Yes", "  2. Yes, and don't ask again", "  3. No, and tell Claude what to do differently"];

/// A terminal whose `typeText` behaves like the real one: it goes through
/// `api.writePane`, which advances the pane's input generation first.
function fakeTerm(
  lines: string[],
  block: string | null = null,
  logical?: string[],
  onType?: (s: string) => void,
): RemoteTerminal & { typed: string[]; lines: string[] } {
  const typed: string[] = [];
  const t = {
    typed,
    lines,
    screenText: () => t.lines,
    screenLogicalText: () => logical ?? t.lines,
    screenBlock: () => block,
    typeText: (s: string) => {
      onType?.(s);
      bumpInput("a");
      typed.push(s);
    },
  };
  return t;
}

let mintCount = 0;
const mint = () => `id${++mintCount}`;

/// The agent state the fake host reports (the raw lead status and tool).
interface FakeAgent {
  status: "working" | "waiting" | "done" | "idle";
  tool: string | null;
}

function fakeHost(opts: {
  terminals?: Record<string, RemoteTerminal>;
  phase?: AgentPhase;
  queued?: number;
  enqueueOk?: boolean;
  send?: "sent" | "pasted" | "waiting" | "no-paste" | "gone" | "failed";
  status?: "working" | "waiting" | "done" | "idle";
  menus?: MenuIds;
  /// Live agent state, changed by a test between calls.
  agent?: FakeAgent;
  /// The generations cannot be read.
  noGens?: boolean;
}): RemoteHost & { sent: string[]; enqueued: string[] } {
  const sent: string[] = [];
  const enqueued: string[] = [];
  const agent = (): FakeAgent => opts.agent ?? { status: opts.status ?? "waiting", tool: null };
  return {
    sent,
    enqueued,
    menus: opts.menus ?? new MenuIds(mint),
    inputGen: (id) => (opts.noGens ? null : inputGeneration(id)),
    agentKey: () =>
      opts.noGens
        ? null
        : agentKey({ kind: "claude", status: agent().status, source: "hook", tool: agent().tool }),
    listInput: () => input({ specs: [term("a")], agents: { a: claude(agent().status) } }),
    terminal: (id) => opts.terminals?.[id] ?? null,
    phase: () => opts.phase ?? "ready",
    queued: () => opts.queued ?? 0,
    enqueue: (_id, text) => {
      if (opts.enqueueOk === false) return false;
      enqueued.push(text);
      return true;
    },
    send: async (_id, text, notAfterMs) => {
      sent.push(text);
      deadlines.push(notAfterMs);
      return opts.send ?? "sent";
    },
  };
}
const deadlines: (number | undefined)[] = [];

const req = (r: Record<string, unknown>) => ({ id: 1, ...r }) as never;

/// What a phone does first: peek, and take the menu id from the reply.
async function menuIdOf(host: RemoteHost): Promise<string> {
  const r = (await handleRequest(host, req({ op: "peek", pane: "a" }))) as { menu_id: string };
  return r.menu_id;
}

const ans = (host: RemoteHost, menu_id: string, n: number, label: string) =>
  handleRequest(host, req({ op: "answer", pane: "a", n, label, menu_id }));

describe("answer", () => {
  it("types the option number when that option is still up and the menu id is current", async () => {
    const t = fakeTerm(MENU);
    const host = fakeHost({ terminals: { a: t } });
    const id = await menuIdOf(host);
    expect(id).not.toBe("");
    expect(await ans(host, id, 2, "Yes, and don't ask again")).toEqual({ result: "sent" });
    expect(t.typed).toEqual(["2"]);
  });

  it("is stale when the label or number moved, nothing typed", async () => {
    const t = fakeTerm(MENU);
    const host = fakeHost({ terminals: { a: t } });
    const id = await menuIdOf(host);
    expect(await ans(host, id, 2, "No")).toEqual({ result: "stale" });
    expect(await ans(host, id, 9, "Yes")).toEqual({ result: "stale" });
    expect(t.typed).toEqual([]);
  });

  it("is stale for a missing, empty or wrong menu id", async () => {
    const t = fakeTerm(MENU);
    const host = fakeHost({ terminals: { a: t } });
    await menuIdOf(host);
    expect(await handleRequest(host, req({ op: "answer", pane: "a", n: 1, label: "Yes" }))).toEqual({ result: "stale" });
    expect(await ans(host, "", 1, "Yes")).toEqual({ result: "stale" });
    expect(await ans(host, "not-the-id", 1, "Yes")).toEqual({ result: "stale" });
    expect(t.typed).toEqual([]);
  });

  it("is stale for an id minted for an earlier menu with the same options and wording", async () => {
    // The reviews' top finding: a held "1 / Yes" arriving at a later question.
    const t = fakeTerm(MENU);
    const host = fakeHost({ terminals: { a: t }, status: "waiting" });
    const held = await menuIdOf(host);
    // The agent works on, then asks the very same question again.
    t.lines = ["working..."];
    expect(await menuIdOf(host)).toBe("");
    t.lines = MENU;
    const fresh = await menuIdOf(host);
    expect(fresh).not.toBe(held);
    expect(await ans(host, held, 1, "Yes")).toEqual({ result: "stale" });
    expect(t.typed).toEqual([]);
    expect(await ans(host, fresh, 1, "Yes")).toEqual({ result: "sent" });
  });

  it("is stale for an id from before the agent's status moved, even for an identical menu", async () => {
    const t = fakeTerm(MENU);
    const menus = new MenuIds(mint);
    const agent: FakeAgent = { status: "waiting", tool: null };
    const host = fakeHost({ terminals: { a: t }, menus, agent });
    const held = await menuIdOf(host);
    // The snapshot feed sees the lead go to working and back to waiting; nobody
    // peeks in between, and the menu text on screen is the same again.
    menus.observeAgents({ a: claude("working") });
    menus.observeAgents({ a: claude("waiting") });
    expect(await ans(host, held, 1, "Yes")).toEqual({ result: "stale" });
    expect(t.typed).toEqual([]);
  });

  it("is stale when the status or the tool changed since the peek, with no feed at all", async () => {
    for (const change of [
      (a: FakeAgent) => (a.status = "working"),
      (a: FakeAgent) => (a.tool = "Bash"),
    ]) {
      const t = fakeTerm(MENU);
      const agent: FakeAgent = { status: "waiting", tool: null };
      const host = fakeHost({ terminals: { a: t }, agent });
      const held = await menuIdOf(host);
      change(agent);
      expect(await ans(host, held, 1, "Yes")).toEqual({ result: "stale" });
      expect(t.typed).toEqual([]);
    }
  });

  it("is stale for an id from before anything was written to the pane", async () => {
    const t = fakeTerm(MENU);
    const host = fakeHost({ terminals: { a: t } });
    const held = await menuIdOf(host);
    // The user types, pastes, the queue pumps or another device answers: any
    // write through the one choke point advances the generation.
    bumpInput("a");
    expect(await ans(host, held, 1, "Yes")).toEqual({ result: "stale" });
    expect(t.typed).toEqual([]);
    // A fresh peek binds a new id to the new generation.
    const fresh = await menuIdOf(host);
    expect(fresh).not.toBe(held);
    expect(await ans(host, fresh, 1, "Yes")).toEqual({ result: "sent" });
  });

  it("mints a new id when only the input generation moved", async () => {
    const host = fakeHost({ terminals: { a: fakeTerm(MENU) } });
    const first = await menuIdOf(host);
    expect(await menuIdOf(host)).toBe(first);
    bumpInput("a");
    expect(await menuIdOf(host)).not.toBe(first);
  });

  it("binds the id to the whole menu block, not to its last three lines", async () => {
    // Two questions with the same options and the same lines right above them,
    // told apart only by the command named further up.
    const screen = (cmd: string) => [
      `● Bash(${cmd})`,
      "  Run the build step",
      "  in the project root",
      "  Do you want to proceed?",
      "❯ 1. Yes",
      "  2. No",
    ];
    const t = fakeTerm(screen("rm -rf build"));
    const host = fakeHost({ terminals: { a: t } });
    const first = await menuIdOf(host);
    expect(await menuIdOf(host)).toBe(first);
    t.lines = screen("ls");
    const second = await menuIdOf(host);
    expect(second).not.toBe(first);
    // The held id for the first question cannot answer the second.
    expect(await ans(host, first, 1, "Yes")).toEqual({ result: "stale" });
    expect(t.typed).toEqual([]);
    expect(await ans(host, second, 1, "Yes")).toEqual({ result: "sent" });
  });

  it("retires the id before the key is typed", async () => {
    const menus = new MenuIds(mint);
    const seenAtType: string[] = [];
    const t = fakeTerm(MENU, null, undefined, () => seenAtType.push(menus.current("a")));
    const host = fakeHost({ terminals: { a: t }, menus });
    const id = await menuIdOf(host);
    expect(menus.current("a")).toBe(id);
    expect(await ans(host, id, 1, "Yes")).toEqual({ result: "sent" });
    expect(seenAtType).toEqual([""]);
  });

  it("fails closed: no menu, no generations, a terminal that is not up", async () => {
    // No menu on screen at all.
    const t = fakeTerm(MENU);
    const host = fakeHost({ terminals: { a: t } });
    const held = await menuIdOf(host);
    t.lines = ["$ ls"];
    expect(await ans(host, held, 1, "Yes")).toEqual({ result: "stale" });
    // The generations cannot be read: the menu shows, but with no id, and an
    // answer is stale whatever it carries.
    const t2 = fakeTerm(MENU);
    const blind = fakeHost({ terminals: { a: t2 }, noGens: true });
    const peeked = (await handleRequest(blind, req({ op: "peek", pane: "a" }))) as {
      menu: unknown[];
      menu_id: string;
    };
    expect(peeked.menu).toHaveLength(3);
    expect(peeked.menu_id).toBe("");
    expect(await ans(blind, "anything", 1, "Yes")).toEqual({ result: "stale" });
    expect(await ans(blind, held, 1, "Yes")).toEqual({ result: "stale" });
    // The pane is listed (an agent runs there) but its terminal is not up yet.
    const cold = fakeHost({});
    expect(await ans(cold, "x", 1, "Yes")).toEqual({ result: "stale" });
    expect(t.typed).toEqual([]);
    expect(t2.typed).toEqual([]);
  });

  it("retires the id after a sent answer: a second answer with it is stale", async () => {
    const t = fakeTerm(MENU);
    const host = fakeHost({ terminals: { a: t } });
    const id = await menuIdOf(host);
    expect(await ans(host, id, 1, "Yes")).toEqual({ result: "sent" });
    expect(await ans(host, id, 1, "Yes")).toEqual({ result: "stale" });
    // The menu is still painted for a moment: a peek must not hand out a fresh
    // id for it, or the same click could be typed twice.
    expect(await menuIdOf(host)).toBe("");
    expect(t.typed).toEqual(["1"]);
  });

  it("refuses options above 9 and unknown numbers without typing", async () => {
    const lines = ["Pick one", "❯ 1. one", ...[2, 3, 4, 5, 6, 7, 8, 9, 10, 11].map((n) => `  ${n}. choice ${n}`)];
    const t = fakeTerm(lines);
    const host = fakeHost({ terminals: { a: t } });
    const id = await menuIdOf(host);
    expect(await ans(host, id, 10, "choice 10")).toEqual({ result: "failed" });
    expect(await ans(host, id, 11, "choice 11")).toEqual({ result: "failed" });
    expect(await ans(host, id, 0, "x")).toEqual({ result: "failed" });
    expect(await ans(host, id, 1.5, "x")).toEqual({ result: "failed" });
    expect(t.typed).toEqual([]);
    // 9 is fine.
    expect(await ans(host, id, 9, "choice 9")).toEqual({ result: "sent" });
    expect(t.typed).toEqual(["9"]);
  });

  // The agreed rule (PROTOCOL.md 5.2): the phone echoes the label as the desktop
  // sent it (redacted, then clipped to 120 chars = 119 + "…"). Unchanged and
  // clipped copies match the screen; a redaction-altered one is stale.
  it("matches an unchanged, a 120-char clipped and a redacted label as sent/sent/stale", async () => {
    const long = `Yes, and allow \`npm run build -- --token ${"ghp_" + "a1B2c3D4e5F6g7H8i9J0".repeat(2)}\` for this session ${"x".repeat(100)}`;
    const clipped = `${[...long].slice(0, 119).join("")}…`;
    expect([...clipped].length).toBe(120);
    const screen = ["Do you want to proceed?", "❯ 1. Yes", `  2. ${long}`];
    const t = fakeTerm(screen);
    const host = fakeHost({ terminals: { a: t } });
    const id = await menuIdOf(host);
    // unchanged short label
    expect(await ans(host, id, 1, "Yes")).toEqual({ result: "sent" });
    // the clip the desktop produces when nothing was redacted inside the first 119 chars
    const plain = `Yes, and allow this exact command for the rest of the session ${"y".repeat(100)}`;
    const t2 = fakeTerm(["Do you want to proceed?", "❯ 1. Yes", `  2. ${plain}`]);
    const clip2 = `${[...plain].slice(0, 119).join("")}…`;
    const host2 = fakeHost({ terminals: { a: t2 } });
    expect(await ans(host2, await menuIdOf(host2), 2, clip2)).toEqual({ result: "sent" });
    expect(t2.typed).toEqual(["2"]);
    // the secret in the screen label was replaced on the way out: the echo no longer matches
    const redacted = `${[...long.replace(/ghp_[A-Za-z0-9]+/, "[REDACTED]")].slice(0, 119).join("")}…`;
    const t3 = fakeTerm(screen);
    const host3 = fakeHost({ terminals: { a: t3 } });
    expect(await ans(host3, await menuIdOf(host3), 2, redacted)).toEqual({ result: "stale" });
    expect(t3.typed).toEqual([]);
    expect(t.typed).toEqual(["1"]);
  });

  it("refuses options that open a text field", async () => {
    const t = fakeTerm(MENU);
    const host = fakeHost({ terminals: { a: t } });
    const r = await ans(host, await menuIdOf(host), 3, "No, and tell Claude what to do differently");
    expect(r).toEqual({ result: "failed" });
    expect(t.typed).toEqual([]);
  });

  it("is gone for a pane nobody knows", async () => {
    const r = await handleRequest(
      fakeHost({}),
      req({ op: "answer", pane: "zz", n: 1, label: "Yes", menu_id: "x" }),
    );
    expect(r).toEqual({ result: "gone" });
  });
});

describe("interrupt / peek / list", () => {
  it("sends exactly one Escape", async () => {
    const t = fakeTerm([]);
    const host = fakeHost({ terminals: { a: t } });
    expect(await handleRequest(host, req({ op: "interrupt", pane: "a" }))).toEqual({ result: "sent" });
    expect(t.typed).toEqual(["\x1b"]);
    expect(await handleRequest(host, req({ op: "interrupt", pane: "zz" }))).toEqual({ result: "gone" });
  });

  it("sends no Escape to a pane that has no agent", async () => {
    const t = fakeTerm(["~/proj $ "]);
    const shell = fakeHost({ terminals: { a: t }, phase: "none" });
    expect(await handleRequest(shell, req({ op: "interrupt", pane: "a" }))).toEqual({ result: "gone" });
    expect(t.typed).toEqual([]);
    // A working agent is interrupted.
    const working = fakeHost({ terminals: { a: t }, phase: "working" });
    expect(await handleRequest(working, req({ op: "interrupt", pane: "a" }))).toEqual({ result: "sent" });
    expect(t.typed).toEqual(["\x1b"]);
  });

  it("peeks lines, menu and status; an absent pane is empty", async () => {
    const lines = ["a", "b", ...MENU];
    const host = fakeHost({ terminals: { a: fakeTerm(lines, "trust") } });
    const r = (await handleRequest(host, req({ op: "peek", pane: "a", lines: 3 }))) as {
      lines: string[];
      menu: { n: number; label: string }[];
      menu_id: string;
      blocked: string | null;
      status: string;
    };
    expect(r.lines).toEqual(lines.slice(-3));
    expect(r.menu_id).toMatch(/^[A-Za-z0-9_-]{1,32}$/);
    expect(r.menu.map((o) => o.n)).toEqual([1, 2, 3]);
    expect(r.blocked).toBe("trust");
    expect(r.status).toBe("waiting");
    expect(await handleRequest(host, req({ op: "peek", pane: "zz" }))).toMatchObject({
      lines: [],
      menu: [],
      status: "none",
    });
  });

  it("sends logical lines, but finds the menu on terminal rows", async () => {
    // A long line the terminal wrapped into two rows; the secret straddles the wrap.
    const rows = ["token sk-ant-api03-abc", "defghijkl done", ...MENU];
    const logical = ["token sk-ant-api03-abcdefghijkl done", ...MENU];
    const host = fakeHost({ terminals: { a: fakeTerm(rows, null, logical) } });
    const r = (await handleRequest(host, req({ op: "peek", pane: "a", lines: 80 }))) as {
      lines: string[];
      menu: { n: number }[];
    };
    expect(r.lines).toEqual(logical);
    expect(r.menu.map((o) => o.n)).toEqual([1, 2, 3]);
    // `lines` counts logical lines, from the end.
    const two = (await handleRequest(host, req({ op: "peek", pane: "a", lines: 2 }))) as { lines: string[] };
    expect(two.lines).toEqual(logical.slice(-2));
  });

  it("has an empty menu id when no menu is on screen", async () => {
    const host = fakeHost({ terminals: { a: fakeTerm(["$ ls", "file.txt"]) } });
    const r = (await handleRequest(host, req({ op: "peek", pane: "a" }))) as { menu: unknown[]; menu_id: string };
    expect(r.menu).toEqual([]);
    expect(r.menu_id).toBe("");
  });

  it("keeps the id while the menu is unchanged and replaces it when the text changes", async () => {
    const t = fakeTerm(MENU);
    const host = fakeHost({ terminals: { a: t } });
    const first = await menuIdOf(host);
    expect(await menuIdOf(host)).toBe(first);
    t.lines = ["Allow this other thing?", "❯ 1. Yes", "  2. No"];
    const second = await menuIdOf(host);
    expect(second).not.toBe("");
    expect(second).not.toBe(first);
    // A changed option label is a different menu, too.
    t.lines = ["Allow this other thing?", "❯ 1. Yes", "  2. Nope"];
    expect(await menuIdOf(host)).not.toBe(second);
  });

  it("lists, and answers the failure shape when the host throws", async () => {
    const host = fakeHost({});
    const ok = (await handleRequest(host, req({ op: "list" }))) as { panes: unknown[] };
    expect(ok.panes).toHaveLength(1);
    const bad: RemoteHost = {
      ...host,
      terminal: () => {
        throw new Error("boom");
      },
    };
    vi.spyOn(console, "warn").mockImplementation(() => {});
    expect(await handleRequest(bad, req({ op: "interrupt", pane: "a" }))).toEqual({ result: "failed" });
    expect(await handleRequest(bad, req({ op: "peek", pane: "a" }))).toMatchObject({ lines: [], status: "none" });
  });
});

describe("prompt", () => {
  const T = () => ({ a: fakeTerm([]) });
  const p = (host: RemoteHost, queue = false, text = "do it") =>
    handleRequest(host, req({ op: "prompt", pane: "a", text, queue }));

  it("sends to a ready agent", async () => {
    const host = fakeHost({ terminals: T() });
    expect(await p(host)).toEqual({ result: "sent" });
    expect(host.sent).toEqual(["do it"]);
  });

  it("hands the request's deadline to the send, so Enter is never pressed after it", async () => {
    deadlines.length = 0;
    const host = fakeHost({ terminals: T() });
    const future = Date.now() + 5000;
    expect(
      await handleRequest(host, req({ op: "prompt", pane: "a", text: "go", queue: false, deadline_ms: future })),
    ).toEqual({ result: "sent" });
    expect(deadlines).toEqual([future]);
    // A request with no deadline (an older backend) sends with none.
    await p(host);
    expect(deadlines).toEqual([future, undefined]);
  });

  it("types nothing at all when the deadline has already passed", async () => {
    const host = fakeHost({ terminals: T() });
    const past = Date.now() - 1;
    expect(
      await handleRequest(host, req({ op: "prompt", pane: "a", text: "go", queue: false, deadline_ms: past })),
    ).toEqual({ result: "failed" });
    expect(host.sent).toEqual([]);
  });

  it("maps the delivery result", async () => {
    expect(await p(fakeHost({ terminals: T(), send: "waiting" }))).toEqual({ result: "blocked" });
    expect(await p(fakeHost({ terminals: T(), send: "pasted" }))).toEqual({ result: "failed" });
  });

  it("queues for a working agent only when asked", async () => {
    const host = fakeHost({ terminals: T(), phase: "working" });
    expect(await p(host, true)).toEqual({ result: "queued" });
    expect(host.enqueued).toEqual(["do it"]);
    expect(await p(host, false)).toEqual({ result: "waiting" });
    expect(host.sent).toEqual([]);
  });

  it("is failed when the queue is full, and keeps order behind a queue", async () => {
    expect(await p(fakeHost({ terminals: T(), phase: "working", enqueueOk: false }), true)).toEqual({ result: "failed" });
    const behind = fakeHost({ terminals: T(), queued: 1 });
    expect(await p(behind, true)).toEqual({ result: "queued" });
    expect(await p(behind, false)).toEqual({ result: "waiting" });
    expect(behind.sent).toEqual([]);
  });

  it("is blocked by a dialog or a permission prompt, gone without a pane or agent", async () => {
    expect(await p(fakeHost({ terminals: { a: fakeTerm([], "trust") } }))).toEqual({ result: "blocked" });
    expect(await p(fakeHost({ terminals: T(), phase: "waiting" }))).toEqual({ result: "blocked" });
    expect(await p(fakeHost({}))).toEqual({ result: "gone" });
    expect(await p(fakeHost({ terminals: T(), phase: "none" }))).toEqual({ result: "gone" });
  });

  it("rejects empty and oversized text", async () => {
    const host = fakeHost({ terminals: T() });
    expect(await p(host, false, "  ")).toEqual({ result: "failed" });
    expect(await p(host, false, "x".repeat(16001))).toEqual({ result: "failed" });
    expect(host.sent).toEqual([]);
  });
});

// ── Publisher ──────────────────────────────────────────────────────────────

describe("publisher", () => {
  function rig(active = true) {
    let fn: (() => void) | null = null;
    let panes: RemotePane[] = [];
    const sent: RemotePane[][] = [];
    const state = { active };
    const pub = createPublisher({
      compute: () => panes,
      send: async (p) => void sent.push(p),
      active: () => state.active,
      setTimer: (f) => {
        fn = f;
        return 1;
      },
      clearTimer: () => {
        fn = null;
      },
    });
    const flush = (): void => {
      const f = fn;
      fn = null;
      f?.();
    };
    return {
      pub,
      sent,
      state,
      flush,
      set: (p: RemotePane[]) => (panes = p),
      pending: () => fn !== null,
    };
  }
  const row = (status: RemotePane["status"]): RemotePane => ({
    pane: "a",
    ws: "1",
    title: "t",
    kind: "claude",
    status,
    since_ms: 0,
    queued: 0,
  });

  it("coalesces pokes into one send and skips an identical list", () => {
    const r = rig();
    r.set([row("working")]);
    r.pub.poke();
    r.pub.poke();
    r.pub.poke();
    r.flush();
    expect(r.sent).toHaveLength(1);
    r.pub.poke();
    r.flush();
    expect(r.sent).toHaveLength(1);
    r.set([row("done")]);
    r.pub.poke();
    r.flush();
    expect(r.sent).toHaveLength(2);
  });

  it("sends nothing while inactive, and resends after invalidate", () => {
    const r = rig(false);
    r.set([row("idle")]);
    r.pub.poke();
    r.flush();
    expect(r.sent).toHaveLength(0);
    r.state.active = true;
    r.pub.poke();
    r.flush();
    expect(r.sent).toHaveLength(1);
    r.pub.invalidate();
    r.flush();
    expect(r.sent).toHaveLength(2);
  });

  it("stops after dispose", () => {
    const r = rig();
    r.pub.dispose();
    r.pub.poke();
    expect(r.pending()).toBe(false);
  });
});
