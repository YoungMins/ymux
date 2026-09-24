import { describe, expect, it } from "vitest";
import type { BranchList, CommitInfo, StatusEntry, WorkStatus, WorktreeEntry } from "../ipc/bridge";
import {
  branchItems,
  changeKind,
  checkoutPlan,
  checkoutRisk,
  clip,
  confirmationStillHolds,
  formatCommitDate,
  refChips,
  removePlan,
  validateBranchName,
} from "./gitModel";

const list = (over: Partial<BranchList> = {}): BranchList => ({
  current: "main",
  local: ["main", "기능/한글브랜치", "held"],
  remote: ["origin/main", "origin/기능/원격", "origin/held"],
  held: { held: "C:/wt/held" },
  ...over,
});

const status = (over: Partial<WorkStatus> = {}): WorkStatus => ({
  detached: false,
  changes: [],
  ignored: [],
  orphans: [],
  more_orphans: false,
  ...over,
});

const wt = (over: Partial<WorktreeEntry> = {}): WorktreeEntry => ({
  path: "C:/wt/x",
  branch: "x",
  head: "abc",
  detached: false,
  bare: false,
  locked: false,
  prunable: false,
  main: false,
  current: false,
  ...over,
});

const commit = (subject: string): CommitInfo => ({
  hash: "f".repeat(40),
  short: "fffffff",
  subject,
  author: "me",
  date_ms: 0,
  parents: [],
  refs: [],
});

const entry = (code: string, path: string, orig = ""): StatusEntry => ({ code, path, orig });

describe("branchItems", () => {
  it("lists local branches, then remote ones, marking current and held", () => {
    const items = branchItems(list());
    expect(items.map((i) => `${i.kind}:${i.name}`)).toEqual([
      "local:main",
      "local:기능/한글브랜치",
      "local:held",
      "remote:origin/main",
      "remote:origin/기능/원격",
      "remote:origin/held",
    ]);
    expect(items[0].current).toBe(true);
    expect(items[1].current).toBe(false);
    expect(items[2].heldBy).toBe("C:/wt/held");
    expect(items[3].heldBy).toBeNull();
  });
});

describe("checkoutPlan", () => {
  const items = branchItems(list());
  const find = (name: string) => items.find((i) => i.name === name)!;

  it("does nothing for the branch already checked out", () => {
    expect(checkoutPlan(find("main"), list())).toEqual({ kind: "noop", branch: "main" });
  });

  it("refuses a branch another worktree holds, naming that worktree", () => {
    expect(checkoutPlan(find("held"), list())).toEqual({
      kind: "held",
      branch: "held",
      path: "C:/wt/held",
    });
  });

  it("checks out a local branch by name, Korean included", () => {
    expect(checkoutPlan(find("기능/한글브랜치"), list())).toEqual({
      kind: "local",
      branch: "기능/한글브랜치",
    });
  });

  it("tracks a remote branch that has no local counterpart instead of detaching", () => {
    expect(checkoutPlan(find("origin/기능/원격"), list())).toEqual({
      kind: "track",
      remote: "origin/기능/원격",
      branch: "기능/원격",
    });
  });

  it("uses the existing local branch for a remote one, with its own rules", () => {
    expect(checkoutPlan(find("origin/main"), list())).toEqual({ kind: "noop", branch: "main" });
    expect(checkoutPlan(find("origin/held"), list())).toEqual({
      kind: "held",
      branch: "held",
      path: "C:/wt/held",
    });
    const other = list({ current: "기능/한글브랜치" });
    expect(checkoutPlan(branchItems(other).find((i) => i.name === "origin/main")!, other)).toEqual({
      kind: "local",
      branch: "main",
      fromRemote: "origin/main",
    });
  });
});

describe("checkoutRisk", () => {
  it("is nothing to report on a clean tree", () => {
    expect(checkoutRisk(status())).toEqual({ carried: [], stranded: [], moreStranded: false, loses: false });
  });

  it("lists changes as carried over, not lost: checkout never discards them", () => {
    const r = checkoutRisk(status({ changes: [entry(" M", "a.rs"), entry("??", "메모.md")] }));
    expect(r.carried.map((e) => e.path)).toEqual(["a.rs", "메모.md"]);
    expect(r.loses).toBe(false);
  });

  it("names detached commits no ref reaches: leaving HEAD strands them", () => {
    const r = checkoutRisk(status({ detached: true, orphans: [commit("떠돌이")], more_orphans: true }));
    expect(r.stranded.map((c) => c.subject)).toEqual(["떠돌이"]);
    expect(r.moreStranded).toBe(true);
    expect(r.loses).toBe(true);
  });
});

