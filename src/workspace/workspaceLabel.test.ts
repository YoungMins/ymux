import { describe, it, expect } from "vitest";
import {
  formatWorkspaceLabel,
  editableWorkspaceName,
  nextWorkspaceName,
} from "./workspaceLabel";

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
