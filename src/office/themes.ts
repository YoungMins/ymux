// The office view's themes: the same floor plan dressed as an office, a
// cave, a castle, … A theme is a palette plus the pieces that fill each slot
// of a room (back wall by pane kind, corner, floor pieces) and of the lobby;
// its floors and walls are CSS (`.office--theme-<id>`). Each workspace (a
// floor of the office) is its own place: the theme picked for it is a
// per-machine view preference, kept in localStorage, and a floor nobody
// picked for gets the next place along (`defaultFloorTheme`).

import * as art from "./pixelArt";
import * as th from "./themeArt";

export type Palette = Readonly<Record<string, string>>;

/// A grid, with its own palette when it doesn't use the theme's.
export type Piece = readonly string[] | { rows: readonly string[]; pal: Palette };

export const THEME_IDS = ["office", "cave", "castle", "ranch", "space", "sea", "snow", "desert"] as const;
export type ThemeId = (typeof THEME_IDS)[number];

export interface Theme {
  id: ThemeId;
  pal: Palette;
  /// 32x22, with `n`/`N` where the pane's status shows.
  desk: readonly string[];
  /// Back-wall pieces, by the kind of pane the room is.
  wall: { main: Piece[]; files: Piece[]; git: Piece[]; board: Piece[] };
  /// Bottom-left of every room.
  corner: Piece;
  /// Floor piece of a browser room, and of an editor room.
  lounge: Piece;
  side: Piece;
  lobby: { centre: Piece; side: Piece; left: Piece; right: Piece };
}

/// Colours for the office's own furniture. A value starting with "." is a
/// CSS class instead of a fill (a desk's screen follows its pane's status).
export const PROPS: Palette = {
  o: "#2a1f1a",
  M: "#2d333b",
  n: ".px-screen",
  N: ".px-screen-hi",
  k: "#d8dde3",
  K: "#9aa3ad",
  d: "#c98e57",
  D: "#a56f40",
  f: "#9a6236",
  F: "#7a4a28",
  g: "#e1c08a",
  G: "#2f7a3b",
  a: "#b0623a",
  A: "#8a4a2a",
  w: "#f3ece0",
  "1": "#d64545",
  "2": "#4a90d9",
  "3": "#e0b84f",
  "4": "#5cb85c",
  "5": "#9b6bd6",
  "6": "#e07a3a",
  R: "#4a5260",
  y: "#f5c542",
  b: "#8ecbf0",
  B: "#ffffff",
  r: "#d64545",
  c: "#5b8c5a",
  C: "#4a7449",
};
const PLANT: Palette = { ...PROPS, g: "#4fa35a", G: "#2f7a3b" };
const RACK: Palette = { ...PROPS, g: ".px-led", y: ".px-led-warn", "1": "#7d8794" };

const SCREEN = { n: ".px-screen", N: ".px-screen-hi" };
const FLAME = { f: ".px-flame", F: ".px-flame-hi" };

const OFFICE: Theme = {
  id: "office",
  pal: PROPS,
  desk: art.DESK,
  wall: {
    main: [art.WINDOW],
    files: [art.SHELF, art.SHELF],
    git: [{ rows: art.RACK, pal: RACK }, { rows: art.RACK, pal: RACK }, { rows: art.RACK, pal: RACK }],
    board: [art.BOARD],
  },
  corner: { rows: art.PLANT, pal: PLANT },
  lounge: art.SOFA,
  side: art.PAPERS,
  lobby: {
    centre: art.RECEPTION,
    side: art.COOLER,
    left: { rows: art.PLANT, pal: PLANT },
    right: { rows: art.PLANT, pal: PLANT },
  },
};

const CAVE: Theme = {
  id: "cave",
  pal: {
    ...SCREEN,
    ...FLAME,
    o: "#1d1714",
    s: "#8a7866",
    S: "#6b5b4c",
    t: "#4a3d33",
    k: "#9aa3ad",
    K: "#6b737d",
    d: "#8a5a34",
    D: "#6a4024",
    b: "#8a5a34",
    y: "#f5c542",
    c: "#8ee6ff",
    C: "#4fb3d9",
    W: "#e8fbff",
    m: "#d64545",
    w: "#f4efe6",
    x: "#e9dcc4",
    g: "#5b8c5a",
    r: "#b8563a",
  },
  desk: th.CAVE_DESK,
  wall: { main: [th.TORCH, th.CRYSTALS, th.TORCH], files: [th.MINECART], git: [th.ORE, th.ORE], board: [th.PAINTING] },
  corner: th.MUSHROOMS,
  lounge: th.ROCKS,
  side: th.CRYSTALS,
  lobby: { centre: th.CAMPFIRE, side: th.MINECART, left: th.MUSHROOMS, right: th.CRYSTALS },
};

