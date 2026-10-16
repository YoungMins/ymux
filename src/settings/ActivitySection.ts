// Settings -> "Activity & notifications": the activity log, the turn-summary
// provider and the webhook destination list. Pure decisions live in
// `activityModel.ts`; this file is DOM + IPC only.
//
// Rules this file keeps:
//  - Every dynamic string goes in through textContent / value, never innerHTML.
//  - URL / HMAC secret / API key inputs are write-only. Nothing here ever
//    reads a secret back; the backend only reports "set" and a scheme+host hint.
//  - `webhook_set_secret`, `webhook_test` and `webhook_preview` look the
//    destination up in the SAVED config, so every one of them first commits the
//    local copy and awaits `flushPendingSave()`.

import { getLang, t } from "../i18n/i18n";
import { activityApi, describeError, summarizerApi, webhookApi } from "../ipc/bridge";
import { askConfirm, open as openDialog } from "../ui/Dialog";
import type { WorkspaceManager } from "../workspace/WorkspaceManager";
import type {
  ActivityConfig,
  SummarizerConfig,
  WebhookDestStatus,
  WebhookDestination,
} from "../types";
import {
  ATTACH_LIMITS,
  ATTACH_GROUPS,
  WEBHOOK_EVENTS,
  WEBHOOK_FORMATS,
  clampInt,
  endpointOnProviderChange,
  formatBytes,
  groupEnabled,
  isRelativeReportDir,
  newDestination,
  recentTurns,
  setGroup,
} from "./activityModel";

const STATUS_POLL_MS = 5000;
const RECENT_TURNS = 20;

type Tone = "" | "ok" | "warn" | "error";

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls = "",
  text?: string,
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function note(text: string, tone: Tone = ""): HTMLElement {
  return el("div", `act-note${tone ? ` act-note--${tone}` : ""}`, text);
}

function setNote(e: HTMLElement, text: string, tone: Tone = ""): void {
  e.className = `act-note${tone ? ` act-note--${tone}` : ""}`;
  e.textContent = text;
}

/// label | control, with optional hint lines underneath the control.
function field(label: string, control: HTMLElement, ...hints: HTMLElement[]): HTMLElement {
  const row = el("div", "act-row");
  row.appendChild(el("div", "act-row__label", label));
  const box = el("div", "act-row__control");
  box.appendChild(control);
  for (const h of hints) box.appendChild(h);
  row.appendChild(box);
  return row;
}

function button(label: string, primary = false): HTMLButtonElement {
  const b = el("button", `hotkey-modal__btn${primary ? " hotkey-modal__btn--primary" : ""}`, label);
  b.type = "button";
  return b;
}

function textInput(value: string, placeholder = "", wide = true): HTMLInputElement {
  const i = el("input", "settings-hex-input");
  i.type = "text";
  i.spellcheck = false;
  i.value = value;
  i.placeholder = placeholder;
  if (wide) i.style.width = "100%";
  return i;
}

function numberInput(value: number, min: number, max: number): HTMLInputElement {
  const i = el("input", "settings-hex-input act-num");
  i.type = "number";
  i.min = String(min);
  i.max = String(max);
  i.step = "1";
  i.value = String(value);
  return i;
}

function secretInput(placeholder: string): HTMLInputElement {
  const i = el("input", "settings-hex-input");
  i.type = "password";
  i.autocomplete = "off";
  i.spellcheck = false;
  i.placeholder = placeholder;
  i.style.width = "100%";
  return i;
}

function checkbox(checked: boolean, label: string): { wrap: HTMLElement; box: HTMLInputElement } {
  const wrap = el("label", "act-check");
  const box = el("input");
  box.type = "checkbox";
  box.checked = checked;
  wrap.append(box, el("span", "", label));
  return { wrap, box };
}

function select(
  options: readonly { value: string; label: string }[],
  value: string,
): HTMLSelectElement {
  const s = el("select", "settings-hex-input");
  s.style.width = "auto";
  for (const o of options) {
    const opt = el("option", "", o.label);
    opt.value = o.value;
    s.appendChild(opt);
  }
  s.value = value;
  return s;
}

function when(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return t("settings.activity.never");
  return new Date(ms).toLocaleString();
}

