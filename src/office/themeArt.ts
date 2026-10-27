// Pixel art for the office's themes (cave, castle, …), as grids of palette
// keys like `pixelArt.ts`; every row of a grid is the same width
// (officeArt.test.ts). Desks are 32x22 like the office's, with `n`/`N` for
// the part that glows with the pane's status. Palettes live in `themes.ts`.

export const CAVE_DESK: readonly string[] = [
  ".......................o........",
  "......................onoo......",
  ".....................onNno......",
  "....................onNnnno.....",
  "........kk..........onnnnno.....",
  ".......okK...........onnno......",
  "......oo..............ono.......",
  ".....ob..............ottto......",
  "....ob..............otttttto....",
  "oooooooooooooooooooooooooooooooo",
  "osssssttsssssssssssssssssstsssso",
  "ossssssssssssssssssstsssssssssso",
  "oSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSo",
  "oooooooooooooooooooooooooooooooo",
  ".....oSSSSSSSSSSSSSSSSSSSSo.....",
  ".....oSSSStSSSSSSSSSSSSSSSo.....",
  ".....oSSSSStSSSSSSSSSSSSSSo.....",
  ".....oSSSSSSSSSSSSSttSSSSSo.....",
  ".....oSSSSSSSSSSSSSSSSSSSSo.....",
  "...oooooooooooooooooooooooooo...",
  "...osssssssssssssssssssssssso...",
  "...oooooooooooooooooooooooooo...",
];

export const CASTLE_DESK: readonly string[] = [
  "......................oooo......",
  ".....................onnNno.....",
  "....................onnnnNno....",
  "....................onnnnnno....",
  "....................onnnnnno....",
  "...oooooooooo........onnnno.....",
  "...owwwwowwwo.........oooo......",
  "...owkkwowkko........oyyyyo.....",
  "...oooooooooo.......oYYYYYYo....",
  "oooooooooooooooooooooooooooooooo",
  "oddddddddddddddddddddddddddddddo",
  "oDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDo",
  "oooooooooooooooooooooooooooooooo",
  "..oDo......................oDo..",
  "..oDooooooooooooooooooooooooDo..",
  "..oDoDDDDDDDDDDDDDDDDDDDDDDoDo..",
  "..oDooooooooooooooooooooooooDo..",
  "..oDo......................oDo..",
  "..oDo......................oDo..",
  "..oDo......................oDo..",
  "..ooo......................ooo..",
  "................................",
];

export const RANCH_DESK: readonly string[] = [
  ".......................oo.......",
  "......................o..o......",
  ".....................oooooo.....",
  ".....................okkkko.....",
  ".....................onNnno.....",
  "......ooo............onnnno.....",
  ".....owwwo...........onnnno.....",
  ".....owwwo...........okkkko.....",
  ".....oooo............oooooo.....",
  "oooooooorooooooooooooooroooooooo",
  "ojjjjjjjrjjjjjjjjjjjjjjrjjjjjjjo",
  "ohhhhHhhrhhhHhhhhhhHhhhrhhHhhhho",
  "oHhhhhhhrhhhhhhHhhhhhhHrhhhhhHho",
  "ohhhHhhhrhhHhhhhhhHhhhhrhHhhhhho",
  "ohhhhhhHrhhhhhHhhhhhhHhrhhhhHhho",
  "ohhHhhhhrhHhhhhhhHhhhhhrHhhhhhho",
  "ohhhhhHhrhhhhHhhhhhhHhhrhhhHhhho",
  "ohHhhhhhrHhhhhhhHhhhhhhrhhhhhhHo",
  "ohhhhHhhrhhhHhhhhhhHhhhrhhHhhhho",
  "oHhhhhhhrhhhhhhHhhhhhhHrhhhhhHho",
  "oHHHHHHHHHHHHHHHHHHHHHHHHHHHHHHo",
  "oooooooorooooooooooooooroooooooo",
];

