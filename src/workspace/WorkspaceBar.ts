import type { DetectedAgent, ShellProfile } from "../types";
import type { WorkspaceManager } from "./WorkspaceManager";
import { api } from "../ipc/bridge";
import { mountSettings } from "../settings/SettingsOverlay";
import { isWorkspacePanelCollapsed, PANEL_TOGGLED_EVENT, toggleWorkspacePanel } from "./WorkspacePanel";
import { t, onLangChange } from "../i18n/i18n";
import { toggleSidePanel } from "../sidepanel/SidePanel";
import { showContextMenu, type ContextMenuEntry } from "../menu/ContextMenu";
import { shellFamilyFromExecutable } from "../terminal/shellQuote";
import { launchableAgents } from "./agentLaunch";
import { ACCOUNT_NONE, agentUsesAccounts } from "../accounts/accountModel";
import { promptNewAccount } from "../accounts/createAccount";
import { fill } from "../git/worktreeFlow";
import type { TabLaunch } from "./WorkspaceManager";

const panelToggleSvg = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="2"/><line x1="9" y1="4" x2="9" y2="20"/></svg>`;
const fileDockSvg = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="2"/><line x1="15" y1="4" x2="15" y2="20"/></svg>`;

const launcherSvg = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>`;

/// Installed agents, detected once per session (the launcher's "Rescan"
/// forces a fresh look). Started at mount so the first open isn't waiting.
let agentScan: Promise<DetectedAgent[]> | null = null;
/// The last detection failed (its `[]` means "unknown", not "none installed"),
/// so the launcher says so instead of claiming no agents are installed.
let agentScanFailed = false;
function scanAgents(force = false): Promise<DetectedAgent[]> {
  if (force || !agentScan) {
    const scan: Promise<DetectedAgent[]> = api.detectAgents(force).then(
      (agents) => {
        if (agentScan === scan) agentScanFailed = false;
        return agents;
      },
      (e) => {
        console.warn("detect_agents failed:", e);
        if (agentScan === scan) {
          agentScanFailed = true;
          agentScan = null;
        }
        return [];
      },
    );
    agentScan = scan;
  }
  return agentScan;
}

/// The "+" launcher's menu: installed agents (each a new tab in the default
/// shell typing `<exe> <bypass flag>`), shells, GUI panes, and Rescan.
function launcherEntries(
  manager: WorkspaceManager,
  shells: ShellProfile[],
  agents: DetectedAgent[],
  detectFailed: boolean,
  reopen: () => void,
  pickAccount: (agentId: string, agentName: string, launch: TabLaunch) => void,
): ContextMenuEntry[] {
  const shell = manager.defaultShell;
  const family = shellFamilyFromExecutable(shells.find((s) => s.name === shell)?.executable);
  const entries: ContextMenuEntry[] = [{ header: t("launcher.agents") }];
  const launchable = launchableAgents(agents, family);
  for (const { agent, command } of launchable) {
    const bypass = agent.bypass_args.length > 0;
    const tip = bypass
      ? [t("launcher.bypassTip"), agent.note ? t(agent.note) : "", command]
      : [command];
    entries.push({
      label: bypass ? `⚡ ${agent.name}` : `${agent.name} ${t("launcher.noBypass")}`,
      title: tip.filter((x) => x).join("\n"),
      onSelect: () =>
        agentUsesAccounts(agent.id)
          ? pickAccount(agent.id, agent.name, { shell, startupCmd: command })
          : void manager.newTabInFocused({ shell, startupCmd: command }),
    });
  }
  if (launchable.length === 0) {
    entries.push({
      label: detectFailed ? t("launcher.detectFailed") : t("launcher.noAgents"),
      disabled: true,
      onSelect: () => {},
    });
  }
  entries.push("separator", { header: t("launcher.terminal") });
  const ordered = [shell, ...shells.map((s) => s.name).filter((n) => n !== shell)];
  for (const name of ordered) {
    entries.push({
      label: name === shell ? `${name} ${t("launcher.defaultShell")}` : name,
      onSelect: () => void manager.newTabInFocused({ shell: name, startupCmd: null }),
    });
  }
  entries.push(
    "separator",
    { header: t("launcher.panes") },
    { label: t("files.title"), onSelect: () => void manager.splitFocusedFiles("horizontal") },
    { label: t("git.title"), onSelect: () => void manager.splitFocusedGit("horizontal") },
    {
      label: t("launcher.browser"),
      onSelect: () => void manager.splitFocusedBrowser("horizontal"),
    },
    "separator",
    {
      label: t("launcher.rescan"),
      onSelect: () => {
        void scanAgents(true);
        reopen();
      },
    },
  );
  return entries;
}

/// The second step for Claude Code / Codex: which login to start it under,
/// like a website's account chooser. "Default login" is the tool's own folder
/// (`~/.claude`, `~/.codex`); "Add account" asks for a name, creates the
/// account's folders and starts the agent there, where it asks to sign in.
/// An account without a folder for this agent (a hand-made profile with only
/// the other tool's folder) is left out: it would silently run under the
/// default login.
function accountEntries(
  manager: WorkspaceManager,
  agentId: string,
  agentName: string,
  launch: TabLaunch,
): ContextMenuEntry[] {
  const start = (account: string): void => void manager.newTabInFocused({ ...launch, account });
  const entries: ContextMenuEntry[] = [
    { header: fill(t("launcher.chooseAccount"), { agent: agentName }) },
    {
      label: t("launcher.defaultAccount"),
      title: t("launcher.defaultAccountTip"),
      onSelect: () => start(ACCOUNT_NONE),
    },
  ];
  for (const a of manager.accounts) {
    const dir = agentId === "codex" ? a.codex_home : a.claude_config_dir;
    if (dir.trim() === "") continue;
    entries.push({ label: `● ${a.name || a.id}`, onSelect: () => start(a.id) });
  }
  entries.push("separator", {
    label: t("launcher.addAccount"),
    title: t("launcher.addAccountTip"),
    onSelect: () => {
      void promptNewAccount(manager).then((profile) => {
        if (profile) start(profile.id);
      });
    },
  });
  return entries;
}

export function mountWorkspaceBar(
  host: HTMLElement,
  manager: WorkspaceManager,
  shells: ShellProfile[],
): () => void {
  const bar = document.createElement("div");
  bar.className = "workspace-bar";

  // Panel toggle — far left, single source of truth for show/hide.
  const toggleBtn = document.createElement("button");
  toggleBtn.className = "workspace-bar__icon-btn";
  toggleBtn.type = "button";
  toggleBtn.innerHTML = panelToggleSvg;
  toggleBtn.title = t("workspace.togglePanel");
  toggleBtn.setAttribute("aria-label", t("workspace.togglePanel"));
  toggleBtn.addEventListener("click", () => toggleWorkspacePanel(manager));
  // Unread count, only while the panel is collapsed (its own bell is hidden then).
  const toggleBadge = document.createElement("span");
  toggleBadge.className = "workspace-panel__bell-badge workspace-bar__badge workspace-panel__bell-badge--hidden";
  toggleBtn.appendChild(toggleBadge);
  const refreshToggleBadge = (): void => {
    const items = isWorkspacePanelCollapsed() ? manager.inboxItems() : [];
    toggleBadge.textContent = String(items.length);
    toggleBadge.classList.toggle("workspace-panel__bell-badge--hidden", items.length === 0);
    toggleBadge.classList.toggle(
      "workspace-panel__bell-badge--attention",
      items.some((i) => i.status === "attention"),
    );
  };
  const cleanupBadge = manager.onInboxChange(refreshToggleBadge);
  document.addEventListener(PANEL_TOGGLED_EVENT, refreshToggleBadge);
  bar.appendChild(toggleBtn);

  const spacer = document.createElement("div");
  spacer.className = "workspace-bar__spacer";
  bar.appendChild(spacer);

  // "+" launcher: a dropdown of agents, shells and panes.
  const launchBtn = document.createElement("button");
  launchBtn.className = "workspace-bar__icon-btn";
  launchBtn.type = "button";
  launchBtn.innerHTML = launcherSvg;
  launchBtn.title = t("launcher.button");
  launchBtn.setAttribute("aria-label", t("launcher.button"));
  launchBtn.setAttribute("aria-haspopup", "menu");
  const openLauncher = async (): Promise<void> => {
    const agents = await scanAgents();
    // Measured after the await: the bar may have moved while detecting.
    const r = launchBtn.getBoundingClientRect();
    const detectFailed = agentScanFailed;
    const pickAccount = (agentId: string, agentName: string, launch: TabLaunch): void => {
      // The launcher menu is already closed by its select; the chooser
      // opens in its place.
      showContextMenu(r.left, r.bottom + 2, accountEntries(manager, agentId, agentName, launch));
    };
    showContextMenu(
      r.left,
      r.bottom + 2,
      launcherEntries(manager, shells, agents, detectFailed, openLauncherSoon, pickAccount),
    );
  };
  const openLauncherSoon = (): void => void openLauncher();
  launchBtn.addEventListener("click", openLauncherSoon);
  bar.appendChild(launchBtn);
  void scanAgents();

  const shellPicker = document.createElement("select");
  shellPicker.className = "workspace-bar__shell";
  shellPicker.title = t("workspace.shellTitle");
  for (const s of shells) {
    const opt = document.createElement("option");
    opt.value = s.name;
    opt.textContent = s.name;
    shellPicker.appendChild(opt);
  }
  shellPicker.addEventListener("change", () => {
    manager.setDefaultShell(shellPicker.value);
  });
  // Reflect the persisted default, and follow along when Settings → General
  // changes it — the two controls edit the same value.
  if (shells.length > 0) shellPicker.value = manager.defaultShell;
  manager.onDefaultShellChange = (name) => {
    shellPicker.value = name;
  };
  bar.appendChild(shellPicker);

  const browserBtn = document.createElement("button");
  browserBtn.className = "workspace-bar__shell";
  browserBtn.type = "button";
  browserBtn.textContent = t("workspace.addBrowser");
  browserBtn.title = t("workspace.addBrowserTitle");
  browserBtn.style.cursor = "pointer";
  browserBtn.addEventListener("click", () => {
    void manager.splitFocusedBrowser("horizontal");
  });
  bar.appendChild(browserBtn);

  // Side panel toggle: the mirror image of the workspace-panel toggle.
  const dockBtn = document.createElement("button");
  dockBtn.className = "workspace-bar__icon-btn";
  dockBtn.type = "button";
  dockBtn.innerHTML = fileDockSvg;
  dockBtn.title = t("sidepanel.toggle");
  dockBtn.setAttribute("aria-label", t("sidepanel.toggle"));
  dockBtn.addEventListener("click", () => toggleSidePanel());
  bar.appendChild(dockBtn);

  const kofiBtn = document.createElement("button");
  kofiBtn.className = "workspace-bar__icon-btn";
  kofiBtn.type = "button";
  kofiBtn.innerHTML = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17 8h2a2 2 0 0 1 0 4h-2"/><path d="M3 8h14v9a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4V8z"/><line x1="6" y1="2" x2="6" y2="6"/><line x1="10" y1="2" x2="10" y2="6"/><line x1="14" y1="2" x2="14" y2="6"/></svg>`;
  kofiBtn.title = t("workspace.supportTitle");
  kofiBtn.addEventListener("click", () => {
    void api.openUrl("https://ko-fi.com/youngminkim").catch((e) =>
      console.warn("openUrl failed:", e),
    );
  });
  bar.appendChild(kofiBtn);

  const ghBtn = document.createElement("button");
  ghBtn.className = "workspace-bar__icon-btn";
  ghBtn.type = "button";
  ghBtn.innerHTML = `<svg width="15" height="15" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>`;
  ghBtn.title = "GitHub";
  ghBtn.addEventListener("click", () => {
    void api.openUrl("https://github.com/youngmins/ymux").catch((e) =>
      console.warn("openUrl failed:", e),
    );
  });
  bar.appendChild(ghBtn);

  const cleanupHelp = mountSettings(bar, manager);

  host.appendChild(bar);

  const cleanupLang = onLangChange(() => {
    toggleBtn.title = t("workspace.togglePanel");
    toggleBtn.setAttribute("aria-label", t("workspace.togglePanel"));
    launchBtn.title = t("launcher.button");
    launchBtn.setAttribute("aria-label", t("launcher.button"));
    shellPicker.title = t("workspace.shellTitle");
    browserBtn.textContent = t("workspace.addBrowser");
    browserBtn.title = t("workspace.addBrowserTitle");
    kofiBtn.title = t("workspace.supportTitle");
    dockBtn.title = t("sidepanel.toggle");
    dockBtn.setAttribute("aria-label", t("sidepanel.toggle"));
  });

  return () => {
    cleanupLang();
    cleanupBadge();
    document.removeEventListener(PANEL_TOGGLED_EVENT, refreshToggleBadge);
    cleanupHelp();
    bar.remove();
  };
}
