import { describe, expect, it } from "vitest";
import * as art from "./pixelArt";
import { characterSvg, deskSvg, iconSvg, lobbyProps, roomProps } from "./sprites";

const grids = Object.entries(art).filter(([, v]) => Array.isArray(v)) as [string, readonly string[]][];

describe("pixel art", () => {
  it("keeps every row of a grid the same width", () => {
    for (const [name, rows] of grids) {
      const widths = new Set(rows.map((r) => r.length));
      expect(widths.size, name).toBe(1);
    }
    for (const [name, rows] of Object.entries(art.ICONS)) {
      expect(new Set(rows.map((r) => r.length)).size, `icon ${name}`).toBe(1);
    }
  });

  it("draws characters on the shared canvas", () => {
    for (const name of ["HEAD", "TORSO", "LEGS_STAND", "LEGS_A", "LEGS_B", "ARMS_DOWN", "ARMS_RAISE", "ARMS_MUG", "CAP", "TIE"] as const) {
      const rows = art[name];
      expect(rows[0].length, name).toBe(art.CHAR_W);
      expect(rows.length, name).toBeLessThanOrEqual(art.CHAR_H);
    }
  });

  it("renders every frame group a pose needs", () => {
    const svg = characterSvg("claude", "lead");
    for (const g of ["px-legs-stand", "px-legs-a", "px-legs-b", "px-arms-down", "px-arms-type-a", "px-arms-type-b", "px-arms-raise", "px-arms-mug", "px-eyes-open", "px-eyes-shut"]) {
      expect(svg, g).toContain(g);
    }
    expect(characterSvg("codex", "sub")).toContain("px-cap");
    expect(characterSvg("", "manager")).toContain("px-tie");
    expect(characterSvg("unknown-cli", "lead")).toContain("<rect");
  });

  it("leaves screen and LED pixels to the stylesheet", () => {
    expect(deskSvg()).toContain('class="px-screen"');
    expect(roomProps("git")).toContain('class="px-led"');
    expect(lobbyProps()).toContain("office-prop--reception");
    expect(iconSvg("alert")).toContain("<rect");
    expect(iconSvg("nope")).toBe("");
  });
});