export const SPACE_DESK: readonly string[] = [
  "..................oooooooooooo..",
  "..................oMMMMMMMMMMo..",
  "..................oMnNnnnnnnMo..",
  "..................oMnnnnnnnnMo..",
  "..................oMnnnnnnnnMo..",
  "..................oMMMMMMMMMMo..",
  "...................oooooooooo...",
  "......................oMMo......",
  "...ookkkkkkkkoo......oMMMMo.....",
  "oooooooooooooooooooooooooooooooo",
  "ollKlklKlklKlklllllllllllllllllo",
  "ommmmmmmmmmmmmmmmmmmmmmmmmmmmmmo",
  "oooooooooooooooooooooooooooooooo",
  "..oMMMMMMMMMMMMMMMMMMMMMMMMMMo..",
  "..ommmmmmmmmmmmmmmmmmmmmmmmmmo..",
  "..ommgmmymmrmmmmmmmmgmmgmmymmo..",
  "..ommmmmmmmmmmmmmmmmmmmmmmmmmo..",
  "..omMMMMMMMMMMMMMMMMMMMMMMMMmo..",
  "..ommmmmmmmmmmmmmmmmmmmmmmmmmo..",
  "..omMMMMMMMMMMMMMMMMMMMMMMMMmo..",
  "..oooooooooooooooooooooooooooo..",
  ".oooooooooooooooooooooooooooooo.",
];

export const SEA_DESK: readonly string[] = [
  "................................",
  "................................",
  "....................oooooooo....",
  "...................olLlLlLlLo...",
  "...................oLlLlLlLlo...",
  ".......o............oooooooo....",
  "......ofo.............onNo......",
  ".....offfo...........onnnno.....",
  "......ofo..........olLonnolLo...",
  ".....o.o.o........oooooooooooo..",
  "..oooooooooooooooooooooooooooo..",
  ".orrrrrrrrrrrrrrrrrrrrrrrrrrrro.",
  "oooooooooooooooooooooooooooooooo",
  "orrrrRRrrrrrrrrrrrrrrrrrrRrrrrro",
  "orrrrrrrrrrrrrrrrrRrrrrrrrrcrcro",
  "orrrrrrrrrrrrrrrrrrrrrrrrrccCcro",
  "orrkrrrrrrrrRrrrrrrrrrrrrrrcCrro",
  "orkKrrrrrrrrrrrrrrrrrrRrrrrrcrro",
  "okKkRRRRRRRRRRRRRRRRRRRRRRRRRRRo",
  "okKkRRRRRRRRRRRRRRRRRRRRRRRRRRRo",
  "oooooooooooooooooooooooooooooooo",
  "..oooooooooooooooooooooooooooo..",
];

export const SNOW_DESK: readonly string[] = [
  ".......................oo.......",
  "......................o..o......",
  ".....................oooooo.....",
  ".....................okkkko.....",
  ".....oooo............onNnno.....",
  ".....owwoo...........onnnno.....",
  ".....owwo.o..........onnnno.....",
  ".....owwoo...........okkkko.....",
  ".....oooo............oooooo.....",
  "oooooooooooooooooooooooooooooooo",
  "ojjjjjjjjjjjjjjojjjjjjjjjjjjjjjo",
  "oijjjiiiiiiiiiioiiiiiiiiiiiiiiio",
  "oijiiiiiiiiiiiioiiiiiiiiiiiiiiio",
  "oiiiiiiiiiiiiiioiiiiiiiiiiiiiiio",
  "oiiiiiiiiiiiiiioiiiiiiiiiiiiiiIo",
  "oIIIIIIIIIIIIIIoIIIIIIIIIIIIIIIo",
  "oiiiiiiiiiiiiiioiiiiiiiiiiiiIiio",
  "oiiiiiiiiiiiiiioiiiiiiiiiiiIiiio",
  "oiiiiiiiiiiiiiioiiiiiiiiiiIiiiio",
  "oiiiiiiiiiiiiiioiiiiiiiiiIiiiiio",
  "oIIIIIIIIIIIIIIoIIIIIIIIIIIIIIIo",
  "oooooooooooooooooooooooooooooooo",
];

