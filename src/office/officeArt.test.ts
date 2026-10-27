import { describe, expect, it } from "vitest";
import * as art from "./pixelArt";
import * as themeArt from "./themeArt";
import { THEMES, THEME_IDS, isThemeId, type Piece, type Theme } from "./themes";
import { characterSvg, deskSvg, iconSvg, lobbyProps, roomProps } from "./sprites";
import { humanGrid } from "./humanArt";
import { TIER } from "./heroTiers";
import { BODIES, EYES, MUG, SLIME_H, SLIME_W, blinkGrid, bodyGrid, eyeRow, eyesGrid, gearGrid, sproutGrid, tieGrid } from "./slimeArt";

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

  it("draws every slime part on the shared canvas", () => {
    const parts = [MUG, ...BODIES.flatMap((b) => [bodyGrid(b), sproutGrid(b), tieGrid(b), ...EYES.map((e) => eyesGrid(b, e))])];
    for (const rows of parts) {
      expect(rows).toHaveLength(SLIME_H);
      for (const r of rows) expect(r).toHaveLength(SLIME_W);
    }
  });

  it("keeps each body outlined, centred and resting on the floor", () => {
    for (const b of BODIES) {
      const rows = bodyGrid(b);
      expect(rows[SLIME_H - 1], b).toMatch(/^\.*o+\.*$/);
      for (const r of rows) {
        const filled = r.replace(/\./g, "");
        if (!filled) continue;
        expect(r.indexOf(filled[0]), b).toBe(SLIME_W - 1 - r.lastIndexOf(filled[filled.length - 1]));
        expect(r[r.search(/[^.]/)], b).toBe("o");
      }
    }
  });

  it("puts the eyes inside the body", () => {
    for (const b of BODIES) {
      const body = bodyGrid(b);
      for (const e of EYES) {
        eyesGrid(b, e).forEach((r, y) => {
          [...r].forEach((k, x) => {
            if (k !== ".") expect(body[y][x], `${b}/${e} at ${x},${y}`).not.toBe(".");
          });
        });
      }
      expect(eyeRow(b)).toBeLessThan(SLIME_H - 1);
    }
  });

  it("renders a group for every pose, moving ones as two-frame strips", () => {
    const svg = characterSvg("claude", "lead");
    for (const g of ["px-still-normal", "px-still-raise", "px-still-sleep", "px-anim-walk", "px-anim-type", "px-anim-relax", "px-anim-cheer", "px-blink"]) {
      expect(svg, g).toContain(g);
    }
    // A strip's second frame sits one canvas to the right.
    expect(svg).toContain(`translate(${SLIME_W} `);
    expect(characterSvg("codex", "sub")).not.toBe(characterSvg("codex", "lead"));
    expect(characterSvg("", "manager")).toContain("#d64545");
    expect(characterSvg("unknown-cli", "lead")).toContain("<rect");
  });

  it("covers the open eyes when blinking", () => {
    for (const b of BODIES) {
      const open = eyesGrid(b, "open");
      const blink = blinkGrid(b);
      open.forEach((r, y) => [...r].forEach((k, x) => {
        if (k === "e") expect(blink[y][x], `${b} ${x},${y}`).not.toBe(".");
      }));
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

  it("falls back to the office for an unknown saved theme", () => {
    expect(isThemeId("cave")).toBe(true);
    expect(isThemeId("moon")).toBe(false);
  });
});

describe("hero gear", () => {
  const classes = ["warrior", "mage", "rogue", "cleric", "cat"] as const;
  // Keys the slime palette (body, face, base colours) and the gear palette define.
  const KNOWN = new Set("oucCwekmrRgGaAbBnNyYjlivdD");

  it("stays on the canvas with known colours, and is empty at tier 0", () => {
    for (const body of BODIES) {
      for (const cls of classes) {
        for (let tier = 0; tier <= TIER.LEGEND; tier++) {
          const { back, front } = gearGrid(body, cls, tier);
          for (const rows of [back, front]) {
            expect(rows).toHaveLength(SLIME_H);
            for (const r of rows) {
              expect(r).toHaveLength(SLIME_W);
              for (const k of r) if (k !== ".") expect(KNOWN.has(k), `${body} ${cls} ${tier} "${k}"`).toBe(true);
            }
          }
          const drawn = front.join("").replace(/\./g, "").length;
          if (tier === 0) expect(drawn, `${cls} tier 0`).toBe(0);
          else expect(drawn, `${body} ${cls} ${tier}`).toBeGreaterThan(0);
        }
      }
    }
  });

  it("adds each step's decoration at its tier", () => {
    const all = (g: { back: string[]; front: string[] }) => g.back.join("") + g.front.join("");
    // A jewel, the class item (the sword's guard is gold), the sash, wings.
    expect(gearGrid("normal", "mage", TIER.JEWEL - 1).front.join("")).not.toMatch(/j/);
    expect(gearGrid("normal", "mage", TIER.JEWEL).front.join("")).toMatch(/j/);
    expect(gearGrid("normal", "warrior", TIER.ITEM - 1).back.join("")).not.toMatch(/y/);
    expect(gearGrid("normal", "warrior", TIER.ITEM).back.join("")).toMatch(/y/);
    expect(all(gearGrid("normal", "warrior", TIER.CAPE - 1))).not.toMatch(/v/);
    expect(all(gearGrid("normal", "warrior", TIER.CAPE))).toMatch(/v/);
    expect(gearGrid("normal", "warrior", TIER.WINGS - 1).back.join("")).not.toMatch(/u/);
    expect(gearGrid("normal", "warrior", TIER.WINGS).back.join("")).toMatch(/u/);
    // Every step looks different from the one before.
    for (const cls of classes) {
      for (let tier = 1; tier <= TIER.LEGEND; tier++) {
        if (tier === TIER.GOLD || tier === TIER.AURA) continue; // a palette swap / CSS
        expect(all(gearGrid("normal", cls, tier)), `${cls} ${tier}`).not.toBe(all(gearGrid("normal", cls, tier - 1)));
      }
    }
  });

  it("dresses only leads", () => {
    const plain = characterSvg("claude", "lead");
    expect(characterSvg("claude", "lead", { cls: "warrior", tier: 2 })).not.toBe(plain);
    expect(characterSvg("claude", "sub", { cls: "warrior", tier: 2 })).toBe(characterSvg("claude", "sub"));
  });
});

describe("human heroes", () => {
  const classes = ["warrior", "mage", "rogue", "cleric", "cat"] as const;
  const arms = ["down", "raise", "type", "typeUp", "up", "mug", "sip", "swingA", "swingB"] as const;
  // Outline, skin, hair, tunic, trousers, boots, face, mug, plus the gear keys.
  const KNOWN = new Set("osShHtTqdekmuUaAbBnNyYjlirRgGv");

  it("draws every class, tier and pose on the canvas with known colours", () => {
    for (const cls of classes) {
      for (let tier = 0; tier <= TIER.LEGEND; tier++) {
        for (const a of arms) {
          for (const legs of ["stand", "stepA", "stepB"] as const) {
            const { back, front } = humanGrid({ arms: a, legs, eyes: "open" }, cls, tier);
            const bad = new Set<string>();
            for (const rows of [back, front]) {
              if (rows.length !== SLIME_H || rows.some((r) => r.length !== SLIME_W)) bad.add("size");
              for (const r of rows) for (const k of r) if (k !== "." && !KNOWN.has(k)) bad.add(k);
            }
            expect([...bad], `${cls} ${tier} ${a} ${legs}`).toEqual([]);
          }
        }
      }
    }
  });

  it("stands on the canvas floor and is outlined all round", () => {
    const { front } = humanGrid({ arms: "down", legs: "stand", eyes: "open" }, "warrior", 0);
    expect(front[SLIME_H - 1]).toMatch(/o/);
    for (const r of front) {
      const first = r.search(/[^.]/);
      if (first >= 0) expect(r[first]).toBe("o");
    }
  });

  it("is drawn for a lead whose hero form is human", () => {
    const human = characterSvg("claude", "lead", { cls: "mage", tier: 2, form: "human" });
    expect(human).not.toBe(characterSvg("claude", "lead", { cls: "mage", tier: 2 }));
    for (const pose of ["px-still-normal", "px-still-raise", "px-still-sleep", "px-anim-walk", "px-anim-type", "px-anim-relax", "px-anim-cheer"]) {
      expect(human).toContain(pose);
    }
  });
});
