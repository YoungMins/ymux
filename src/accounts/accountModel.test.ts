import { describe, expect, it } from "vitest";
import type { AccountProfile, Workspace } from "../types";
import { newPane, paneNode, splitPane, panes } from "../layout/LayoutTree";
import {
  ACCOUNT_NONE,
  clearAccountRefs,
  effectiveAccount,
  isAccountPending,
  newAccountId,
  nextAccountColor,
  nextAccountName,
  normalizeColor,
  agentUsesAccounts,
  isManagedAccount,
  ACCOUNT_COLORS,
} from "./accountModel";

const acct = (id: string): AccountProfile => ({
  id,
  name: `n-${id}`,
  claude_config_dir: "",
  codex_home: "",
  color: "",
});

describe("effectiveAccount", () => {
  const accounts = [acct("a"), acct("b")];
  it("pane choice wins over the workspace default", () => {
    expect(effectiveAccount(accounts, "b", "a")?.id).toBe("b");
  });
  it("empty pane account inherits the workspace", () => {
    expect(effectiveAccount(accounts, "", "a")?.id).toBe("a");
    expect(effectiveAccount(accounts, undefined, "a")?.id).toBe("a");
  });
  it("none opts out of the workspace default", () => {
    expect(effectiveAccount(accounts, ACCOUNT_NONE, "a")).toBeNull();
  });
  it("nothing selected, or an unknown id, is no account", () => {
    expect(effectiveAccount(accounts, "", "")).toBeNull();
    expect(effectiveAccount(accounts, "gone", "a")).toBeNull();
    expect(effectiveAccount(accounts, "", "gone")).toBeNull();
  });
});

describe("account ids and colours", () => {
  it("ids are 8 hex chars and never collide with the none sentinel", () => {
    const id = newAccountId();
    expect(id).toMatch(/^[0-9a-f]{8}$/);
    expect(id).not.toBe(ACCOUNT_NONE);
  });
  it("normalizeColor keeps valid #rrggbb only", () => {
    expect(normalizeColor(" #FF8800 ")).toBe("#ff8800");
    expect(normalizeColor("red")).toBe("");
    expect(normalizeColor("#fff")).toBe("");
  });
});

describe("clearAccountRefs", () => {
  it("resets panes (nested) and workspaces that referenced the account, leaving others", () => {
    const p1 = newPane("bash");
    p1.account = "a";
    const p2 = newPane("bash");
    p2.account = "b";
    const p3 = newPane("bash");
    p3.account = ACCOUNT_NONE;
    let root = paneNode(p1);
    root = splitPane(root, p1.id, "horizontal", p2);
    root = splitPane(root, p2.id, "vertical", p3);
    const ws: Workspace[] = [
      { id: 1, name: "one", root, account: "a" },
      { id: 2, name: "two", root: paneNode(newPane("bash")), account: "b" },
    ];
    expect(clearAccountRefs(ws, "a")).toBe(true);
    const got = Object.fromEntries(panes(ws[0].root).map((p) => [p.id, p.account]));
    expect(got[p1.id]).toBe("");
    expect(got[p2.id]).toBe("b");
    expect(got[p3.id]).toBe(ACCOUNT_NONE);
    expect(ws[0].account).toBe("");
    expect(ws[1].account).toBe("b");
    expect(clearAccountRefs(ws, "a")).toBe(false);
  });
});

describe("isAccountPending", () => {
  it("is false before the PTY has spawned or with no account", () => {
    expect(isAccountPending("a", null)).toBe(false);
    expect(isAccountPending(null, "a")).toBe(false);
    expect(isAccountPending(null, null)).toBe(false);
  });
  it("is true only when the configured account differs from the one it ran under", () => {
    expect(isAccountPending("a", "a")).toBe(false);
    expect(isAccountPending("b", "a")).toBe(true);
    expect(isAccountPending("a", "")).toBe(true);
  });
});