export const DESERT_DESK: readonly string[] = [
  "....................oooooooo....",
  "...................osssssssso...",
  "...................osnnNnnnso...",
  "...................osnsnnsnso...",
  "...................osnnnNnnso...",
  "....oooooooo.......osnsnnnnso...",
  "....owwwwwwo.......osssssssso...",
  "....okkkkkwo........oSSSSSSo....",
  "....oooooooo.......oSSSSSSSSo...",
  "oooooooooooooooooooooooooooooooo",
  "osssssttsssssssssssssssssstsssso",
  "ossssssssssssssssssstsssssssssso",
  "oSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSo",
  "oooooooooooooooooooooooooooooooo",
  ".....oSSSSSSSSSSSSSSSSSSSSo.....",
  ".....oSSSStSSSSSSSSSSSSSSSo.....",
  ".....oSSSSStSSSSSSSSSSSSSSo.....",
  ".....oSSSSSSSSSSSSSttSSSSSo.....",
  ".....oSSSSSSSSSSSSSSSSSSSSo.....",
  "...oooooooooooooooooooooooooo...",
  "...osssssssssssssssssssssssso...",
  "...oooooooooooooooooooooooooo...",
];

export const TORCH: readonly string[] = [
  "...f...",
  "..fFf..",
  "..FfF..",
  "..oFo..",
  ".oyyyo.",
  "..odo..",
  "..odo..",
  "..odo..",
  "...o...",
];

export const CRYSTALS: readonly string[] = [
  "....o.......",
  "...oco...o..",
  "...ocWo.oco.",
  "..o.oCco.cWo",
  ".oco.oCo.oCo",
  ".ocWooCcoCco",
  "oCcCcoCcoCco",
  "osssssssssso",
  "oooooooooooo",
];

export const MUSHROOMS: readonly string[] = [
  "...oooo.....",
  "..ommwmo....",
  ".omwmmmmo...",
  ".oooooooo...",
  "...oxxo.ooo.",
  "...oxxoommo.",
  "...oxxo.oo..",
  "...oxxo.ox..",
  "..ggxxggxg..",
  "oooooooooooo",
];

export const MINECART: readonly string[] = [
  "..s..t...s..t.s.",
  ".tssytsstsssyts.",
  "oooooooooooooooo",
  "okkkkkkkkkkkkkko",
  "oKkkkkkkkkkkkkKo",
  "oKKKKKKKKKKKKKKo",
  ".oooooooooooooo.",
  "..ooo......ooo..",
  "..oko......oko..",
  "...o........o...",
];

export const ORE: readonly string[] = [
  "oooooooooooooo",
  "osssstssssssso",
  "osyysssstssyso",
  "ossyssssssssso",
  "ostssssyysssto",
  "osssssssysssso",
  "osstsyssssssso",
  "oooooooooooooo",
];

export const PAINTING: readonly string[] = [
  "oooooooooooooooooo",
  "osssssssssssssssso",
  "osssrssssssssrsrso",
  "osrrrrssssssrrrrso",
  "osrssrssbsssrssrso",
  "osssssssbbssssssso",
  "osssssssbsbsssssso",
  "osssssssbssssssyso",
  "osssssssssssssssso",
  "oooooooooooooooooo",
];

export const ROCKS: readonly string[] = [
  "....oooo....",
  "...ossSso...",
  "..ossssSSo..",
  ".oossSSoooo.",
  "ossSSSossSo.",
  "oSSSSSoSSSSo",
  "oooooooooooo",
];

export const CAMPFIRE: readonly string[] = [
  ".......f.......",
  "......fFf......",
  ".....fFyFf.....",
  "....fFyyyFf....",
  "....FfyyyfF....",
  ".....FFfFF.....",
  "..oooooooooo...",
  ".oddDDdddDDdo..",
  "..oooooooooooo.",
  ".odDdddDDdddDo.",
  "..oooooooooo...",
];

export const BANNER: readonly string[] = [
  "oooooooooo",
  "oyrrrrrryo",
  ".orrrrrro.",
  ".orryyrro.",
  ".oryyyyro.",
  ".orryyrro.",
  ".orrrrrro.",
  ".orrrrrro.",
  ".orrRRrro.",
  ".orRooRro.",
  ".oRo..oRo.",
  ".oo....oo.",
];

export const SHIELD: readonly string[] = [
  "oooooooooo",
  "obbbbyyyyo",
  "obbbbyyyyo",
  "obbbbyyyyo",
  "oyyyybbbbo",
  "oyyyybbbbo",
  ".oyyybbbo.",
  "..oyybbo..",
  "...oooo...",
];

