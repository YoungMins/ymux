import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { ATTACH_LIMITS } from "./activityModel";

// The Settings caps must equal what webhooks/mod.rs clamps to, or the UI
// offers values the backend silently lowers. Reads the Rust constants.
const rs = readFileSync(
  fileURLToPath(new URL("../../src-tauri/src/webhooks/mod.rs", import.meta.url)),
  "utf8",
);

function rustConst(name: string): number {
  const m = new RegExp(`pub const ${name}: u32 = ([0-9]+);`).exec(rs);
  if (!m) throw new Error(`webhooks/mod.rs: const ${name} not found`);
  return Number(m[1]);
}

describe("attachment limits", () => {
  it("match the backend clamps", () => {
    expect(ATTACH_LIMITS.maxFileMb).toBe(rustConst("MAX_ATTACH_FILE_MB"));
    expect(ATTACH_LIMITS.maxTotalMb).toBe(rustConst("MAX_ATTACH_TOTAL_MB"));
    expect(ATTACH_LIMITS.maxFiles).toBe(rustConst("MAX_ATTACH_FILES"));
  });
});