describe("removePlan", () => {
  it("never removes the main worktree", () => {
    expect(removePlan(wt({ main: true }), null)).toEqual({ kind: "blocked", reason: "main" });
  });

  it("refuses a locked worktree rather than double-forcing it", () => {
    expect(removePlan(wt({ locked: true }), null)).toEqual({ kind: "blocked", reason: "locked" });
  });

  it("refuses the worktree the pane is showing", () => {
    expect(removePlan(wt({ current: true }), null)).toEqual({ kind: "blocked", reason: "current" });
  });

  it("refuses one whose folder is gone", () => {
    expect(removePlan(wt({ prunable: true }), null)).toEqual({ kind: "blocked", reason: "missing" });
  });

  it("refuses while another pane is working inside the worktree, naming it", () => {
    const panes = ["ws1: pwsh", "ws2: notes.md"];
    expect(removePlan(wt(), null, panes)).toEqual({ kind: "blocked", reason: "inUse", panes });
    expect(removePlan(wt(), status(), panes)).toEqual({ kind: "blocked", reason: "inUse", panes });
    // The stronger reasons still win.
    expect(removePlan(wt({ main: true }), null, panes)).toEqual({ kind: "blocked", reason: "main" });
    expect(removePlan(wt(), null, [])).toEqual({ kind: "needStatus" });
  });

  it("needs the status before deciding anything else", () => {
    expect(removePlan(wt(), null)).toEqual({ kind: "needStatus" });
  });

  it("removes a clean worktree without force, still naming ignored files it deletes", () => {
    const p = removePlan(wt(), status({ ignored: ["target/", "node_modules/"] }));
    expect(p).toEqual({
      kind: "remove",
      force: false,
      branch: "x",
      lost: [],
      ignored: ["target/", "node_modules/"],
      stranded: [],
      moreStranded: false,
      loses: true,
    });
  });

  it("counts ignored files as a loss: git deletes them without --force", () => {
    // `.env`, a local database: gone with the folder, so Cancel is the default.
    const withIgnored = removePlan(wt(), status({ ignored: [".env"] }));
    expect(withIgnored.kind === "remove" && withIgnored.loses).toBe(true);
    const clean = removePlan(wt(), status());
    expect(clean.kind === "remove" && clean.loses).toBe(false);
    const dirty = removePlan(wt(), status({ changes: [entry(" M", "a.rs")] }));
    expect(dirty.kind === "remove" && dirty.loses).toBe(true);
  });

  it("forces only with the changes it destroys listed", () => {
    const p = removePlan(wt(), status({ changes: [entry(" M", "a.rs"), entry("??", "새 파일.txt")] }));
    expect(p.kind).toBe("remove");
    if (p.kind !== "remove") return;
    expect(p.force).toBe(true);
    expect(p.lost.map((e) => e.path)).toEqual(["a.rs", "새 파일.txt"]);
  });

  it("names stranded commits of a detached worktree", () => {
    const p = removePlan(
      wt({ branch: "", detached: true }),
      status({ detached: true, orphans: [commit("wip")] }),
    );
    expect(p.kind === "remove" && p.stranded.map((c) => c.subject)).toEqual(["wip"]);
    expect(p.kind === "remove" && p.force).toBe(false);
  });
});

