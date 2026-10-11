import { describe, it, expect } from "vitest";
import {
  formatWorkspaceLabel,
  workspaceIdAtPosition,
  workspacePosition,
  editableWorkspaceName,
  nextWorkspaceName,
} from "./workspaceLabel";

describe("formatWorkspaceLabel", () => {
  it("shows just the id when there is no custom name", () => {
    expect(formatWorkspaceLabel(3, null, 3)).toBe("3");
    expect(formatWorkspaceLabel(3, undefined, 3)).toBe("3");
    expect(formatWorkspaceLabel(3, "", 3)).toBe("3");
  });

  it("treats the default names as no custom name", () => {
    expect(formatWorkspaceLabel(2, "workspace-2", 2)).toBe("2");
    expect(formatWorkspaceLabel(1, "main", 1)).toBe("1");
  });

  it("shows 'id: name' for a custom name", () => {
    expect(formatWorkspaceLabel(1, "build", 1)).toBe("1: build");
  });
});

describe("editableWorkspaceName", () => {
  it("is empty for a workspace that was never renamed", () => {
    expect(editableWorkspaceName(3, null)).toBe("");
    expect(editableWorkspaceName(3, undefined)).toBe("");
    expect(editableWorkspaceName(3, "")).toBe("");
    expect(editableWorkspaceName(2, "workspace-2")).toBe("");
    expect(editableWorkspaceName(1, "main")).toBe("");
  });

  it("is the raw name for a custom name", () => {
    expect(editableWorkspaceName(1, "build")).toBe("build");
  });
});

describe("nextWorkspaceName", () => {
  it("rejects an empty or whitespace-only name", () => {
    expect(nextWorkspaceName("", "build")).toBeNull();
    expect(nextWorkspaceName("   ", "build")).toBeNull();
    expect(nextWorkspaceName("\t \n", "build")).toBeNull();
  });

  it("rejects a name that is unchanged after trimming", () => {
    expect(nextWorkspaceName("build", "build")).toBeNull();
    expect(nextWorkspaceName("  build  ", "build")).toBeNull();
  });

  it("trims the accepted name", () => {
    expect(nextWorkspaceName("  deploy  ", "build")).toBe("deploy");
  });

  it("accepts the first name of a never-renamed workspace", () => {
    expect(nextWorkspaceName("deploy", "")).toBe("deploy");
  });

  it("rejects an empty edit of a never-renamed workspace", () => {
    expect(nextWorkspaceName("", "")).toBeNull();
    expect(nextWorkspaceName("  ", "")).toBeNull();
  });
});

describe("positional numbering", () => {
  const ws = (...ids: number[]) => ids.map((id) => ({ id }));

  it("renumbers the survivors after a delete (ids {1,3} -> positions 1,2)", () => {
    const list = ws(1, 3);
    expect(workspacePosition(list, 1)).toBe(1);
    expect(workspacePosition(list, 3)).toBe(2);
    expect(formatWorkspaceLabel(3, "workspace-3", workspacePosition(list, 3))).toBe("2");
    expect(formatWorkspaceLabel(3, "build", workspacePosition(list, 3))).toBe("2: build");
  });

  it("follows display order after a reorder", () => {
    const list = ws(3, 1, 2);
    expect(workspacePosition(list, 3)).toBe(1);
    expect(workspaceIdAtPosition(list, 1)).toBe(3);
    expect(workspaceIdAtPosition(list, 3)).toBe(2);
  });

  it("maps a shortcut digit to the Nth workspace, null when out of range", () => {
    const list = ws(1, 3);
    expect(workspaceIdAtPosition(list, 2)).toBe(3);
    expect(workspaceIdAtPosition(list, 3)).toBeNull();
    expect(workspaceIdAtPosition(list, 0)).toBeNull();
  });

  it("an appended workspace gets position count + 1 even when it reuses a low id", () => {
    expect(workspacePosition(ws(1, 3, 2), 2)).toBe(3);
  });

  it("unknown id has position 0", () => {
    expect(workspacePosition(ws(1), 9)).toBe(0);
  });
});
