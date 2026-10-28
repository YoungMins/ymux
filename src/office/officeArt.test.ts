// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import * as art from "./pixelArt";
import * as themeArt from "./themeArt";
import { THEMES, THEME_ICONS, THEME_IDS, defaultFloorTheme, isThemeId, type Piece, type Theme } from "./themes";
import { deskSvg, iconSvg, lobbyProps, roomProps } from "./sprites";
import { CHAR_FRAMES, characterFrames, characterSprite, petFrames, petSprite } from "./sheet";
import { PET_H, PET_POSES, PET_SPECIES, PET_W } from "./hiPets";
import { FRAME_H, FRAME_W, slimeFrame } from "./hiSlime";
import { humanFrame } from "./hiHuman";
import { TIER } from "./heroTiers";
import { Raster, ellipse, paint, ramp } from "./paint";
import { hiPiece, scale3x } from "./hiProps";
import { WALL_FACE, surfaceTile } from "./hiFloors";
import { FUN_ART, FUN_PAL } from "./funArt";
import { SHOP } from "./funModel";

const grids = Object.entries(art).filter(([, v]) => Array.isArray(v)) as [string, readonly string[]][];
const same = (a: Raster, b: Raster) => a.px.every((c, i) => c === b.px[i]);
const sheetKey = (frames: Raster[]) => frames.map((f) => f.px.join(",")).join("|");
/// Lowest row with anything drawn.
const lowest = (r: Raster) => {
  for (let y = r.h - 1; y >= 0; y--) for (let x = 0; x < r.w; x++) if (r.get(x, y)) return y;
  return -1;
};

describe("painter", () => {
  it("shades a part from its ramp and edges it with the outline", () => {
    const r = ramp("#f0905f");
    expect(new Set(r.tones).size).toBe(5);
    const out = paint(new Raster(20, 20), ellipse(10, 10, 6, 6), r);
    expect(out.get(10, 10)).not.toBeNull();
    expect(r.tones).toContain(out.get(10, 10));
    // The leftmost pixel of the middle row is outline.
    const row = [...Array(20).keys()].map((x) => out.get(x, 10));
    expect(row[row.findIndex((c) => c !== null)]).toBe(r.outline);
    expect(out.get(0, 0)).toBeNull();
  });

  it("mirrors a raster", () => {
    const r = new Raster(4, 1);
    r.set(0, 0, "#ffffff");
    expect(r.flipped().get(3, 0)).toBe("#ffffff");
  });
});

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

  it("leaves screen and LED pixels to the stylesheet", () => {
    expect(deskSvg()).toContain('class="px-screen"');
    expect(roomProps("git")).toContain('class="px-led"');
    expect(lobbyProps()).toContain("office-prop--reception");
    expect(iconSvg("alert")).toContain("<rect");
    expect(iconSvg("nope")).toBe("");
  });
});

describe("characters", () => {
  const at = (frames: Raster[], f: (typeof CHAR_FRAMES)[number]) => frames[CHAR_FRAMES.indexOf(f)];

  it("paints every frame of the sheet at one art pixel per CSS pixel", () => {
    for (const role of ["lead", "sub", "manager"] as const) {
      const frames = characterFrames("claude", role);
      expect(frames).toHaveLength(CHAR_FRAMES.length);
      for (const f of frames) {
        expect([f.w, f.h]).toEqual([FRAME_W, FRAME_H]);
        expect(f.count(), role).toBeGreaterThan(200);
      }
    }
  });

  it("stands on the floor, and lifts off it at the top of a hop", () => {
    const frames = characterFrames("claude", "lead");
    expect(lowest(at(frames, "normal"))).toBeGreaterThanOrEqual(FRAME_H - 4);
    expect(lowest(at(frames, "walkB"))).toBeLessThan(lowest(at(frames, "walkA")));
  });

  it("blinks, and moves between the frames of a moving pose", () => {
    const frames = characterFrames("gemini", "lead");
    expect(same(at(frames, "normal"), at(frames, "normalBlink"))).toBe(false);
    expect(same(at(frames, "raise"), at(frames, "raiseBlink"))).toBe(false);
    for (const [a, b] of [["walkA", "walkB"], ["typeA", "typeB"], ["relaxA", "relaxB"], ["cheerA", "cheerB"]] as const) {
      expect(same(at(frames, a), at(frames, b)), a).toBe(false);
    }
  });

  it("draws helpers smaller, the user with a bow tie, and any unknown agent", () => {
    const lead = at(characterFrames("codex", "lead"), "normal");
    expect(at(characterFrames("codex", "sub"), "normal").count()).toBeLessThan(lead.count() * 0.7);
    expect(at(characterFrames("", "manager"), "normal").px.some((c) => c !== null && c.toLowerCase().startsWith("#c8"))).toBe(true);
    expect(at(characterFrames("unknown-cli", "lead"), "normal").count()).toBeGreaterThan(200);
  });

  it("puts a sheet in a box the stylesheet crops to one frame", () => {
    const el = characterSprite("claude", "lead");
    expect(el.classList.contains("office-sprite")).toBe(true);
    const canvas = el.querySelector("canvas")!;
    expect([canvas.width, canvas.height]).toEqual([FRAME_W * CHAR_FRAMES.length, FRAME_H]);
    expect(petSprite("cat").querySelector("canvas")!.width).toBe(PET_W * PET_POSES.length);
  });
});

