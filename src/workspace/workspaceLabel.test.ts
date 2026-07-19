import { describe, it, expect } from "vitest";
import { formatWorkspaceLabel, sortWorkspacesById } from "./workspaceLabel";

describe("formatWorkspaceLabel", () => {
  it("shows just the id when there is no custom name", () => {
    expect(formatWorkspaceLabel(3, null)).toBe("3");
    expect(formatWorkspaceLabel(3, undefined)).toBe("3");
    expect(formatWorkspaceLabel(3, "")).toBe("3");
  });

  it("treats the default names as no custom name", () => {
    expect(formatWorkspaceLabel(2, "workspace-2")).toBe("2");
    expect(formatWorkspaceLabel(1, "main")).toBe("1");
  });

  it("shows 'id: name' for a custom name", () => {
    expect(formatWorkspaceLabel(1, "build")).toBe("1: build");
  });
});

describe("sortWorkspacesById", () => {
  it("returns a new array sorted ascending by id, leaving the input untouched", () => {
    const input = [{ id: 3 }, { id: 1 }, { id: 2 }];
    const out = sortWorkspacesById(input);
    expect(out.map((w) => w.id)).toEqual([1, 2, 3]);
    expect(input.map((w) => w.id)).toEqual([3, 1, 2]);
  });
});