describe("adding an account", () => {
  const colored = (id: string, color: string): AccountProfile => ({ ...acct(id), color });

  it("hands out the first unused palette colour", () => {
    expect(nextAccountColor([])).toBe(ACCOUNT_COLORS[0]);
    expect(nextAccountColor([colored("a", ACCOUNT_COLORS[0].toUpperCase())])).toBe(ACCOUNT_COLORS[1]);
    expect(nextAccountColor([colored("a", ACCOUNT_COLORS[1])])).toBe(ACCOUNT_COLORS[0]);
  });

  it("cycles once every colour is taken", () => {
    const all = ACCOUNT_COLORS.map((c, i) => colored(String(i), c));
    expect(nextAccountColor(all)).toBe(ACCOUNT_COLORS[all.length % ACCOUNT_COLORS.length]);
  });

  it("names the next account after the count, skipping names in use", () => {
    expect(nextAccountName([], "Account")).toBe("Account 1");
    const a = { ...acct("a"), name: "Account 2" };
    expect(nextAccountName([a], "Account")).toBe("Account 3");
    expect(nextAccountName([{ ...acct("b"), name: "Work" }], "Account")).toBe("Account 2");
  });

  it("asks for an account only for agents that read one", () => {
    expect(agentUsesAccounts("claude")).toBe(true);
    expect(agentUsesAccounts("codex")).toBe(true);
    expect(agentUsesAccounts("gemini")).toBe(false);
  });
});

describe("isManagedAccount", () => {
  const prof = (id: string, claude: string, codex: string): AccountProfile => ({
    id,
    name: "x",
    claude_config_dir: claude,
    codex_home: codex,
    color: "",
  });
  it("accepts the folders ymux made for this id (POSIX)", () => {
    expect(
      isManagedAccount(
        prof("ab12cd34", "/home/u/.config/ymux/accounts/ab12cd34/claude", "/home/u/.config/ymux/accounts/ab12cd34/codex"),
      ),
    ).toBe(true);
  });
  it("accepts Windows backslash paths", () => {
    expect(
      isManagedAccount(
        prof(
          "ab12cd34",
          "C:\\Users\\u\\AppData\\Roaming\\ymux\\accounts\\ab12cd34\\claude",
          "C:\\Users\\u\\AppData\\Roaming\\ymux\\accounts\\ab12cd34\\codex",
        ),
      ),
    ).toBe(true);
  });
  it("tolerates a trailing separator", () => {
    expect(isManagedAccount(prof("ab12cd34", "/c/accounts/ab12cd34/claude/", "/c/accounts/ab12cd34/codex\\"))).toBe(true);
  });
  it("rejects when either folder is hand-entered or empty", () => {
    expect(isManagedAccount(prof("ab12cd34", "/home/u/.claude", "/c/accounts/ab12cd34/codex"))).toBe(false);
    expect(isManagedAccount(prof("ab12cd34", "/c/accounts/ab12cd34/claude", ""))).toBe(false);
    expect(isManagedAccount(prof("ab12cd34", "", ""))).toBe(false);
  });
  it("rejects folders belonging to another id", () => {
    expect(isManagedAccount(prof("ab12cd34", "/c/accounts/ffff0000/claude", "/c/accounts/ffff0000/codex"))).toBe(false);
  });
  it("rejects swapped leaves", () => {
    expect(isManagedAccount(prof("ab12cd34", "/c/accounts/ab12cd34/codex", "/c/accounts/ab12cd34/claude"))).toBe(false);
  });
  it("rejects an id that is not a plain token", () => {
    expect(isManagedAccount(prof("", "/c/accounts//claude", "/c/accounts//codex"))).toBe(false);
    expect(isManagedAccount(prof("../x", "/c/accounts/../x/claude", "/c/accounts/../x/codex"))).toBe(false);
  });
  it("requires a whole path segment before accounts/", () => {
    expect(isManagedAccount(prof("ab12cd34", "/c/myaccounts/ab12cd34/claude", "/c/myaccounts/ab12cd34/codex"))).toBe(false);
  });
});
