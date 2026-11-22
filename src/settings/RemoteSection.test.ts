// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import type { RemoteStatus } from "../types";

const calls: string[] = [];
const shared: boolean[] = [];
const pictures: boolean[] = [];
let status: RemoteStatus;

vi.mock("../ipc/bridge", () => ({
  describeError: (e: unknown) => String(e),
  onRemoteStatus: () => Promise.resolve(() => {}),
  remoteApi: {
    status: () => Promise.resolve(status),
    configure: (c: { serverUrl: string; shareChat: boolean; shareImages: boolean }) => {
      calls.push(`configure:${c.serverUrl}`);
      shared.push(c.shareChat);
      pictures.push(c.shareImages);
      status = { ...status, server_url: c.serverUrl };
      return Promise.resolve(status);
    },
    setToken: (token: string) => {
      calls.push(`setToken:${token}`);
      status = { ...status, has_token: token !== "" };
      return Promise.resolve(status);
    },
    pairCancel: () => Promise.resolve(),
    pairStart: () => Promise.resolve(),
    deviceSetCap: () => Promise.resolve(status),
    deviceRemove: () => Promise.resolve(status),
  },
}));

import { renderRemoteSection } from "./RemoteSection";

const LINK = "https://ymux.example.com/#pair=dddddddd-dddd-4ddd-8ddd-dddddddddddd.SECRETCODE";