describe("themes", () => {
  const pieces = (t: Theme): Piece[] => [
    ...t.wall.main,
    ...t.wall.files,
    ...t.wall.git,
    ...t.wall.board,
    t.corner,
    t.lounge,
    t.side,
    ...Object.values(t.lobby),
  ];
  const rowsOf = (p: Piece) => ("rows" in p ? p.rows : p);

  it("keeps every theme grid rectangular", () => {
    for (const [name, rows] of Object.entries(themeArt)) {
      expect(new Set((rows as readonly string[]).map((r) => r.length)).size, name).toBe(1);
    }
  });

  it("gives every theme a 32x22 desk whose status pixels the stylesheet colours", () => {
    for (const id of THEME_IDS) {
      const t = THEMES[id];
      expect(t.desk, id).toHaveLength(art.DESK.length);
      for (const r of t.desk) expect(r, id).toHaveLength(art.DESK[0].length);
      expect(deskSvg(id), id).toContain('class="px-screen"');
    }
  });

  it("has a colour for every pixel of every piece", () => {
    for (const id of THEME_IDS) {
      const t = THEMES[id];
      const check = (rows: readonly string[], pal: Readonly<Record<string, string>>, what: string) => {
        for (const k of new Set(rows.join(""))) if (k !== ".") expect(pal[k], `${id} ${what} "${k}"`).toBeDefined();
      };
      check(t.desk, t.pal, "desk");
      pieces(t).forEach((p, i) => check(rowsOf(p), "rows" in p ? p.pal : t.pal, `piece ${i}`));
    }
  });

  it("dresses every room kind and the lobby", () => {
    for (const id of THEME_IDS) {
      for (const kind of ["terminal", "files", "git", "editor", "browser", "token_usage"] as const) {
        expect(roomProps(kind, id), `${id} ${kind}`).toContain("office-wall-row");
      }
      expect(lobbyProps(id), id).toContain("office-prop--reception");
    }
    expect(roomProps("terminal", "cave")).toContain('class="px-flame"');
  });

  it("gives each workspace its own place by default", () => {
    expect(defaultFloorTheme(1, "castle")).toBe("castle");
    expect(defaultFloorTheme(2, "office")).toBe("cave");
    const places = [1, 2, 3, 4, 5, 6, 7, 8].map((p) => defaultFloorTheme(p, "office"));
    expect(new Set(places).size).toBe(8);
    for (const id of THEME_IDS) expect(THEME_ICONS[id]).toBeTruthy();
  });

  it("falls back to the office for an unknown saved theme", () => {
    expect(isThemeId("cave")).toBe(true);
    expect(isThemeId("moon")).toBe(false);
  });
});