export const SWORDS: readonly string[] = [
  "k.........k",
  ".k.......k.",
  "..k.....k..",
  "...k...k...",
  "....k.k....",
  ".....k.....",
  "....yky....",
  "...y...y...",
  "..d.....d..",
  ".d.......d.",
];

export const TAPESTRY: readonly string[] = [
  "dddddddddddddddddddd",
  "orrrrrrrrrrrrrrrrrro",
  "oryyyyyyyyyyyyyyyyro",
  "orybbbbbbbbbbbbbbyro",
  "orybbbykkybbbbbbbyro",
  "orybbykkkkybbbbbbyro",
  "orybbbykkybbbbybbyro",
  "orybbbbbbbbbbbyybyro",
  "oryyyyyyyyyyyyyyyyro",
  "orrrrrrrrrrrrrrrrrro",
  ".oro.oro.oro.oro.ro.",
];

export const BARREL: readonly string[] = [
  "..oooooo..",
  ".oddddddo.",
  "okkkkkkkko",
  "oddDddDddo",
  "oddDddDddo",
  "okkkkkkkko",
  "oddDddDddo",
  "oddDddDddo",
  "okkkkkkkko",
  ".oddddddo.",
  "..oooooo..",
];

export const ARMOR: readonly string[] = [
  "...oooo...",
  "..okkkko..",
  "..okooko..",
  "..okkkko..",
  ".oooooooo.",
  "okKkkkkKko",
  "okKkkkkKko",
  "oko.kk.oko",
  "oo.okko.oo",
  "...okko...",
  "...oKKo...",
  "..okoook..",
  "..oko.oko.",
  ".ookoo.ooo",
];

export const THRONE: readonly string[] = [
  "......oooooo......",
  ".....oyyyyyyo.....",
  "....oyrrrrrryo....",
  "....oyrrrrrryo....",
  "....oyrrrrrryo....",
  "....oyrrrrrryo....",
  "..oooyrrrrrryooo..",
  "..oyyyrrrrrryyyo..",
  "..oyoooooooooyo...",
  "..oyrrrrrrrrrryo..",
  "..oyyyyyyyyyyyyo..",
  "..oyo........oyo..",
  "..ooo........ooo..",
];

export const CANDLE: readonly string[] = [
  "...f...",
  "..fFf..",
  "...w...",
  "...w...",
  "o.oyo.o",
  "oyyyyyo",
  "...y...",
  "...y...",
  "...y...",
  "..oyo..",
  ".oyyyo.",
  "ooooooo",
];

export const HORSESHOE: readonly string[] = [
  ".oooooo.",
  "okkkkkko",
  "okKooKko",
  "oko..oko",
  "oko..oko",
  "oko..oko",
  "oo....oo",
];

export const SACKS: readonly string[] = [
  "...oo......oo....",
  "..owwo....owwo...",
  ".owwwwo..owwwwo..",
  "owwwwwwooowwwwwo.",
  "owwkkwwoowwkkwwo.",
  "owwwwwwoowwwwwwo.",
  "owwwwwwoowwwwwwo.",
  ".oooooo..oooooo..",
];

export const TOOLS: readonly string[] = [
  "oooooooooooooooo",
  "oddddddddddddddo",
  "oooooooooooooooo",
  "..k.k.k..ok.....",
  "..kkkkk..kk.....",
  "....d....kk.....",
  "....d.....d.....",
  "....d.....d.....",
  "....d.....d.....",
  "....d.....d.....",
  "....d.....d.....",
];

export const SUNFLOWER: readonly string[] = [
  "..yyyy...",
  ".yYbbYy..",
  "yYbbbbYy.",
  "yYbbbbYy.",
  ".yYbbYy..",
  "..yyyy...",
  "....g..gg",
  "gg..g.gg.",
  ".gg.gg...",
  "...gg....",
  "..ooooo..",
  "..oDDDo..",
  "..ooooo..",
];

export const MILKCAN: readonly string[] = [
  "..oooo..",
  "..okko..",
  ".oooooo.",
  "okkkkkko",
  "okKkkkko",
  "okKkkkko",
  "okkkkkko",
  "okrrrrko",
  "okkkkkko",
  "oooooooo",
];

