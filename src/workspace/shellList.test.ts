import { describe, expect, it } from "vitest";
import type { ShellProfile } from "../types";
import { mergeNewShells } from "./shellList";

const sh = (name: string): ShellProfile => ({ name, executable: `${name}.exe`, args: [] }) as ShellProfile;

describe("mergeNewShells", () => {
  it("appends only shells the list lacks and keeps the existing order", () => {
    const known = [sh("zsh"), sh("bash")];
    const out = mergeNewShells(known, [sh("bash"), sh("fish"), sh("zsh")]);
    expect(out.map((s) => s.name)).toEqual(["zsh", "bash", "fish"]);
  });

  it("returns the same array when nothing is new", () => {
    const known = [sh("zsh")];
    expect(mergeNewShells(known, [sh("zsh")])).toBe(known);
  });

  it("never drops a known shell the backend no longer lists", () => {
    expect(mergeNewShells([sh("a"), sh("b")], [sh("a")]).map((s) => s.name)).toEqual(["a", "b"]);
  });
});