describe("hero gear", () => {
  const classes = ["warrior", "mage", "rogue", "cleric", "cat"] as const;
  const JELLY = { body: "#f0905f", eye: "#2a1f1a", blush: "#ff8c8c" };
  const look = (cls: (typeof classes)[number], tier: number, form?: "slime" | "human") => characterFrames("claude", "lead", { cls, tier, form });

  it("is nothing at tier 0, and only leads wear it", () => {
    const plain = sheetKey(characterFrames("claude", "lead"));
    expect(sheetKey(look("warrior", 0))).toBe(plain);
    expect(sheetKey(look("warrior", 2))).not.toBe(plain);
    expect(sheetKey(characterFrames("claude", "sub", { cls: "warrior", tier: 2 }))).toBe(sheetKey(characterFrames("claude", "sub")));
  });

  it("changes the look at every step, for both forms", () => {
    for (const form of ["slime", "human"] as const) {
      for (const cls of classes) {
        const still = (tier: number) =>
          form === "human" ? humanFrame(JELLY, cls, tier, { arms: "down", legs: "stand", eyes: "open" }) : slimeFrame({ jelly: JELLY, role: "lead", hero: { cls, tier } }, { body: "normal", eyes: "open" });
        let prev = still(0);
        for (let tier = 1; tier <= TIER.LEGEND; tier++) {
          const now = still(tier);
          // Gold is a colour swap some classes have no metal for; the aura is CSS.
          if (tier !== TIER.GOLD && tier !== TIER.AURA) expect(same(now, prev), `${form} ${cls} ${tier}`).toBe(false);
          prev = now;
        }
      }
    }
  });

  it("draws a human for a lead whose form is human, standing on the floor", () => {
    for (const cls of classes) {
      const human = look(cls, TIER.ITEM, "human");
      expect(same(human[0], look(cls, TIER.ITEM)[0]), cls).toBe(false);
      expect(lowest(human[0]), cls).toBeGreaterThanOrEqual(FRAME_H - 4);
      for (const f of human) expect(f.count(), cls).toBeGreaterThan(300);
    }
  });
});

describe("pets", () => {
  it("paints every species' poses, with a walk of two different frames", () => {
    for (const sp of PET_SPECIES) {
      const f = petFrames(sp);
      expect(f).toHaveLength(PET_POSES.length);
      for (const r of f) {
        expect([r.w, r.h]).toEqual([PET_W, PET_H]);
        expect(r.count(), sp).toBeGreaterThan(80);
      }
      expect(same(f[0], f[1]), sp).toBe(false);
      expect(lowest(f[PET_POSES.indexOf("sit")]), sp).toBeGreaterThanOrEqual(PET_H - 4);
    }
  });
});

describe("hi-res furniture", () => {
  it("redraws a grid at three times the size, rounding diagonals", () => {
    const g = scale3x(["x.", ".x"]);
    expect([g.length, g[0].length]).toEqual([6, 6]);
    expect(g[0][0]).toBe("x");
  });

  it("thins outer outlines, keeps class pixels apart and every piece drawn", () => {
    const pal = { o: "#2a1f1a", a: "#c08a5a", n: ".px-screen" };
    const { raster, classes } = hiPiece(["oooo", "oano", "oooo"], pal);
    expect([raster.w, raster.h]).toEqual([12, 9]);
    // The outer two of three outline rows are worn away.
    expect(raster.get(5, 0)).toBeNull();
    expect(raster.get(5, 2)).toBe("#2a1f1a");
    expect(classes.some((c) => c.cls === "px-screen")).toBe(true);
    for (const id of THEME_IDS) expect(deskSvg(id), id).toContain("hp-canvas");
  });
});

describe("hi-res floors", () => {
  it("has a tile for every surface of every theme, walls the height of the face", () => {
    for (const id of THEME_IDS) {
      for (const s of ["corridor", "lobby", "room", "wall"] as const) {
        const tile = surfaceTile(id, s);
        expect(tile, `${id} ${s}`).not.toBeNull();
        expect(tile!.count(), `${id} ${s}`).toBe(tile!.w * tile!.h);
        if (s === "wall") expect(tile!.h, id).toBe(WALL_FACE);
      }
    }
    expect(surfaceTile("office", "room", "git")).not.toBe(surfaceTile("office", "room", "terminal"));
  });
});

describe("office games art", () => {
  it("draws every shop piece and prop rectangular, with known colours", () => {
    for (const [name, rows] of Object.entries(FUN_ART)) {
      expect(new Set(rows.map((r) => r.length)).size, name).toBe(1);
      for (const k of new Set(rows.join(""))) if (k !== ".") expect(FUN_PAL[k], `${name} "${k}"`).toBeDefined();
    }
    for (const item of SHOP) if (item.place !== "pet") expect(FUN_ART[item.id], item.id).toBeDefined();
  });
});
