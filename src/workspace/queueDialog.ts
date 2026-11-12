// The "Queue prompt…" dialog: a multi-line box (Enter = newline, Ctrl+Enter =
// add) above the pane's queued prompts, each with a remove button, plus
// "clear all". Stays open while prompts are added so several can be lined up.

import { open } from "../ui/Dialog";
import { t } from "../i18n/i18n";
import { fill } from "../git/worktreeFlow";
import { hasMod, shortcutLabel } from "../platform";

export interface QueueDialogHost {
  /// Pane name shown in the title.
  label: string;
  /// Whether an AI agent is running in the pane right now.
  hasAgent: () => boolean;
  list: () => readonly string[];
  /// Queue the text; false when refused (blank, queue full).
  add: (text: string) => boolean;
  remove: (index: number) => void;
  clear: () => void;
  /// Called whenever the queue changes (a delivery); returns an unsubscribe.
  subscribe: (cb: () => void) => () => void;
}

export function openQueueDialog(host: QueueDialogHost): Promise<void> {
  let unsub = (): void => {};
  const closed = open((card, done) => {
    card.classList.add("queue-dialog");

    const title = document.createElement("div");
    title.className = "dialog__message";
    title.textContent = fill(t("queue.title"), { pane: host.label });

    const hint = document.createElement("div");
    hint.className = "queue-dialog__hint";

    const input = document.createElement("textarea");
    input.className = "dialog__input queue-dialog__input";
    input.rows = 4;
    input.placeholder = fill(t("queue.placeholder"), { key: shortcutLabel("Ctrl+Enter") });

    const list = document.createElement("ol");
    list.className = "queue-dialog__list";

    const row = document.createElement("div");
    row.className = "dialog__buttons";

    const clearBtn = document.createElement("button");
    clearBtn.type = "button";
    clearBtn.className = "dialog__btn";
    clearBtn.textContent = t("queue.clearAll");
    clearBtn.addEventListener("click", () => {
      host.clear();
      render();
    });

    const closeBtn = document.createElement("button");
    closeBtn.type = "button";
    closeBtn.className = "dialog__btn";
    closeBtn.textContent = t("queue.close");
    closeBtn.addEventListener("click", () => done(null));

    const addBtn = document.createElement("button");
    addBtn.type = "button";
    addBtn.className = "dialog__btn dialog__btn--primary";
    addBtn.textContent = t("queue.add");
    addBtn.title = shortcutLabel("Ctrl+Enter");

    const add = (): void => {
      if (input.value.trim() === "") return;
      if (host.add(input.value)) {
        input.value = "";
        render();
      } else {
        hint.textContent = t("queue.full");
      }
      input.focus();
    };
    addBtn.addEventListener("click", add);

    input.addEventListener("keydown", (ev) => {
      // The Enter that commits an IME composition is not a command.
      if (ev.isComposing || ev.keyCode === 229) return;
      if (ev.key === "Enter" && hasMod(ev)) {
        ev.preventDefault();
        add();
      }
    });

    function render(): void {
      const items = host.list();
      hint.textContent = host.hasAgent() ? t("queue.hint") : t("queue.noAgent");
      list.replaceChildren();
      if (items.length === 0) {
        const empty = document.createElement("li");
        empty.className = "queue-dialog__empty";
        empty.textContent = t("queue.empty");
        list.appendChild(empty);
      }
      items.forEach((text, i) => {
        const li = document.createElement("li");
        li.className = "queue-dialog__item";
        const body = document.createElement("span");
        body.className = "queue-dialog__text";
        body.textContent = text;
        body.title = text;
        const rm = document.createElement("button");
        rm.type = "button";
        rm.className = "queue-dialog__remove";
        rm.textContent = "×";
        rm.title = t("queue.remove");
        rm.setAttribute("aria-label", t("queue.remove"));
        rm.addEventListener("click", () => {
          host.remove(i);
          render();
        });
        li.append(body, rm);
        list.appendChild(li);
      });
      clearBtn.disabled = items.length === 0;
    }

    row.append(clearBtn, closeBtn, addBtn);
    card.append(title, hint, input, list, row);
    unsub = host.subscribe(render);
    render();
    return input;
  });
  return closed.then(
    () => unsub(),
    () => unsub(),
  );
}
