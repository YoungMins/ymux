// Pixel art for the office view's furniture and bubble icons, as grids of
// palette keys ("." is clear). The characters and pets are hi-res canvas
// sheets (`sheet.ts`). Keep every row of a grid the same width
// (officeArt.test.ts).

export const DESK: readonly string[] = [
  "..................oooooooooooo..",
  "..................oMMMMMMMMMMo..",
  "..................oMnnnnnnnnMo..",
  "..................oMnNnnnnnnMo..",
  "..................oMnnnnnnnnMo..",
  "..................oMnnnnnnnnMo..",
  "..................oMMMMMMMMMMo..",
  "......................oMMo......",
  "...ookkkkkkkkoo......oMMMMo.....",
  "oooooooooooooooooooooooooooooooo",
  "oddddddddddddddddddddddddddddddo",
  "oDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDo",
  "oooooooooooooooooooooooooooooooo",
  ".offfffffffffffffffffoFFFFFFFFo.",
  ".offfffffffffffffffffoFgggggFFo.",
  ".offfffffffffffffffffoFFFFFFFFo.",
  ".offfffffffffffffffffoooooooooo.",
  ".offfffffffffffffffffoFgggggFFo.",
  ".offfffffffffffffffffoFFFFFFFFo.",
  ".oooooooooooooooooooooooooooooo.",
  ".oFo........................oFo.",
  ".ooo........................ooo.",
];

export const PLANT: readonly string[] = [
  ".....oo.....",
  "....oggo.oo.",
  ".oo.oggooggo",
  "oggoogGoogo.",
  ".ogGoGgoGo..",
  "..oggGggGgo.",
  ".ogGgggGgGo.",
  "..oogGgGoo..",
  "....oGgo....",
  "..oooooooo..",
  "..oaaaaaao..",
  "...oaAaao...",
  "...oaaAao...",
  "...oooooo...",
];

export const SHELF: readonly string[] = [
  "oooooooooooooooooooo",
  "owwwwwwwwwwwwwwwwwwo",
  "owoooooooooooooooowo",
  "owo1o2o3oo4o5o6o3owo",
  "owo1o2o3oo4o5o6o3owo",
  "owo1o2o3oo4o5o6o3owo",
  "owoooooooooooooooowo",
  "owwwwwwwwwwwwwwwwwwo",
  "owoooooooooooooooowo",
  "owo4o4oo6o1o1o3o5owo",
  "owo4o4oo6o1o1o3o5owo",
  "owo4o4oo6o1o1o3o5owo",
  "owoooooooooooooooowo",
  "owwwwwwwwwwwwwwwwwwo",
  "oooooooooooooooooooo",
];

export const RACK: readonly string[] = [
  "oooooooooooooo",
  "oRRRRRRRRRRRRo",
  "oRooooooooooRo",
  "oRo11o..g.y.Ro",
  "oRooooooooooRo",
  "oRo11o..g.g.Ro",
  "oRooooooooooRo",
  "oRo11o..y.g.Ro",
  "oRooooooooooRo",
  "oRo11o..g.g.Ro",
  "oRooooooooooRo",
  "oRRRRRRRRRRRRo",
  "oooooooooooooo",
  ".oo........oo.",
];

export const WINDOW: readonly string[] = [
  "oooooooooooooooooooooooo",
  "owwwwwwwwwwwwwwwwwwwwwwo",
  "owbbbbbbbbbowbbbbbbbbbwo",
  "owbbbbbBbbbowbbbbbbyybwo",
  "owbbbbBBBbbowbbbbbbyybwo",
  "owbbbbbbbbbowbbbbbbbbbwo",
  "owbbbbbbbbbowbbbbbbbbbwo",
  "owgggbbbbbbowbbbbbgggbwo",
  "owgGgggbbbbowbbbggGggGwo",
  "owwwwwwwwwwwwwwwwwwwwwwo",
  "oooooooooooooooooooooooo",
];

export const BOARD: readonly string[] = [
  "oooooooooooooooooooooo",
  "owwwwwwwwwwwwwwwwwwwwo",
  "ow..............y...wo",
  "ow..........r...y...wo",
  "ow......g...r...y...wo",
  "ow..b...g...r...y...wo",
  "ow..b...g...r...y...wo",
  "ow..b...g...r...y...wo",
  "owkkkkkkkkkkkkkkkkkkwo",
  "owwwwwwwwwwwwwwwwwwwwo",
  "oooooooooooooooooooooo",
  "..........oo..........",
  ".........oooo.........",
];

export const SOFA: readonly string[] = [
  "..oooooooooooooooooooooo..",
  ".occcccccccccccccccccccco.",
  ".ocCCCCCCCCCCCCCCCCCCCCco.",
  "ooccccccccccccccccccccccoo",
  "occoccccccccccccccccccocco",
  "occoCCCCCCCCCCCCCCCCCCocco",
  "occoooooooooooooooooooocco",
  "occcccccccccccccccccccccco",
  "oooooooooooooooooooooooooo",
  ".oo....................oo.",
];