function base(over: Partial<RemoteStatus> = {}): RemoteStatus {
  return {
    enabled: true,
    server_url: "https://ymux.example.com",
    name: "PC",
    registered: true,
    has_token: false,
    state: "online",
    error: "",
    notice: "",
    notify_waiting: true,
    notify_done: false,
    share_chat: true,
    share_images: false,
    server_version: "",
    devices: [],
    pairing: null,
    ...over,
  };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("Settings -> Remote server", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
    calls.length = 0;
    shared.length = 0;
    pictures.length = 0;
  });

  it("shows the server version next to the connection state, and nothing when there is none", async () => {
    const shown = async (over: Partial<RemoteStatus>): Promise<string> => {
      status = base(over);
      const host = document.createElement("div");
      document.body.appendChild(host);
      const stop = renderRemoteSection(host);
      await tick();
      const visible = [...host.querySelectorAll(".act-note")].filter(
        (n) => !(n as HTMLElement).hidden && /^Server /.test(n.textContent ?? ""),
      );
      stop();
      host.remove();
      return visible.map((n) => n.textContent).join("|");
    };
    expect(await shown({ state: "online", server_version: "0.19.0" })).toBe("Server 0.19.0");
    expect(await shown({ state: "online", server_version: "" })).toBe("");
    expect(await shown({ state: "online", server_version: "<b>x</b>" })).toBe("");
    expect(await shown({ state: "connecting", server_version: "0.19.0" })).toBe("");
  });

  it("has a separate picture switch: off by default, needs conversations, applies at once", async () => {
    status = base({ share_chat: true, share_images: false });
    const host = document.createElement("div");
    document.body.appendChild(host);
    const stop = renderRemoteSection(host);
    await tick();
    const row = [...host.querySelectorAll("label")].find((l) => /pictures from conversations/i.test(l.textContent ?? ""));
    expect(row).toBeDefined();
    expect(host.textContent).toMatch(/cannot be redacted/);
    const box = row!.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    expect(box.checked).toBe(false);
    expect(box.disabled).toBe(false);
    box.checked = true;
    box.dispatchEvent(new Event("change"));
    await tick();
    await tick();
    expect(pictures).toEqual([true]);
    expect(shared).toEqual([true]);
    stop();
    // With conversations off the picture switch is unavailable.
    const host2 = document.createElement("div");
    document.body.appendChild(host2);
    status = base({ share_chat: false, share_images: true });
    const stop2 = renderRemoteSection(host2);
    await tick();
    const row2 = [...host2.querySelectorAll("label")].find((l) => /pictures from conversations/i.test(l.textContent ?? ""));
    expect(row2!.querySelector<HTMLInputElement>('input[type="checkbox"]')!.disabled).toBe(true);
    stop2();
  });

  it("shows the conversation switch from the status and applies a change at once", async () => {
    status = base({ share_chat: true });
    const host = document.createElement("div");
    document.body.appendChild(host);
    const stop = renderRemoteSection(host);
    await tick();
    const labels = [...host.querySelectorAll("label")];
    const row = labels.find((l) => /agent conversations/i.test(l.textContent ?? ""));
    expect(row).toBeDefined();
    expect(host.textContent).toMatch(/end-to-end encrypted/);
    const box = row!.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    expect(box.checked).toBe(true);
    box.checked = false;
    box.dispatchEvent(new Event("change"));
    await tick();
    await tick();
    expect(shared).toEqual([false]);
    stop();
  });

  it("shows the pairing QR code but never the link as text", async () => {
    status = base({ pairing: { url: LINK, expires_ms: Date.now() + 100_000 } });
    const host = document.createElement("div");
    document.body.appendChild(host);
    const stop = renderRemoteSection(host);
    await tick();
    expect(host.querySelector(".remote-pair svg")).not.toBeNull();
    expect(host.textContent).not.toContain("SECRETCODE");
    expect(host.textContent).not.toContain("#pair=");
    expect(host.querySelector(".remote-pair__url")).toBeNull();
    // The card keeps the countdown and Cancel.
    expect(host.querySelector(".remote-pair")?.textContent).toMatch(/\d:\d\d/);
    expect(host.querySelectorAll(".remote-pair button")).toHaveLength(1);
    // Nothing in the markup carries the link either (attributes, titles).
    expect(host.innerHTML).not.toContain("SECRETCODE");
    stop();
  });

  it("tells the user to pair again after the identity was reset, and only then", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    status = base({ notice: "identity_reset", registered: false });
    const stop = renderRemoteSection(host);
    await tick();
    const shown = [...host.querySelectorAll(".act-note")].filter(
      (n) => !(n as HTMLElement).hidden && /identity key was lost/.test(n.textContent ?? ""),
    );
    expect(shown).toHaveLength(1);
    stop();
    host.remove();
    const host2 = document.createElement("div");
    document.body.appendChild(host2);
    status = base();
    const stop2 = renderRemoteSection(host2);
    await tick();
    expect(host2.textContent).not.toMatch(/identity key was lost/);
    stop2();
  });

  it("says plainly that the addresses differ for origin_mismatch, and shows no secret", async () => {
    status = base({ state: "error", error: "origin_mismatch", has_token: true });
    const host = document.createElement("div");
    document.body.appendChild(host);
    const stop = renderRemoteSection(host);
    await tick();
    const line = host.querySelector(".act-note--error")?.textContent ?? "";
    expect(line).toContain("do not match");
    expect(line).toContain("YMUX_SERVER_PUBLIC_URL");
    stop();
  });

  it("saves the address first and the token after it", async () => {
    status = base({ has_token: false });
    const host = document.createElement("div");
    document.body.appendChild(host);
    const stop = renderRemoteSection(host);
    await tick();
    const inputs = host.querySelectorAll<HTMLInputElement>("input.remote-wide");
    inputs[0].value = "https://other.example.com";
    host.querySelector<HTMLInputElement>('input[type="password"]')!.value = "new-token";
    host.querySelector<HTMLButtonElement>(".hotkey-modal__btn--primary")!.click();
    await tick();
    await tick();
    expect(calls).toEqual(["configure:https://other.example.com", "setToken:new-token"]);
    stop();
  });

  it("only configures when no token was typed", async () => {
    status = base();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const stop = renderRemoteSection(host);
    await tick();
    host.querySelector<HTMLButtonElement>(".hotkey-modal__btn--primary")!.click();
    await tick();
    await tick();
    expect(calls).toEqual(["configure:https://ymux.example.com"]);
    stop();
  });
});