export const WELL: readonly string[] = [
  ".oooooooooooooo.",
  "ooRRRRRRRRRRRRoo",
  "..od........do..",
  "..od...ko...do..",
  "..od...kk...do..",
  "..od...oo...do..",
  "osssssssssssssso",
  "osSssSssSssSssso",
  "osssssssssssssso",
  "osSssSssSssSssso",
  "oooooooooooooooo",
];

export const FENCE: readonly string[] = [
  ".o....o....o.",
  "odo..odo..odo",
  "odooooodooodo",
  "odddddddddddo",
  "odooooodooodo",
  "odddddddddddo",
  "odooooodooodo",
  "odo..odo..odo",
  "ooo..ooo..ooo",
];

export const HAYBALE: readonly string[] = [
  "oooooooooooo",
  "ojjjjjjjjjjo",
  "ohhHhrhhHhho",
  "ohHhhrhHhhho",
  "ohhhHrhhhHho",
  "oHhhhrhhHhho",
  "oooooooooooo",
];

export const PORTHOLE: readonly string[] = [
  "....oooooo....",
  "..ookkkkkkoo..",
  ".okksdddddkko.",
  ".okdddddsddko.",
  "okddppPddddkko",
  "okdpppPPddsdko",
  "okdpPPPPdddko.",
  "okddPPPdddddko",
  ".okddddsdddko.",
  ".okksddddddko.",
  "..ookkkkkkoo..",
  "....oooooo....",
];

export const PANEL: readonly string[] = [
  "oooooooooooo",
  "ommmmmmmmmmo",
  "omgyrggyrgmo",
  "ommmmmmmmmmo",
  "omddddddddmo",
  "omdcdcdcddmo",
  "omddddddddmo",
  "ommmmmmmmmmo",
  "oooooooooooo",
];

export const LOCKER: readonly string[] = [
  "oooooooo",
  "ommmmmmo",
  "omddddmo",
  "omddddmo",
  "ommmmmmo",
  "ommmmMmo",
  "ommmmMmo",
  "ommmmmmo",
  "ommmmmmo",
  "omMMMMmo",
  "ommmmmmo",
  "oooooooo",
];

export const HOLO: readonly string[] = [
  "oooooooooooooooooooo",
  "occcccccccccccccccco",
  "ocddddddddddddddddco",
  "ocdddddddddddccdddco",
  "ocddddddddcccddcddco",
  "ocddddcccddddddcddco",
  "ocddccddddddddddddco",
  "ocdddddddddddddddcco",
  "occcccccccccccccccco",
  "oooooooooooooooooooo",
];

export const ALIEN: readonly string[] = [
  "..a.....a...",
  ".aAa...aAa..",
  "..aAa.aAa...",
  "...aAaAa..a.",
  ".a..aAa..aA.",
  ".Aa.aAa.aA..",
  "..AaaAaaA...",
  "....aAa.....",
  "..oooooooo..",
  "..ommmmmmo..",
  "...oMmmMo...",
  "...oooooo...",
];

export const TANK: readonly string[] = [
  "...oo...",
  "..okko..",
  ".oooooo.",
  "obbbbbbo",
  "obBbbbbo",
  "obBbbbbo",
  "obbbbbbo",
  "owwwwwwo",
  "obbbbbbo",
  "obBbbbbo",
  "obbbbbbo",
  "oooooooo",
];

export const PAD: readonly string[] = [
  "......cccccccccc......",
  "....cc..........cc....",
  "...c..............c...",
  "....cc..........cc....",
  "......cccccccccc......",
  "..oooooooooooooooooo..",
  ".ommmmmmmmmmmmmmmmmmo.",
  "omMgMMMMMyMMMMMMMgMMmo",
  "oooooooooooooooooooooo",
];

export const ROBOT: readonly string[] = [
  "....g.....",
  "....o.....",
  "..oooooo..",
  "..olllllo.",
  "..olclclo.",
  "..olllllo.",
  "..oooooo..",
  ".oommmmoo.",
  "omomrgmomo",
  "omommmmomo",
  "o.ommmmo.o",
  "..oo..oo..",
  "..om..mo..",
  ".ooo..ooo.",
];

