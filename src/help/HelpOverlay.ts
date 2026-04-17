// Help overlay: a "?" button that opens a keyboard-shortcut popup.
// Multilingual — the UI language is derived from `navigator.language` on
// first mount and can be toggled inside the popup itself.

type Lang = "en" | "ko" | "ja";

interface ShortcutEntry {
  keys: string;
  en: string;
  ko: string;
  ja: string;
}

const SHORTCUTS: ShortcutEntry[] = [
  {
    keys: "Ctrl+Alt+1 … 9",
    en: "Switch to workspace 1 – 9",
    ko: "워크스페이스 1 – 9로 전환",
    ja: "ワークスペース 1 – 9 に切り替え",
  },
  {
    keys: "Ctrl+Shift+H",
    en: "Split pane horizontally",
    ko: "현재 창을 가로로 분할",
    ja: "ペインを水平に分割",
  },
  {
    keys: "Ctrl+Shift+V",
    en: "Split pane vertically",
    ko: "현재 창을 세로로 분할",
    ja: "ペインを垂直に分割",
  },
  {
    keys: "Ctrl+Shift+W",
    en: "Close focused pane",
    ko: "포커스된 창 닫기",
    ja: "フォーカスしているペインを閉じる",
  },
  {
    keys: "Ctrl+Tab",
    en: "Focus next pane",
    ko: "다음 창으로 포커스 이동",
    ja: "次のペインにフォーカス",
  },
  {
    keys: "Ctrl+Shift+Tab",
    en: "Focus previous pane",
    ko: "이전 창으로 포커스 이동",
    ja: "前のペインにフォーカス",
  },
  {
    keys: "Ctrl+Click (URL)",
    en: "Open link in default browser",
    ko: "기본 브라우저로 링크 열기",
    ja: "リンクをデフォルトブラウザで開く",
  },
  {
    keys: "Ctrl+Shift+Z",
    en: "Zoom / unzoom focused pane",
    ko: "포커스된 창 확대 / 원복",
    ja: "フォーカスしているペインを拡大 / 戻す",
  },
  {
    keys: "Ctrl+F",
    en: "Search terminal scrollback",
    ko: "터미널 스크롤백 검색",
    ja: "ターミナルのスクロールバックを検索",
  },
  {
    keys: "Ctrl+Shift+R",
    en: "Rename focused pane",
    ko: "포커스된 창 이름 변경",
    ja: "フォーカスしているペインの名前を変更",
  },
];

const LABELS: Record<
  Lang,
  { title: string; close: string; langLabel: string }
> = {
  en: { title: "Keyboard Shortcuts", close: "Close", langLabel: "Language" },
  ko: { title: "키보드 단축키", close: "닫기", langLabel: "언어" },
  ja: { title: "キーボードショートカット", close: "閉じる", langLabel: "言語" },
};

/// Detect the preferred UI language from the browser locale, defaulting to
/// English when the locale is unrecognised.
function detectLang(): Lang {
  const nav = navigator.language?.toLowerCase() ?? "";
  if (nav.startsWith("ko")) return "ko";
  if (nav.startsWith("ja")) return "ja";
  return "en";
}

/// Mount the "?" button and the hidden overlay into `parent`. Returns a
/// cleanup function that removes both from the DOM.
export function mountHelpButton(parent: HTMLElement): () => void {
  let currentLang: Lang = detectLang();

  // --- "?" button ---
  const btn = document.createElement("button");
  btn.className = "workspace-bar__help";
  btn.textContent = "?";
  btn.title = "Keyboard shortcuts";
  btn.setAttribute("aria-label", "Show keyboard shortcuts");
  parent.appendChild(btn);

  // --- overlay backdrop ---
  const backdrop = document.createElement("div");
  backdrop.className = "help-backdrop";
  backdrop.setAttribute("aria-hidden", "true");
  document.body.appendChild(backdrop);

  // --- modal panel ---
  const modal = document.createElement("div");
  modal.className = "help-modal";
  modal.setAttribute("role", "dialog");
  modal.setAttribute("aria-modal", "true");
  modal.setAttribute("aria-labelledby", "help-modal-title");
  document.body.appendChild(modal);

  function render() {
    const lbl = LABELS[currentLang];
    modal.innerHTML = "";

    // Header row
    const header = document.createElement("div");
    header.className = "help-modal__header";

    const title = document.createElement("h2");
    title.id = "help-modal-title";
    title.className = "help-modal__title";
    title.textContent = lbl.title;
    header.appendChild(title);

    // Language selector
    const langWrap = document.createElement("div");
    langWrap.className = "help-modal__lang-wrap";

    const langLabel = document.createElement("label");
    langLabel.className = "help-modal__lang-label";
    langLabel.textContent = lbl.langLabel + ":";

    const sel = document.createElement("select");
    sel.className = "help-modal__lang-sel";
    const langs: Array<[Lang, string]> = [
      ["en", "English"],
      ["ko", "한국어"],
      ["ja", "日本語"],
    ];
    for (const [code, name] of langs) {
      const opt = document.createElement("option");
      opt.value = code;
      opt.textContent = name;
      if (code === currentLang) opt.selected = true;
      sel.appendChild(opt);
    }
    sel.addEventListener("change", () => {
      currentLang = sel.value as Lang;
      render();
    });

    langWrap.appendChild(langLabel);
    langWrap.appendChild(sel);
    header.appendChild(langWrap);
    modal.appendChild(header);

    // Shortcut table
    const table = document.createElement("table");
    table.className = "help-modal__table";

    for (const s of SHORTCUTS) {
      const tr = document.createElement("tr");

      const tdKeys = document.createElement("td");
      tdKeys.className = "help-modal__keys";
      // Render each key segment as a <kbd>
      const segments = s.keys.split("+");
      segments.forEach((seg, i) => {
        if (i > 0) {
          tdKeys.appendChild(document.createTextNode(" + "));
        }
        const kbd = document.createElement("kbd");
        kbd.textContent = seg.trim();
        tdKeys.appendChild(kbd);
      });

      const tdDesc = document.createElement("td");
      tdDesc.className = "help-modal__desc";
      tdDesc.textContent = s[currentLang];

      tr.appendChild(tdKeys);
      tr.appendChild(tdDesc);
      table.appendChild(tr);
    }

    modal.appendChild(table);

    // Close button
    const closeBtn = document.createElement("button");
    closeBtn.className = "help-modal__close";
    closeBtn.textContent = lbl.close;
    closeBtn.addEventListener("click", hide);
    modal.appendChild(closeBtn);
  }

  function show() {
    render();
    backdrop.classList.add("help-backdrop--visible");
    modal.classList.add("help-modal--visible");
    backdrop.setAttribute("aria-hidden", "false");
    // Trap focus inside the modal
    const firstFocusable = modal.querySelector<HTMLElement>(
      "button, select, [tabindex]",
    );
    firstFocusable?.focus();
  }

  function hide() {
    backdrop.classList.remove("help-backdrop--visible");
    modal.classList.remove("help-modal--visible");
    backdrop.setAttribute("aria-hidden", "true");
    btn.focus();
  }

  btn.addEventListener("click", show);

  // Close on backdrop click or Escape key
  backdrop.addEventListener("click", hide);
  document.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape" && modal.classList.contains("help-modal--visible")) {
      ev.preventDefault();
      hide();
    }
  });

  return () => {
    btn.remove();
    backdrop.remove();
    modal.remove();
  };
}
