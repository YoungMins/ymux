import { describe, it, expect } from "vitest";
import { langDir, t, translationGaps } from "./i18n";

describe("i18n completeness (rule 7)", () => {
  it("has every key in all 13 languages", () => {
    expect(translationGaps("")).toEqual([]);
  });

  it("marks only Arabic as right-to-left", () => {
    expect(langDir("ar")).toBe("rtl");
    expect(langDir("en")).toBe("ltr");
    expect(langDir("ja")).toBe("ltr");
  });

  it("has every files pane string in all 13 languages", () => {
    expect(t("files.title")).toBe("Files");
    expect(translationGaps("files.")).toEqual([]);
  });

  it("has every editor pane string in all 13 languages", () => {
    expect(t("editor.save")).toBe("Save");
    expect(translationGaps("editor.")).toEqual([]);
    expect(translationGaps("shortcut.save")).toEqual([]);
    expect(translationGaps("shortcut.gotoLine")).toEqual([]);
  });

  it("has every \"+\" launcher string in all 13 languages", () => {
    expect(t("launcher.agents")).toBe("Agents");
    expect(translationGaps("launcher.")).toEqual([]);
  });

  it("has every Activity & notifications string in all 13 languages", () => {
    expect(t("settings.section.activity")).toBe("Activity & notifications");
    expect(translationGaps("settings.activity.")).toEqual([]);
    expect(translationGaps("settings.section.activity")).toEqual([]);
  });

  it("defines every settings.activity key the section uses", async () => {
    const { readFileSync } = await import("node:fs");
    const { fileURLToPath } = await import("node:url");
    const src = readFileSync(
      fileURLToPath(new URL("../settings/ActivitySection.ts", import.meta.url)),
      "utf8",
    );
    const keys = [...src.matchAll(/t\("(settings\.activity\.[A-Za-z0-9_.]+)"\)/g)].map((m) => m[1]);
    expect(keys.length).toBeGreaterThan(50);
    expect(keys.filter((k) => t(k) === k)).toEqual([]);
    // Template keys: one per format / event / attachment group.
    for (const k of [
      ...["json", "slack", "discord", "teams"].flatMap((f) => [`format.${f}`, `formatNote.${f}`]),
      "event.turn_end", "event.attention",
      ...["images", "pdf", "audio", "video", "text", "office"].map((g) => `group.${g}`),
    ]) {
      expect(t(`settings.activity.${k}`)).not.toBe(`settings.activity.${k}`);
    }
  });

  it("has every Remote server string in all 13 languages", () => {
    expect(t("settings.section.remote")).toBe("Remote server");
    expect(translationGaps("settings.remote.")).toEqual([]);
    expect(translationGaps("settings.section.remote")).toEqual([]);
  });

  it("defines every settings.remote key the section and pair dialog use", async () => {
    const { readFileSync } = await import("node:fs");
    const { fileURLToPath } = await import("node:url");
    const read = (p: string) => readFileSync(fileURLToPath(new URL(p, import.meta.url)), "utf8");
    const src =
      read("../settings/RemoteSection.ts") + read("../remote/pairDialog.ts") + read("../remote/remoteWiring.ts");
    const keys = [...src.matchAll(/t\("(settings\.remote\.[A-Za-z0-9_.]+)"\)/g)].map((m) => m[1]);
    expect(keys.length).toBeGreaterThan(30);
    expect(keys.filter((k) => t(k) === k)).toEqual([]);
    // Template keys: capabilities, and every status line the model can pick.
    for (const k of [
      ...["view", "respond", "prompt"].flatMap((c) => [`cap.${c}`, `capDesc.${c}`]),
      ...["off", "connecting", "online", "unavailable"].map((s) => `state.${s}`),
      ...["bad_token", "key_mismatch", "version", "bad_sig", "limit", "origin_mismatch", "other"].map(
        (e) => `err.${e}`,
      ),
      ...["pairReq.type", "pairReq.wrong", "pairReq.refused", "pairReq.failed", "pairConflict", "pairScan"],
    ]) {
      expect(t(`settings.remote.${k}`)).not.toBe(`settings.remote.${k}`);
    }
  });

  it("has every backend error kind in all 13 languages", () => {
    expect(translationGaps("fsError.")).toEqual([]);
  });
});

describe("no hard-coded shortcuts in translations", () => {
  it("leaves shortcut text to shortcutLabel() at the use site, so macOS shows Cmd", async () => {
    const { readFileSync } = await import("node:fs");
    const { fileURLToPath } = await import("node:url");
    const src = readFileSync(fileURLToPath(new URL("./i18n.ts", import.meta.url)), "utf8");
    expect(src.match(/\b(Ctrl|Alt)\+\S+/g) ?? []).toEqual([]);
    expect(translationGaps("browser.zoom")).toEqual([]);
    expect(translationGaps("notes.buttonTitle")).toEqual([]);
  });

  it("has every terminal exit / restart / spawn / paste message in all 13 languages", () => {
    for (const k of [
      "terminal.processExited",
      "terminal.restartHint",
      "terminal.spawnFailed",
      "terminal.pasteImageFailed",
      "terminal.pastePathUnsupported",
    ]) {
      expect(translationGaps(k)).toEqual([]);
    }
  });
});