export interface ActivitySectionOpts {
  /// Jump to Settings -> General (where the agent tracking toggle lives).
  goToGeneral: () => void;
}

/// Renders into `host`; the returned function stops the status polling.
export function renderActivitySection(
  host: HTMLElement,
  manager: WorkspaceManager,
  opts: ActivitySectionOpts,
): () => void {
  let disposed = false;
  const statusEls = new Map<string, HTMLElement>();

  host.appendChild(el("h3", "", t("settings.activity.heading")));
  host.appendChild(buildActivity());
  host.appendChild(buildSummarizer());
  host.appendChild(buildDestinations());

  const poll = async (): Promise<void> => {
    if (disposed || statusEls.size === 0) return;
    try {
      const all = await webhookApi.status();
      if (disposed) return;
      for (const [id, box] of statusEls) paintStatus(box, all[id]);
    } catch {
      /* status is informational */
    }
  };
  const timer = window.setInterval(() => void poll(), STATUS_POLL_MS);
  void poll();

  return () => {
    disposed = true;
    window.clearInterval(timer);
  };

  // ── Activity log ──────────────────────────────────────────────────────
  function buildActivity(): HTMLElement {
    const block = el("div", "act-block");
    block.appendChild(el("h4", "", t("settings.activity.logHeading")));

    const act: ActivityConfig = { ...manager.activityConfig };
    const commit = (): void => {
      act.language = getLang();
      manager.setActivityConfig({ ...act });
    };

    const enable = checkbox(act.enabled, t("settings.activity.logEnable"));
    enable.box.addEventListener("change", () => {
      act.enabled = enable.box.checked;
      commit();
    });
    const trackingWarn = el("div", "act-note act-note--warn");
    if (!manager.agentTracking) {
      trackingWarn.append(el("span", "", `${t("settings.activity.needTracking")} `));
      const go = button(t("settings.activity.openGeneral"));
      go.addEventListener("click", opts.goToGeneral);
      trackingWarn.appendChild(go);
    } else {
      trackingWarn.hidden = true;
    }
    block.appendChild(
      field(
        t("settings.activity.logEnableLabel"),
        enable.wrap,
        note(t("settings.activity.logEnableHint")),
        trackingWarn,
      ),
    );

    const retention = numberInput(act.retention_days, 1, 365);
    retention.addEventListener("change", () => {
      act.retention_days = clampInt(retention.value, 1, 365, act.retention_days);
      retention.value = String(act.retention_days);
      commit();
    });
    block.appendChild(field(t("settings.activity.retention"), retention));

    const prompts = checkbox(act.record_prompts, t("settings.activity.recordPrompts"));
    prompts.box.addEventListener("change", () => {
      act.record_prompts = prompts.box.checked;
      commit();
    });
    block.appendChild(
      field(
        t("settings.activity.recordPromptsLabel"),
        prompts.wrap,
        note(t("settings.activity.recordPromptsWarn"), "warn"),
      ),
    );

    const reportDir = textInput(act.report_dir, ".ymux/report");
    const reportErr = note("", "error");
    reportDir.addEventListener("change", () => {
      const v = reportDir.value.trim();
      if (!isRelativeReportDir(v)) {
        setNote(reportErr, t("settings.activity.reportDirInvalid"), "error");
        return;
      }
      setNote(reportErr, "");
      act.report_dir = v === "" ? ".ymux/report" : v;
      reportDir.value = act.report_dir;
      commit();
    });
    block.appendChild(
      field(
        t("settings.activity.reportDir"),
        reportDir,
        note(t("settings.activity.reportDirHint")),
        reportErr,
      ),
    );

    const actions = el("div", "act-actions");
    const openBtn = button(t("settings.activity.openDir"));
    const clearBtn = button(t("settings.activity.clearLog"));
    const actionMsg = note("");
    openBtn.addEventListener("click", () => {
      activityApi.openDir().catch((e) => setNote(actionMsg, describeError(e), "error"));
    });
    clearBtn.addEventListener("click", async () => {
      if (!(await askConfirm(t("settings.activity.clearConfirm"), t("settings.activity.clearLog")))) {
        return;
      }
      try {
        const { files } = await activityApi.clear();
        setNote(actionMsg, t("settings.activity.cleared").replace("{n}", String(files)), "ok");
        void loadRecent();
      } catch (e) {
        setNote(actionMsg, describeError(e), "error");
      }
    });
    actions.append(openBtn, clearBtn);
    block.append(actions, actionMsg);

    // Recent turns.
    const recentHead = el("div", "act-subhead");
    recentHead.appendChild(el("span", "", t("settings.activity.recentHeading")));
    const refresh = button(t("settings.activity.refresh"));
    recentHead.appendChild(refresh);
    const list = el("ul", "act-turns");
    block.append(recentHead, list);

    const paneName = (id: string): string => {
      const title = manager.getPaneSpec(id)?.title;
      return title && title.trim() !== "" ? title : id.slice(0, 6);
    };
    async function loadRecent(): Promise<void> {
      try {
        const records = await activityApi.read(1000);
        if (disposed) return;
        const turns = recentTurns(records, RECENT_TURNS);
        list.replaceChildren();
        if (turns.length === 0) {
          list.appendChild(el("li", "act-turns__empty", t("settings.activity.recentEmpty")));
          return;
        }
        for (const turn of turns) {
          const li = el("li", "act-turns__item");
          li.appendChild(el("span", "act-turns__time", new Date(turn.ts).toLocaleString()));
          const ws = manager.workspaceName(turn.ws) || String(turn.ws);
          li.appendChild(el("span", "act-turns__where", `${ws} › ${paneName(turn.pane)}`));
          li.appendChild(el("span", "act-turns__agent", turn.agent));
          li.appendChild(
            el("span", "act-turns__headline", turn.headline || t("settings.activity.noHeadline")),
          );
          list.appendChild(li);
        }
      } catch (e) {
        list.replaceChildren(el("li", "act-turns__empty", describeError(e)));
      }
    }
    refresh.addEventListener("click", () => void loadRecent());
    void loadRecent();
    return block;
  }

  // ── Summarizer ────────────────────────────────────────────────────────
  function buildSummarizer(): HTMLElement {
    const block = el("div", "act-block");
    block.appendChild(el("h4", "", t("settings.activity.sumHeading")));
    block.appendChild(note(t("settings.activity.sumHint")));

    const sum: SummarizerConfig = { ...manager.summarizerConfig };
    const commit = (): void => manager.setSummarizerConfig({ ...sum });

    const providerSel = select(
      [
        { value: "rule", label: t("settings.activity.providerRule") },
        { value: "ollama", label: t("settings.activity.providerOllama") },
        { value: "openai", label: t("settings.activity.providerOpenai") },
      ],
      sum.provider,
    );
    block.appendChild(field(t("settings.activity.provider"), providerSel));

    const body = el("div");
    block.appendChild(body);

    const buildBody = (): void => {
      body.replaceChildren();
      if (sum.provider === "rule") return;

      const endpoint = textInput(sum.endpoint);
      endpoint.addEventListener("change", () => {
        sum.endpoint = endpoint.value.trim();
        commit();
      });
      body.appendChild(field(t("settings.activity.endpoint"), endpoint));

      const remote = checkbox(sum.allow_remote, t("settings.activity.allowRemote"));
      const remoteWarn = note(t("settings.activity.allowRemoteWarn"), "warn");
      remoteWarn.hidden = !sum.allow_remote;
      remote.box.addEventListener("change", () => {
        sum.allow_remote = remote.box.checked;
        remoteWarn.hidden = !sum.allow_remote;
        commit();
      });
      body.appendChild(field("", remote.wrap, remoteWarn));

      // Model: free text with the loaded list as suggestions.
      const model = textInput(sum.model, t("settings.activity.modelPlaceholder"));
      const listId = `act-models-${Math.random().toString(36).slice(2, 8)}`;
      const datalist = el("datalist");
      datalist.id = listId;
      model.setAttribute("list", listId);
      model.addEventListener("change", () => {
        sum.model = model.value.trim();
        commit();
      });
      const load = button(t("settings.activity.loadModels"));
      const modelMsg = note("");
      load.addEventListener("click", async () => {
        load.disabled = true;
        setNote(modelMsg, "");
        try {
          await manager.flushPendingSave();
          const names = await summarizerApi.listModels();
          datalist.replaceChildren(
            ...names.map((n) => {
              const o = el("option");
              o.value = n;
              return o;
            }),
          );
          setNote(
            modelMsg,
            t("settings.activity.modelsLoaded").replace("{n}", String(names.length)),
            names.length > 0 ? "ok" : "warn",
          );
        } catch (e) {
          setNote(modelMsg, `${t("settings.activity.modelsFailed")} ${describeError(e)}`, "error");
        } finally {
          load.disabled = false;
        }
      });
      const modelRow = el("div", "act-inline");
      modelRow.append(model, load, datalist);
      body.appendChild(field(t("settings.activity.model"), modelRow, modelMsg));

      if (sum.provider === "openai") {
        const key = secretInput(t("settings.activity.apiKeyPlaceholder"));
        const keySave = button(t("settings.activity.save"));
        const keyState = note("");
        keySave.addEventListener("click", async () => {
          keySave.disabled = true;
          try {
            const value = key.value;
            await summarizerApi.setApiKey(value);
            key.value = "";
            setNote(
              keyState,
              value === "" ? t("settings.activity.cleared1") : t("settings.activity.isSet"),
              value === "" ? "" : "ok",
            );
          } catch (e) {
            setNote(keyState, describeError(e), "error");
          } finally {
            keySave.disabled = false;
          }
        });
        const keyRow = el("div", "act-inline");
        keyRow.append(key, keySave);
        body.appendChild(
          field(
            t("settings.activity.apiKey"),
            keyRow,
            keyState,
            note(t("settings.activity.apiKeyHint")),
          ),
        );
      }

      const timeout = numberInput(sum.timeout_secs, 1, 300);
      timeout.addEventListener("change", () => {
        sum.timeout_secs = clampInt(timeout.value, 1, 300, sum.timeout_secs);
        timeout.value = String(sum.timeout_secs);
        commit();
      });
      body.appendChild(field(t("settings.activity.timeout"), timeout));

      const testBtn = button(t("settings.activity.sumTest"));
      const testMsg = note("");
      const testOut = el("pre", "act-pre");
      testOut.hidden = true;
      testBtn.addEventListener("click", async () => {
        testBtn.disabled = true;
        testOut.hidden = true;
        setNote(testMsg, t("settings.activity.testing"));
        try {
          await manager.flushPendingSave();
          const r = await summarizerApi.test();
          setNote(
            testMsg,
            r.used_llm ? t("settings.activity.testOk") : t("settings.activity.testFallback"),
            r.used_llm ? "ok" : "warn",
          );
          testOut.textContent = r.text;
          testOut.hidden = false;
        } catch (e) {
          setNote(testMsg, describeError(e), "error");
        } finally {
          testBtn.disabled = false;
        }
      });
      body.appendChild(field("", testBtn, testMsg, testOut));
    };

    providerSel.addEventListener("change", () => {
      const next = providerSel.value;
      sum.endpoint = endpointOnProviderChange(sum.provider, sum.endpoint, next);
      sum.provider = next;
      commit();
      buildBody();
    });
    buildBody();
    return block;
  }

  // ── Destinations ──────────────────────────────────────────────────────
  function buildDestinations(): HTMLElement {
    const block = el("div", "act-block");
    block.appendChild(el("h4", "", t("settings.activity.destHeading")));
    block.appendChild(note(t("settings.activity.destHint")));

    const dests: WebhookDestination[] = manager.webhookDestinations.map((d) => ({
      ...d,
      events: [...d.events],
      attach_types: [...d.attach_types],
    }));
    const commit = (): void =>
      manager.setWebhookDestinations(
        dests.map((d) => ({ ...d, events: [...d.events], attach_types: [...d.attach_types] })),
      );
    // Secrets, tests and previews act on the saved config: save first.
    const saved = async (): Promise<void> => {
      commit();
      await manager.flushPendingSave();
    };

    const cards = el("div");
    block.appendChild(cards);

    const rebuild = (): void => {
      statusEls.clear();
      cards.replaceChildren();
      if (dests.length === 0) cards.appendChild(note(t("settings.activity.destEmpty")));
      for (const d of dests) cards.appendChild(renderCard(d));
      void poll();
    };

    const renderCard = (d: WebhookDestination): HTMLElement => {
      const card = el("div", "act-card");
      const repaint = (): void => card.replaceWith(renderCard(d));

      // Head: enabled, name, delete.
      const head = el("div", "act-card__head");
      const enabled = el("input");
      enabled.type = "checkbox";
      enabled.checked = d.enabled;
      enabled.title = t("settings.activity.enabled");
      enabled.setAttribute("aria-label", t("settings.activity.enabled"));
      enabled.addEventListener("change", () => {
        d.enabled = enabled.checked;
        commit();
      });
      const name = textInput(d.name, t("settings.activity.destDefaultName"));
      name.setAttribute("aria-label", t("settings.activity.name"));
      name.addEventListener("change", () => {
        d.name = name.value.trim();
        commit();
      });
      const del = button(t("settings.activity.destDelete"));
      del.addEventListener("click", async () => {
        if (!(await askConfirm(t("settings.activity.destDeleteConfirm"), t("settings.activity.destDelete")))) {
          return;
        }
        dests.splice(dests.indexOf(d), 1);
        commit();
        rebuild();
      });
      head.append(enabled, name, del);
      card.appendChild(head);

      // Format.
      const formatSel = select(
        WEBHOOK_FORMATS.map((f) => ({ value: f, label: t(`settings.activity.format.${f}`) })),
        d.format,
      );
      formatSel.addEventListener("change", () => {
        d.format = formatSel.value;
        commit();
        repaint();
      });
      card.appendChild(field(t("settings.activity.format"), formatSel));

      // URL (write-only).
      card.appendChild(urlField(d));

      // HMAC secret (generic JSON only).
      if (d.format === "json") card.appendChild(secretField(d));

      // Events.
      const eventsBox = el("div", "act-checks");
      for (const ev of WEBHOOK_EVENTS) {
        const c = checkbox(d.events.includes(ev), t(`settings.activity.event.${ev}`));
        c.box.addEventListener("change", () => {
          d.events = d.events.filter((x) => x !== ev);
          if (c.box.checked) d.events.push(ev);
          commit();
        });
        eventsBox.appendChild(c.wrap);
      }
      card.appendChild(field(t("settings.activity.events"), eventsBox));

      // Detail + prompts.
      const detailSel = select(
        [
          { value: "summary", label: t("settings.activity.detailSummary") },
          { value: "full", label: t("settings.activity.detailFull") },
        ],
        d.detail,
      );
      detailSel.addEventListener("change", () => {
        d.detail = detailSel.value;
        if (d.detail !== "full") d.include_prompts = false;
        commit();
        repaint();
      });
      card.appendChild(field(t("settings.activity.detail"), detailSel));
      if (d.detail === "full") {
        const ip = checkbox(d.include_prompts, t("settings.activity.includePrompts"));
        ip.box.addEventListener("change", () => {
          d.include_prompts = ip.box.checked;
          commit();
          repaint();
        });
        card.appendChild(
          field(
            "",
            ip.wrap,
            ...(d.include_prompts ? [note(t("settings.activity.includePromptsWarn"), "warn")] : []),
          ),
        );
      }

      const debounce = numberInput(d.debounce_secs, 0, 3600);
      debounce.addEventListener("change", () => {
        d.debounce_secs = clampInt(debounce.value, 0, 3600, d.debounce_secs);
        debounce.value = String(d.debounce_secs);
        commit();
      });
      card.appendChild(field(t("settings.activity.debounce"), debounce));

      // Attachments.
      const attach = checkbox(d.attach, t("settings.activity.attach"));
      attach.box.addEventListener("change", () => {
        d.attach = attach.box.checked;
        commit();
        repaint();
      });
      card.appendChild(field(t("settings.activity.attachLabel"), attach.wrap));
      if (d.attach) {
        const groups = el("div", "act-checks");
        for (const g of ATTACH_GROUPS) {
          const c = checkbox(groupEnabled(d.attach_types, g), t(`settings.activity.group.${g.id}`));
          c.box.addEventListener("change", () => {
            d.attach_types = setGroup(d.attach_types, g, c.box.checked);
            commit();
          });
          groups.appendChild(c.wrap);
        }
        card.appendChild(field(t("settings.activity.attachTypes"), groups));
        const caps = (
          label: string,
          key: "max_file_mb" | "max_total_mb" | "max_files",
          max: number,
        ): void => {
          const n = numberInput(d[key], 1, max);
          n.addEventListener("change", () => {
            d[key] = clampInt(n.value, 1, max, d[key]);
            n.value = String(d[key]);
            commit();
          });
          card.appendChild(field(label, n));
        };
        caps(t("settings.activity.maxFileMb"), "max_file_mb", ATTACH_LIMITS.maxFileMb);
        caps(t("settings.activity.maxTotalMb"), "max_total_mb", ATTACH_LIMITS.maxTotalMb);
        caps(t("settings.activity.maxFiles"), "max_files", ATTACH_LIMITS.maxFiles);
      }
      card.appendChild(note(t(`settings.activity.formatNote.${d.format}`)));

      // Test / preview / status.
      const actions = el("div", "act-actions");
      const testBtn = button(t("settings.activity.sendTest"));
      const previewBtn = button(t("settings.activity.preview"));
      const msg = note("");
      testBtn.addEventListener("click", async () => {
        testBtn.disabled = true;
        setNote(msg, t("settings.activity.testing"));
        try {
          await saved();
          const r = await webhookApi.test(d.id);
          if (r.ok) {
            const code = r.status !== null ? ` (HTTP ${r.status})` : "";
            setNote(msg, `${t("settings.activity.testDelivered")}${code}`, "ok");
          } else {
            const why = r.error ?? (r.status !== null ? `HTTP ${r.status}` : "");
            setNote(msg, `${t("settings.activity.testFailed")} ${why}`.trim(), "error");
          }
        } catch (e) {
          setNote(msg, describeError(e), "error");
        } finally {
          testBtn.disabled = false;
        }
      });
      previewBtn.addEventListener("click", async () => {
        previewBtn.disabled = true;
        setNote(msg, "");
        try {
          await saved();
          const p = await webhookApi.preview(d.id);
          if (!p) {
            setNote(msg, t("settings.activity.previewNone"), "warn");
          } else {
            await showPreview(p);
          }
        } catch (e) {
          setNote(msg, describeError(e), "error");
        } finally {
          previewBtn.disabled = false;
        }
      });
      actions.append(testBtn, previewBtn);
      card.append(actions, msg);

      const status = el("div", "act-status");
      statusEls.set(d.id, status);
      paintStatus(status, undefined);
      card.appendChild(status);
      return card;
    };

    /// Write-only URL field. Shows only the backend's scheme+host hint.
    const urlField = (d: WebhookDestination): HTMLElement => {
      const input = secretInput(t("settings.activity.urlPlaceholder"));
      const save = button(t("settings.activity.save"));
      const clear = button(t("settings.activity.remove"));
      const state = note("");
      const insecure = note(t("settings.activity.urlInsecure"), "warn");
      insecure.hidden = true;

      const paint = (isSet: boolean, isInsecure: boolean): void => {
        setNote(
          state,
          isSet
            ? `${d.url_hint ? `${d.url_hint} ` : ""}${t("settings.activity.isSet")}`
            : t("settings.activity.notSet"),
          isSet ? "ok" : "",
        );
        insecure.hidden = !(isSet && isInsecure);
      };
      const refreshStatus = async (): Promise<void> => {
        try {
          const s = await webhookApi.secretStatus(d.id);
          if (!disposed) paint(s.url, s.insecure);
        } catch {
          // Not saved yet (a brand-new destination): nothing is set.
          if (!disposed) paint(false, false);
        }
      };
      const apply = async (value: string): Promise<void> => {
        save.disabled = true;
        clear.disabled = true;
        try {
          await saved();
          d.url_hint = await webhookApi.setSecret(d.id, "url", value);
          input.value = "";
          commit();
          await refreshStatus();
        } catch (e) {
          setNote(state, describeError(e), "error");
        } finally {
          save.disabled = false;
          clear.disabled = false;
        }
      };
      save.addEventListener("click", () => {
        const v = input.value.trim();
        if (v !== "") void apply(v);
      });
      clear.addEventListener("click", () => void apply(""));
      paint(d.url_hint !== "", false);
      void refreshStatus();

      const row = el("div", "act-inline");
      row.append(input, save, clear);
      return field(t("settings.activity.url"), row, state, insecure);
    };

    /// Write-only HMAC secret (generic JSON destinations).
    const secretField = (d: WebhookDestination): HTMLElement => {
      const input = secretInput(t("settings.activity.secretPlaceholder"));
      const save = button(t("settings.activity.save"));
      const clear = button(t("settings.activity.remove"));
      const state = note("");
      const paint = (isSet: boolean): void =>
        setNote(state, isSet ? t("settings.activity.isSet") : t("settings.activity.notSet"), isSet ? "ok" : "");
      const refreshStatus = async (): Promise<void> => {
        try {
          const s = await webhookApi.secretStatus(d.id);
          if (!disposed) paint(s.secret);
        } catch {
          if (!disposed) paint(false);
        }
      };
      const apply = async (value: string): Promise<void> => {
        save.disabled = true;
        clear.disabled = true;
        try {
          await saved();
          await webhookApi.setSecret(d.id, "secret", value);
          input.value = "";
          await refreshStatus();
        } catch (e) {
          setNote(state, describeError(e), "error");
        } finally {
          save.disabled = false;
          clear.disabled = false;
        }
      };
      save.addEventListener("click", () => {
        if (input.value !== "") void apply(input.value);
      });
      clear.addEventListener("click", () => void apply(""));
      paint(false);
      void refreshStatus();

      const row = el("div", "act-inline");
      row.append(input, save, clear);
      return field(t("settings.activity.secret"), row, state, note(t("settings.activity.secretHint")));
    };

    const add = button(t("settings.activity.destAdd"), true);
    add.addEventListener("click", () => {
      dests.push(newDestination(`${t("settings.activity.destDefaultName")} ${dests.length + 1}`));
      commit();
      rebuild();
    });
    block.appendChild(add);
    rebuild();
    return block;
  }
}