const CASTLE_PAL: Palette = {
  ...SCREEN,
  ...FLAME,
  o: "#1b1a22",
  y: "#e8c14a",
  Y: "#b8902a",
  d: "#8a5a34",
  D: "#6a4024",
  w: "#f3ece0",
  k: "#c6ccd6",
  K: "#8a94a3",
  r: "#b83b3b",
  R: "#852a2a",
  b: "#4a6fb8",
  "1": "#b83b3b",
  "2": "#4a6fb8",
  "3": "#e8c14a",
  "4": "#4f8a4f",
  "5": "#7a4fa8",
  "6": "#c4703a",
};
const CASTLE: Theme = {
  id: "castle",
  pal: CASTLE_PAL,
  desk: th.CASTLE_DESK,
  wall: {
    main: [th.BANNER, th.TORCH, th.BANNER],
    files: [
      { rows: art.SHELF, pal: { ...CASTLE_PAL, w: "#6a4024" } },
      { rows: art.SHELF, pal: { ...CASTLE_PAL, w: "#6a4024" } },
    ],
    git: [th.SHIELD, th.SWORDS, th.SHIELD],
    board: [th.TAPESTRY],
  },
  corner: th.BARREL,
  lounge: th.ARMOR,
  side: th.CANDLE,
  lobby: { centre: th.THRONE, side: th.ARMOR, left: th.CANDLE, right: th.CANDLE },
};

const RANCH: Theme = {
  id: "ranch",
  pal: {
    ...SCREEN,
    o: "#2a1d14",
    h: "#e8c35a",
    H: "#c79a32",
    j: "#f5dc8a",
    r: "#b84a3a",
    R: "#8a3328",
    d: "#a8743f",
    D: "#7d5229",
    k: "#9aa3ad",
    K: "#6b737d",
    w: "#f4efe6",
    g: "#6cbf4f",
    y: "#f5c542",
    Y: "#d18f1f",
    b: "#6a4024",
    s: "#a7a7a7",
    S: "#808080",
  },
  desk: th.RANCH_DESK,
  wall: {
    main: [{ rows: art.WINDOW, pal: PROPS }, th.HORSESHOE],
    files: [th.SACKS],
    git: [th.TOOLS],
    board: [{ rows: art.BOARD, pal: PROPS }],
  },
  corner: th.SUNFLOWER,
  lounge: th.HAYBALE,
  side: th.MILKCAN,
  lobby: { centre: th.WELL, side: th.HAYBALE, left: th.FENCE, right: th.SUNFLOWER },
};

const SPACE: Theme = {
  id: "space",
  pal: {
    ...SCREEN,
    o: "#0e1018",
    M: "#3a4050",
    m: "#6b7486",
    l: "#a9b3c2",
    k: "#c6ccd6",
    K: "#e07a3a",
    d: "#141826",
    s: "#ffffff",
    p: "#e07a3a",
    P: "#b8562a",
    g: ".px-led",
    y: ".px-led-warn",
    r: "#d64545",
    c: "#5be3e0",
    a: "#b48cf2",
    A: "#7d5ad1",
    b: "#4a90d9",
    B: "#8ecbf0",
    w: "#f4efe6",
  },
  desk: th.SPACE_DESK,
  wall: {
    main: [th.PORTHOLE, th.PANEL],
    files: [th.LOCKER, th.LOCKER, th.LOCKER],
    git: [{ rows: art.RACK, pal: RACK }, { rows: art.RACK, pal: RACK }, { rows: art.RACK, pal: RACK }],
    board: [th.HOLO],
  },
  corner: th.ALIEN,
  lounge: th.ROBOT,
  side: th.TANK,
  lobby: { centre: th.PAD, side: th.ROBOT, left: th.TANK, right: th.ALIEN },
};

const SEA: Theme = {
  id: "sea",
  pal: {
    ...SCREEN,
    o: "#0f2230",
    l: "#c9a3e6",
    L: "#9a72c0",
    f: "#ff9a4a",
    r: "#6f7f8f",
    R: "#4d5a68",
    k: "#3fae6a",
    K: "#2a7d4a",
    c: "#ff7a8a",
    C: "#d94f6a",
    d: "#8a5a34",
    D: "#6a4024",
    y: "#f5c542",
    Y: "#c89a2a",
    m: "#8a94a3",
  },
  desk: th.SEA_DESK,
  wall: { main: [th.KELP, th.CORAL, th.KELP], files: [th.CHEST], git: [th.ANCHOR], board: [th.WHEEL, th.CORAL] },
  corner: th.KELP,
  lounge: th.STARFISH,
  side: th.BARREL,
  lobby: { centre: th.OPENCHEST, side: th.ANCHOR, left: th.CORAL, right: th.KELP },
};