describe("confirmationStillHolds", () => {
  const shown = removePlan(wt(), status({ changes: [entry("??", "a.txt")], ignored: ["target/"] }));

  it("holds when a fresh status would show exactly the same list", () => {
    const fresh = removePlan(wt(), status({ changes: [entry("??", "a.txt")], ignored: ["target/"] }));
    expect(confirmationStillHolds(shown, fresh)).toBe(true);
  });

  it("breaks when a file appeared, changed state or vanished since the dialog", () => {
    const more = removePlan(
      wt(),
      status({ changes: [entry("??", "a.txt"), entry("??", "agent-wrote.rs")], ignored: ["target/"] }),
    );
    expect(confirmationStillHolds(shown, more)).toBe(false);
    const restaged = removePlan(wt(), status({ changes: [entry("A ", "a.txt")], ignored: ["target/"] }));
    expect(confirmationStillHolds(shown, restaged)).toBe(false);
    const gone = removePlan(wt(), status({ ignored: ["target/"] }));
    expect(confirmationStillHolds(shown, gone)).toBe(false);
  });

  it("breaks when new ignored files or stranded commits appeared", () => {
    const ignored = removePlan(
      wt(),
      status({ changes: [entry("??", "a.txt")], ignored: ["target/", ".env"] }),
    );
    expect(confirmationStillHolds(shown, ignored)).toBe(false);
    const clean = removePlan(wt({ detached: true, branch: "" }), status({ detached: true }));
    const committed = removePlan(
      wt({ detached: true, branch: "" }),
      status({ detached: true, orphans: [commit("agent commit")] }),
    );
    expect(confirmationStillHolds(clean, committed)).toBe(false);
  });

  it("breaks when the worktree can no longer be removed at all", () => {
    expect(confirmationStillHolds(shown, { kind: "blocked", reason: "locked" })).toBe(false);
  });
});

describe("changeKind", () => {
  it("reads git's XY code", () => {
    expect(changeKind(" M")).toBe("modified");
    expect(changeKind("M ")).toBe("modified");
    expect(changeKind("MM")).toBe("modified");
    expect(changeKind("A ")).toBe("added");
    expect(changeKind(" D")).toBe("deleted");
    expect(changeKind("R ")).toBe("renamed");
    expect(changeKind("C ")).toBe("copied");
    expect(changeKind("??")).toBe("untracked");
    expect(changeKind("UU")).toBe("conflict");
    expect(changeKind("AA")).toBe("conflict");
    expect(changeKind("DD")).toBe("conflict");
  });
});

describe("clip", () => {
  it("keeps short lists and counts what it leaves out", () => {
    expect(clip([1, 2], 3)).toEqual({ shown: [1, 2], more: 0 });
    expect(clip([1, 2, 3, 4, 5], 3)).toEqual({ shown: [1, 2, 3], more: 2 });
  });
});

describe("refChips", () => {
  const remotes = new Set(["origin/main", "origin/HEAD"]);

  it("splits %D decorations into head, branch, remote and tag chips", () => {
    expect(refChips(["HEAD -> main", "tag: v0.10.1", "origin/main", "기능/한글"], remotes)).toEqual([
      { kind: "head", name: "main" },
      { kind: "tag", name: "v0.10.1" },
      { kind: "remote", name: "origin/main" },
      { kind: "branch", name: "기능/한글" },
    ]);
  });

  it("shows a bare HEAD as detached and hides the origin/HEAD alias", () => {
    expect(refChips(["HEAD", "origin/HEAD", "origin/HEAD -> origin/main"], remotes)).toEqual([
      { kind: "detached", name: "HEAD" },
    ]);
  });
});

describe("validateBranchName", () => {
  it("accepts ordinary and Korean names", () => {
    for (const ok of ["main", "agent/abc123", "기능/한글브랜치", "fix-1.2"]) {
      expect(validateBranchName(ok)).toBeNull();
    }
  });

  it("rejects what git check-ref-format would", () => {
    for (const bad of [
      "",
      "-x",
      "a b",
      "a..b",
      "a~1",
      "a^",
      "a:b",
      "a?",
      "a*",
      "a[",
      "a\\b",
      "a/",
      "/a",
      "a//b",
      "a.lock",
      "a/.hidden",
      "a.",
      "a@{1}",
      "@",
      "tab\there",
    ]) {
      expect(validateBranchName(bad), bad).not.toBeNull();
    }
  });
});

describe("formatCommitDate", () => {
  const now = new Date(2026, 8, 24, 16, 0);

  it("shows the time for today and the date otherwise", () => {
    expect(formatCommitDate(new Date(2026, 8, 24, 9, 5).getTime(), now)).toBe("09:05");
    expect(formatCommitDate(new Date(2026, 8, 23, 23, 59).getTime(), now)).toBe("2026-09-23");
    expect(formatCommitDate(0, now)).toBe("");
  });
});