function paintStatus(box: HTMLElement, s: WebhookDestStatus | undefined): void {
  box.replaceChildren();
  const part = (label: string, value: string, tone: Tone = ""): void => {
    const span = el("span", `act-status__item${tone ? ` act-status__item--${tone}` : ""}`);
    span.append(el("b", "", `${label}: `), document.createTextNode(value));
    box.appendChild(span);
  };
  part(t("settings.activity.statusQueued"), String(s?.queued ?? 0));
  part(t("settings.activity.statusLastOk"), when(s?.last_ok_at ?? null));
  part(t("settings.activity.statusDropped"), String(s?.dropped ?? 0), (s?.dropped ?? 0) > 0 ? "warn" : "");
  if (s?.last_error) part(t("settings.activity.statusLastError"), s.last_error, "error");
}

/// Pretty JSON body plus what would be attached / only listed.
function showPreview(p: {
  format: string;
  payload: unknown;
  attachments: { name: string; size: number; mime: string }[];
  listed: { name: string; size: number; reason: string }[];
}): Promise<string | null> {
  return openDialog((card, done) => {
    card.classList.add("act-preview");
    card.appendChild(el("div", "dialog__message", t("settings.activity.previewTitle")));
    const pre = el("pre", "act-pre act-pre--tall", JSON.stringify(p.payload, null, 2));
    card.appendChild(pre);

    const section = (title: string, rows: string[], empty: string): void => {
      card.appendChild(el("div", "act-subhead", title));
      if (rows.length === 0) card.appendChild(note(empty));
      const ul = el("ul", "act-files");
      for (const r of rows) ul.appendChild(el("li", "", r));
      card.appendChild(ul);
    };
    section(
      t("settings.activity.previewAttachments"),
      p.attachments.map((a) => `${a.name} (${formatBytes(a.size)}, ${a.mime})`),
      t("settings.activity.previewNoAttach"),
    );
    if (p.listed.length > 0) {
      section(
        t("settings.activity.previewListed"),
        p.listed.map((a) => `${a.name} (${formatBytes(a.size)}) - ${a.reason}`),
        "",
      );
    }

    const row = el("div", "dialog__buttons");
    const close = el("button", "dialog__btn dialog__btn--primary", t("settings.activity.close"));
    close.addEventListener("click", () => done(null));
    row.appendChild(close);
    card.appendChild(row);
    return close;
  });
}
