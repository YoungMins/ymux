// Refuse to boot inside a frame. MUST stay the first import of `main.ts`:
// ES modules evaluate in import order, so this runs before any other module's
// side effects, and throwing here aborts the whole module graph before
// anything can reach the IPC. See `frameGuard.ts` for why.
import { isTopLevelWindow } from "./frameGuard";

if (!isTopLevelWindow(window)) {
  document.documentElement.replaceChildren();
  throw new Error("ymux refuses to run inside a frame");
}
