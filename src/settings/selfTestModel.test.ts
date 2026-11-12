import { describe, expect, it } from "vitest";
import { itemLabelKey, statusKey, worstStatus, type SelfTestItem } from "./selfTestModel";

const item = (status: SelfTestItem["status"]): SelfTestItem => ({ id: "ping", status, detail: "" });

describe("selfTestModel", () => {
  it("summarises by the worst status and ignores skips", () => {
    expect(worstStatus([])).toBe("ok");
    expect(worstStatus([item("ok"), item("skip")])).toBe("ok");
    expect(worstStatus([item("ok"), item("warn"), item("skip")])).toBe("warn");
    expect(worstStatus([item("warn"), item("fail"), item("ok")])).toBe("fail");
  });

  it("maps check ids and statuses to i18n keys, with a fallback for unknown ids", () => {
    expect(itemLabelKey("wrong_token")).toBe("settings.activity.st.wrong_token");
    expect(itemLabelKey("hooks_dir")).toBe("settings.activity.st.hooks_dir");
    expect(itemLabelKey("from-a-newer-backend")).toBe("settings.activity.st.receiver");
    expect(statusKey("skip")).toBe("settings.activity.st.skip");
  });
});