export const KELP: readonly string[] = [
  "..k.....",
  ".kK..k..",
  ".kK.kK..",
  "..kKk...",
  "..kK....",
  ".kKk..k.",
  ".kK..kK.",
  "..kKkK..",
  "...kK...",
  "..kKk...",
  "..kK....",
  ".ooooo..",
];

export const CORAL: readonly string[] = [
  ".c....c...c.",
  ".cc..cC..cc.",
  "..cC.cC.cC..",
  "c..cCcCcC..c",
  "cC..cCCc..cC",
  ".cCccCCcccC.",
  "...ccCCcc...",
  "....cCCc....",
  "..oooooooo..",
  ".orrrRrrrro.",
  "oooooooooooo",
];

export const CHEST: readonly string[] = [
  "..oooooooooooo..",
  ".oddddddddddddo.",
  "oddddddddddddddo",
  "oyyyyyyyyyyyyyyo",
  "oddddddoyodddddo",
  "oddddddoYodddddo",
  "oddddddooodddddo",
  "oddddddddddddddo",
  "oyyyyyyyyyyyyyyo",
  "oooooooooooooooo",
];

export const ANCHOR: readonly string[] = [
  "....ooo....",
  "....omo....",
  "....ooo....",
  "..ooooooo..",
  "....omo....",
  "....omo....",
  "....omo....",
  "o...omo...o",
  "om..omo..mo",
  ".om.omo.mo.",
  "..ommmmmo..",
  "...ooooo...",
];

export const WHEEL: readonly string[] = [
  ".....o.....",
  "..o..d..o..",
  "...odddo...",
  "..oDo.oDo..",
  "oddo...oddo",
  "..oDo.oDo..",
  "...odddo...",
  "..o..d..o..",
  ".....o.....",
];

export const OPENCHEST: readonly string[] = [
  "..oooooooooooo..",
  ".oddddddddddddo.",
  "odDDDDDDDDDDDDdo",
  "oyyyyyyyyyyyyyyo",
  "oyYyYyyyYyYyyYyo",
  "oyyyyyYyyyyyyyyo",
  "oddddddoyodddddo",
  "oddddddoYodddddo",
  "oddddddddddddddo",
  "oyyyyyyyyyyyyyyo",
  "oooooooooooooooo",
];

export const STARFISH: readonly string[] = [
  "....o....",
  "...ofo...",
  "oooofoooo",
  ".offfffo.",
  "..offfo..",
  ".offoffo.",
  "ofo...ofo",
  "oo.....oo",
];

export const FROST: readonly string[] = [
  "oooooooooooooooooooooooo",
  "owwwwwwwwwwwwwwwwwwwwwwo",
  "owbbbbbbbbbowbbbbbbbbbwo",
  "owbjbbbjbbbowbbbjbbbbbwo",
  "owbbbbbbbbbowbbbbbbbjbwo",
  "owbbjbbbbbbowbbjbbbbbbwo",
  "owbbbbbbjbbowbbbbbbbbbwo",
  "owjjjjjjjjjowjjjjjjjjjwo",
  "owwwwwwwwwwwwwwwwwwwwwwo",
  "oooooooooooooooooooooooo",
  ".j.j..j..j.j..j...j..j..",
  ".j....j....j......j.....",
];

export const SKIS: readonly string[] = [
  "o.o..o.o",
  "rorororo",
  "rorororo",
  "rorororo",
  "rorororo",
  "rorororo",
  "rorororo",
  "rorororo",
  "rorororo",
  "rorororo",
  "oooooooo",
];

export const WOODPILE: readonly string[] = [
  "....oooo..oooo....",
  "...odDdDooDdDdo...",
  "...oDdDdooDdDdo...",
  ".oooooooooooooooo.",
  "odDdDooDdDdooDdDdo",
  "oDdDdooDdDdooDdDdo",
  "oooooooooooooooooo",
];

export const SLED: readonly string[] = [
  "........o...",
  "oooooooooo..",
  "oddddddddoo.",
  "oDDDDDDDDo..",
  "oooooooooo..",
  ".o..o..o..o.",
  "okkkkkkkkkko",
  ".oooooooooo.",
];

export const PINE: readonly string[] = [
  "....p....",
  "...pPp...",
  "..ppPpp..",
  "...pPp...",
  "..pPPPp..",
  ".ppPPPpp.",
  "..pPPPp..",
  ".pPPPPPp.",
  "ppPPPPPpp",
  "....d....",
  "...ooo...",
];