export const PAPERS: readonly string[] = [
  "....oooooooooo..",
  "...owwwwwwwwwwo.",
  "..oowkkkkkkkwwo.",
  ".owwwwwwwwwwwwo.",
  ".owkkkkkkkkkwo..",
  ".owwwwwwwwwwwo..",
  ".owkkkkkkkwwwo..",
  ".owwwwwwwwwwwo..",
  ".ooooooooooooo..",
];

export const COOLER: readonly string[] = [
  "..oooooo..",
  ".obbbbbbo.",
  ".obBbbbbo.",
  ".obbbbbbo.",
  "..oooooo..",
  ".owwwwwwo.",
  ".owrwwbwo.",
  ".owwwwwwo.",
  ".owwwwwwo.",
  ".owwwwwwo.",
  ".oooooooo.",
];

export const RECEPTION: readonly string[] = [
  "..oooooooooooooooooooooooooooooooooooo..",
  ".oddddddddddddddddddddddddddddddddddddo.",
  ".oDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDo.",
  "oooooooooooooooooooooooooooooooooooooooo",
  "offffffffffffffffffffffffffffffffffffffo",
  "offfffffffffffffgggggggffffffffffffffffo",
  "offffffffffffffffffffffffffffffffffffffo",
  "offffffffffffffffffffffffffffffffffffffo",
  "oooooooooooooooooooooooooooooooooooooooo",
];

export const ICONS: Readonly<Record<string, readonly string[]>> = {
  alert: ["orro", "orro", "orro", "orro", "oooo", "orro", "oooo"],
  check: ["......o", ".....og", "o...ogo", "go.ogo.", ".ogo...", "..o...."],
  zzz: ["oooo...", "..o....", ".o.....", "oooo.oo", ".....o.", "....oo."],
  dots: [".......", ".......", "o.o.o..", "......."],
  paper: ["ooooo.", "owwwoo", "okkkwo", "owwwwo", "okkkwo", "owwwwo", "oooooo"],
  chat: [".ooooo.", "owwwwwo", "owkwkwo", "owwwwwo", ".oowoo.", "..oo..."],
  review: ["oooooo.", "owwwwo.", "okkwwor", "owwwwrr", "okkwrro", "owwrro.", "ooooo.."],
  box: [".ooooo.", "oddDddo", "oooooo.", "odddddo", "oddgddo", "odddddo", "ooooooo"],
  heart: [".oo.oo.", "orrorro", "orrrrro", ".orrro.", "..oro..", "...o..."],
};

/// A pennant with a check, flown outside a room whose agent finished.
export const FLAG: readonly string[] = [
  "oo..........",
  "owoooooooooo",
  "owGGGGGGGGGo",
  "owGGGGGGGBGo",
  "owGGBGGGBGGo",
  "owGGGBGBGGGo",
  "owGGGGBGGGGo",
  "owoooooooooo",
  "ow..........",
  "ow..........",
  "ow..........",
  "ow..........",
  "ow..........",
  "ow..........",
  "ow..........",
  "ow..........",
  "oooo........",
  "oooo........",
];

/// A post box by a room's door. `LETTER_SEALED` / `LETTER_OPEN` are drawn
/// over it, sticking out of the slot, while it holds mail.
export const MAILBOX: readonly string[] = [
  "............",
  "............",
  "............",
  "..oooooooo..",
  ".orrrrrrrro.",
  "orrrrrrrrrro",
  "orRRRRRRRRro",
  "orrrrrrrrrro",
  "orryyrrrrrro",
  "oRRRRRRRRRRo",
  "oooooooooooo",
  "....oPPo....",
  "....oPPo....",
  "....oPPo....",
  "....oPPo....",
  "....oPPo....",
  "...oooooo...",
  "............",
];

export const LETTER_SEALED: readonly string[] = [
  "...oooooo...",
  "...owwwwo...",
  "...okwwko...",
  "...owkkwo...",
  "...owwwwo...",
  "............",
  ...Array.from({ length: 12 }, () => "............"),
];

export const LETTER_OPEN: readonly string[] = [
  "...o....o...",
  "...oo..oo...",
  "...owoowo...",
  "...owwwwo...",
  "...owwwwo...",
  "............",
  ...Array.from({ length: 12 }, () => "............"),
];

/// The lobby's cork board: a frame around cork, `w` x `h` art pixels.
export function corkGrid(w: number, h: number): string[] {
  const out: string[] = [];
  for (let y = 0; y < h; y++) {
    let row = "";
    for (let x = 0; x < w; x++) {
      const edge = x === 0 || y === 0 || x === w - 1 || y === h - 1;
      const frame = x <= 2 || y <= 2 || x >= w - 3 || y >= h - 3;
      if (edge) row += "o";
      else if (frame) row += y <= 2 || x <= 2 ? "d" : "D";
      else row += (x * 7 + y * 13) % 11 === 0 ? "Q" : "q";
    }
    out.push(row);
  }
  return out;
}