const SNOW: Theme = {
  id: "snow",
  pal: {
    ...SCREEN,
    o: "#1d2430",
    i: "#cfeefe",
    I: "#9fd3ef",
    j: "#ffffff",
    w: "#f4efe6",
    b: "#8ecbf0",
    s: "#f7fbff",
    k: "#2a2f3d",
    c: "#f08a3a",
    r: "#d64545",
    d: "#8a5a34",
    D: "#6a4024",
    p: "#3f8a5a",
    P: "#2a6a42",
    y: "#f5c542",
    Y: "#ffe58a",
  },
  desk: th.SNOW_DESK,
  wall: { main: [th.FROST, th.SKIS], files: [th.WOODPILE], git: [th.SLED], board: [{ rows: art.BOARD, pal: PROPS }] },
  corner: th.PINE,
  lounge: th.SNOWMAN,
  side: th.WOODPILE,
  lobby: { centre: th.BIGTREE, side: th.SNOWMAN, left: th.PINE, right: th.PINE },
};

const DESERT: Theme = {
  id: "desert",
  pal: {
    ...SCREEN,
    ...FLAME,
    o: "#2a1d14",
    s: "#e2c28a",
    S: "#c49e62",
    t: "#9e7a46",
    g: "#3a6fb0",
    b: "#3a6fb0",
    y: "#f5c542",
    Y: "#c8962a",
    u: "#c46a3a",
    U: "#8f4626",
    p: "#5aa84a",
    P: "#3a7a32",
    c: "#5aa84a",
    C: "#3a7a32",
    d: "#8a5a34",
    w: "#f3ece0",
    k: "#8a7a66",
    a: "#4fb3d9",
    A: "#8ee6ff",
  },
  desk: th.DESERT_DESK,
  wall: { main: [th.GLYPHS, th.TORCH], files: [th.URN, th.URN, th.URN], git: [th.SARCO], board: [th.FRIEZE] },
  corner: th.CACTUS,
  lounge: th.PALM,
  side: th.URN,
  lobby: { centre: th.OASIS, side: th.OBELISK, left: th.CACTUS, right: th.PALM },
};

export const THEMES: Readonly<Record<ThemeId, Theme>> = {
  office: OFFICE,
  cave: CAVE,
  castle: CASTLE,
  ranch: RANCH,
  space: SPACE,
  sea: SEA,
  snow: SNOW,
  desert: DESERT,
};

export function isThemeId(v: unknown): v is ThemeId {
  return typeof v === "string" && (THEME_IDS as readonly string[]).includes(v);
}

const STORAGE_KEY = "ymux.office.theme";

/// The saved theme, or the office. localStorage may be unavailable.
export function loadTheme(): ThemeId {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    return isThemeId(v) ? v : "office";
  } catch {
    return "office";
  }
}

export function saveTheme(id: ThemeId): void {
  try {
    localStorage.setItem(STORAGE_KEY, id);
  } catch {
    /* localStorage unavailable */
  }
}

/// An icon per place, for the floor buttons.
export const THEME_ICONS: Readonly<Record<ThemeId, string>> = {
  office: "🏢",
  cave: "⛏",
  castle: "🏰",
  ranch: "🌾",
  space: "🚀",
  sea: "🌊",
  snow: "❄",
  desert: "🏜",
};

const FLOORS_KEY = "ymux.office.floorThemes";

/// The place a floor gets before anyone picks one: the first workspace keeps
/// the theme chosen before themes were per floor, the others take the next
/// places in turn.
export function defaultFloorTheme(position: number, first: ThemeId): ThemeId {
  if (position <= 1) return first;
  const i = THEME_IDS.indexOf(first);
  return THEME_IDS[(i + position - 1) % THEME_IDS.length];
}

function readFloors(): Record<string, ThemeId> {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(FLOORS_KEY) ?? "{}");
    const out: Record<string, ThemeId> = {};
    if (v && typeof v === "object" && !Array.isArray(v)) for (const [k, id] of Object.entries(v)) if (isThemeId(id)) out[k] = id;
    return out;
  } catch {
    return {};
  }
}

/// The theme of workspace `wsId` (at 1-based `position` in the panel).
export function loadFloorTheme(wsId: number, position: number): ThemeId {
  return readFloors()[String(wsId)] ?? defaultFloorTheme(position, loadTheme());
}

export function saveFloorTheme(wsId: number, id: ThemeId): void {
  try {
    localStorage.setItem(FLOORS_KEY, JSON.stringify({ ...readFloors(), [String(wsId)]: id }));
  } catch {
    /* localStorage unavailable */
  }
}