export const SNOWMAN: readonly string[] = [
  "...ooo....",
  "..okkko...",
  ".ooooooo..",
  "..osssso..",
  "..okssko..",
  "..osscso..",
  "..obbbbo..",
  ".osbbsss.o",
  "ossskssso.",
  "osssssssso",
  "osssksssso",
  ".osssssso.",
  "..oooooo..",
];

export const BIGTREE: readonly string[] = [
  ".......y.......",
  "......yYy......",
  "......pPp......",
  ".....pPrPp.....",
  "....pPPPPyp....",
  ".....pPPPp.....",
  "....pPyPPPp....",
  "...ppPPPPrPp...",
  "..pPPrPPPPPPp..",
  "...pPPPPyPPp...",
  "..pPyPPPPPPrPp.",
  ".ppPPPPPPPPPPpp",
  "......ddd......",
  ".....ooooo.....",
];

export const GLYPHS: readonly string[] = [
  "oooooooooooooo",
  "osssssssssssso",
  "osgsssggsssgso",
  "osgsssgsgsggso",
  "osggssggsssgso",
  "osssssssssssso",
  "osyyssgsssyyso",
  "osysssggssysso",
  "osssssssssssso",
  "oooooooooooooo",
];

export const URN: readonly string[] = [
  "..oooo..",
  "..ouuo..",
  ".ouuuuo.",
  "ouuyyuuo",
  "ouuuuuuo",
  "oUuuuuUo",
  ".oUuuUo.",
  "..oooo..",
];

export const SARCO: readonly string[] = [
  "..oooooo..",
  ".oyyyyyyo.",
  "oyysyysyyo",
  "oyyyyyyyyo",
  ".oyyYYyyo.",
  ".oybgbgyo.",
  ".oygbgbyo.",
  ".oybgbgyo.",
  ".oyyyyyyo.",
  ".oyYyyYyo.",
  ".oyyyyyyo.",
  "..oooooo..",
];

export const FRIEZE: readonly string[] = [
  "oooooooooooooooooooooo",
  "osssssssssssssssssssso",
  "osyyyyyyyyyyyyyyyyyyso",
  "osssgsssssgsssssssgsso",
  "ossgggsssgggssgsssgsso",
  "osssgsssssgsssgggsgsso",
  "ossgsgsssgsgsssgssgsso",
  "osyyyyyyyyyyyyyyyyyyso",
  "osssssssssssssssssssso",
  "oooooooooooooooooooooo",
];

export const CACTUS: readonly string[] = [
  "....oo....",
  "...occo...",
  ".o.ocCo...",
  "oco.cCo.o.",
  "oCo.cCooco",
  "oCcccCocCo",
  ".oCCcCcCo.",
  "...ocCoo..",
  "...ocCo...",
  "..oooooo..",
  "..ouuuuo..",
  "...oUUo...",
  "...oooo...",
];

export const OASIS: readonly string[] = [
  "......ppp..pp...........",
  "....ppPPpppPPpp.........",
  "...pP...dPp...Pp........",
  "........d...............",
  ".........d..............",
  ".........d..............",
  "....oooo.d.oooooooo.....",
  "..ooaaaaadaaaaaaaaaoo...",
  ".oaaaAaaadaaaAAaaaaaao..",
  "oaaAaaaaaaaaaaaaaAaaaao.",
  ".ooaaaaAaaaaAaaaaaaaoo..",
  "...ooooooooooooooooo....",
];

export const OBELISK: readonly string[] = [
  "...oo...",
  "..oyyo..",
  "..osso..",
  "..osgo..",
  "..osso..",
  "..ogso..",
  ".ossgso.",
  ".osssso.",
  ".osgsso.",
  ".osssso.",
  "oooooooo",
  "oSSSSSSo",
  "oooooooo",
];

export const PALM: readonly string[] = [
  "..pp...pp..",
  ".pPPp.pPPp.",
  "p...pdp...p",
  ".....d.....",
  ".....d.....",
  "....d......",
  "....d......",
  "....d......",
  "...ooo.....",
  "..ouuuo....",
  "..oUUUo....",
  "..ooooo....",
];
