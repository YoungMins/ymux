<h1 align="center">ymux</h1>

<p align="center">
  <strong>English</strong> &nbsp;·&nbsp; <a href="./README.ko.md">한국어</a> &nbsp;·&nbsp; <a href="./README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.14.0-7fdbca?style=flat-square" alt="version 0.14.0" />
</p>

<p align="center">
  <a href="https://apps.microsoft.com/detail/9N20Z9NKD2Q1?mode=direct">
    <img src="https://get.microsoft.com/images/en-us%20dark.svg" width="200" alt="Get it from Microsoft" />
  </a>
</p>

<p align="center">
  <a href="https://ko-fi.com/youngminkim">
    <img src="https://ko-fi.com/img/githubbutton_sm.svg" alt="Support on Ko-fi" />
  </a>
</p>

---

A lightweight, tmux-inspired terminal multiplexer for Windows and macOS.

https://github.com/user-attachments/assets/705fff59-0bda-4460-a87f-d7ba6f50993a

ymux splits one window into as many terminals as you need, saves the layout per
workspace, and keeps shells, scrollback and AI coding agents organized. It is
built with Tauri 2 (Rust) and xterm.js, on WebView2 (Windows) and WKWebView
(macOS).

## Install

### Windows

**Microsoft Store**:
<https://apps.microsoft.com/detail/9N20Z9NKD2Q1>. The Store build uses the
system Evergreen WebView2 runtime (built into Windows 11, delivered by Windows
Update on Windows 10) and registers a `ymux` app execution alias, so `ymux`
runs from any terminal.

**MSI**: download `ymux_*_x64_en-US.msi` from
[Releases](https://github.com/YoungMins/ymux/releases) and run it. The installer
bundles a WebView2 bootstrapper and adds the install directory to `PATH`, so
`ymux` can be launched from any terminal.

### macOS (Apple Silicon, macOS 11+)

Download `ymux_*_macos_aarch64.dmg` from
[Releases](https://github.com/YoungMins/ymux/releases), open it, and drag ymux
to Applications.

The app is signed ad-hoc but **not notarized**, so Gatekeeper blocks it on first
launch with a "damaged" or "unidentified developer" message. Clear the download
quarantine flag once:

```sh
xattr -dr com.apple.quarantine /Applications/ymux.app
```

## Features

### Terminals and layouts

- **Layouts that persist**: recursive horizontal / vertical splits. Each pane
  remembers its shell, `cwd`, and an optional startup command.
- **Pane tabs**: `Ctrl+Shift+T` adds a tab to the focused pane; hidden tabs keep
  running.
- **Live cwd inheritance**: splitting a pane opens the new pane in the directory
  the parent shell is in now, not its startup directory (OSC 7 tracking).
- **Shell auto-detection**: on Windows, `cmd.exe`, Windows PowerShell,
  PowerShell 7 (`pwsh`), Git Bash, WSL distros, and the Developer Command
  Prompt / Developer PowerShell of every Visual Studio install `vswhere` finds.
  On macOS, your login shell
  plus any zsh / bash / fish it finds (Homebrew included). zsh and bash start
  through a generated shim that sources your own dotfiles first and then adds
  the OSC 7 hook.
- **Persistent scrollback**: each pane's buffer (colors included) is restored
  beneath a dimmed *"session restored"* separator on the next launch. Toggle
  under **Settings → General**.
- **Pane zoom, rename, search**: `Ctrl+Shift+Z` hides every other pane,
  `Ctrl+Shift+R` renames the focused pane, `Ctrl+F` opens a find bar on the
  terminal (Enter / Shift+Enter step through matches).
- **Bottom-anchored prompt**: when output is shorter than the pane, the prompt
  sits at the bottom edge and output grows upward. On by default; toggle under
  **Settings → General**.
- **Clickable links and paths**: `Ctrl+Click` opens `http://` / `https://` links
  in your default browser, and filesystem paths a command printed with the OS
  default program. Executables and scripts are revealed in the file manager
  instead of run.
- **Paste text and images**: `Ctrl+V` pastes text. If the clipboard holds an
  image (for example a `Win+Shift+S` screenshot), it is saved to a self-cleaning
  temp file and its quoted path is typed instead. Pruned after 24 h
  (`paste_image_retention_hours`).
- **Drag files in**: drop files or folders onto a terminal and their quoted paths
  are typed at the cursor. Nothing runs until you press Enter.
- **Per-pane settings (⚙)**: set a custom background color and HotKey buttons
  (single- or multi-line commands on labelled buttons above the terminal).
- **Close to tray**: closing the window hides ymux to the tray (Windows
  notification area / macOS menu bar) and keeps every shell and agent running.
  Click the tray icon, or launch ymux again, to bring it back (one ymux runs at
  a time). Quit from the tray menu, `Cmd+Q` on macOS, or **Quit ymux** in the
  command palette (unsaved editors are still asked about).

### Workspaces

- **Numbered workspaces**: `Ctrl+Alt+1` .. `Ctrl+Alt+9` switch between the first
  nine. Add more with the `+` button in the workspace panel and remove one with
  the `×` on its row (the last one can't be deleted). Each workspace saves its
  own layout, and panes stay alive across switches so REPLs and tails don't die.
- **Workspace panel**: a collapsible, scrollable list on the left. Double-click a
  name to rename; drag a row to reorder. The active workspace is outlined.
- **Per-workspace notes**: a notes button beside each workspace number, plus
  `Ctrl+Alt+N` for the active workspace. Stored in `localStorage`.
- **Git worktree panes**: the command palette's **"Open pane in new git
  worktree"** asks for a branch name, creates a worktree (default: a sibling
  `.ymux-worktrees/<branch>` directory, override with `worktree_base_dir`) and
  opens a terminal in it. Closing the pane, or deleting the workspace, offers to
  remove the worktree again; branches and commits are never touched.

### AI agents

- **Agent status at a glance**: every terminal pane shows idle / running / done /
  needs-attention, from output activity and the bell / OSC 9 completion signal,
  as a colored pane border plus a tint on its workspace row. When a CLI
  finishes out of sight you get an OS notification and a short beep (toggle in
  Settings).
- **Launcher**: the top **+** menu lists installed agent CLIs, your shells and
  the GUI panes. Choose **Rescan** to refresh the list.
- **Agent tree**: the workspace panel lists every pane and tab with the agents
  running in it: Claude Code sessions and their subagents (via hooks), plus
  Codex, Gemini, aider and other CLIs (via a lightweight process scan). Click a
  row to jump to that pane.
- **Session resume**: restart ymux while a pane runs Claude Code or Codex and
  the pane resumes the same conversation: `claude --resume <id>
  --dangerously-skip-permissions` (always added, so you aren't re-approving what
  you already approved) or `codex resume <id>`. Eligible for 24 h after the
  session was last active. Works without agent tracking, by reading the CLI's
  own transcripts from disk. Shell-only panes keep restoring scrollback.
- **Claude Code hook tracking** (off by default, **Settings → General**): adds
  user-level HTTP hooks to `~/.claude/settings.json` and to the Claude folder of
  every account (see Multiple accounts below; a folder that does not exist yet
  is skipped until a terminal starts in it). While it is on, *every* Claude Code
  session on the machine that uses one of those folders, including ones
  outside ymux, sends its hook events (prompts, tool input and output) to
  ymux's port on `127.0.0.1`. ymux ignores events that don't come from its own
  panes; turning tracking off removes the hooks from all of those folders. If
  ymux is not running while tracking is on, Claude Code reports those hooks as
  failed (for example "Stop hook error occurred"), so turn tracking off before
  you uninstall ymux, delete an account, or downgrade to a version that does
  not know about accounts (it would leave the hooks in the account folders).
  ymux ships no helper binary.

#### Auto-continue (Claude Code)

Turn it on per terminal in the pane's ⚙ dialog or with the command palette
("Toggle auto-continue for this pane"); an **AUTO** badge appears in the
pane title, and hovering it lists the last answers. While it is on, ymux
answers Claude Code's multiple-choice questions (`AskUserQuestion`) itself: it
picks the option whose label ends with "(Recommended)" (all such options for a
multi-select question). If any question has no recommended option, ymux
refuses the question and tells Claude to choose for itself. It needs Claude Code
hook tracking (Settings → General), works for Claude Code only, and is
runtime-only: it is off again after a restart or when the terminal closes.
Risk: the recommended option is only Claude's suggestion and is not always the
right one, so use it where an unattended wrong answer is cheap to undo.

#### Office view

Press the office button in the top bar (or `Ctrl+Shift+O`) to see the current
workspace as a pixel-art office. Each pane, or tab group, is a room laid out
the way you split it, with corridors between the rooms, and each tab is a
desk. Agents are slimes in their CLI's colour (Claude Code orange, Codex dark
grey, Gemini blue): they jiggle at their desk while working (with the tool in
a speech bubble), stand tall when they need you, sip a coffee when done and
melt into a nap when idle. A subagent hops in as a little slime with a sprout,
works in the front row, reports to its lead when it finishes and leaves.

The picker at the top dresses the same floor plan as an office, a cave, a
castle, a ranch, a space station, the sea floor, a snow village or a desert
temple: desks become stone slabs, hay bales or consoles whose crystal, lantern
or screen glows with the pane's status. The choice is remembered.

**Heroes.** Every account is an RPG hero: its all-time token total in the
Usage view (Claude Code and Codex together) is its experience, one level per
100M tokens. *⚔ Heroes* in the office header opens the adventurers' guild with
each account's level, XP bar, class (warrior, mage, rogue, cleric or cat) and
look: a slime or a human adventurer, whose hair takes the agent's colour.
Agents on the floor wear their account's class gear and a level badge, and the
gear grows with the level in ten steps: plain clothes at Lv 0, starter gear at
Lv 1, the full outfit at Lv 3, a jewel at Lv 5, the class item (sword, staff,
dagger, holy staff, fish) at Lv 10, a cape (a sash on slimes) at Lv 20, gold at
Lv 30, wings at Lv 50, a glowing aura with sparkles at Lv 75 and, at Lv 100, a
legendary crown with a rainbow glow and a star in orbit. A level-up is announced
with a banner while the office is open.

- **Finished work** — the agent does a victory hop with confetti, and a flag
  goes up outside its room until you open that pane.
- **Mail** — work ymux hands from one place to another (a fan-out prompt, diff
  review comments, a commit request) is carried over through the corridors
  and left in the room's mailbox: a sealed letter until the agent starts on
  it, an opened one while it works, gone when that turn ends. Hover a mailbox
  for what is in it.
- **Board** — the cork board in the lobby pins a note for every finished turn
  and hand-over; click it for the list, and click an entry to jump there.

Click a room, desk or slime to peek at its terminal in place: a window over
the room shows the live screen, with key buttons (arrows, Enter, Esc, Tab,
1–3) for answering a menu and a box for typing a prompt (Enter sends,
Shift+Enter adds a line). Double-click a room, or use *Open terminal*, to jump
to the pane; a flag, mailbox or board note jumps there directly. `Esc` closes
the peek, then the office; the terminals keep running underneath.
The flags, mail and board live for the session only.

#### Multiple accounts

Keep several Claude Code or Codex logins side by side, the way a website lets
you sign in to more than one account. Press **+** in the top bar and pick
Claude Code or Codex: ymux asks which account to start it under — the default
login (`~/.claude`, `~/.codex`), one of your accounts, or **+ Add account…**.
A new account only needs a name; ymux gives it its own login folders under its
config directory, and the agent asks you to sign in the first time it runs
there. **Settings → Accounts** lists the accounts for renaming, recoloring and
deleting (an existing login folder can still be set by hand under
"advanced"). Deleting an account also deletes the login folder ymux made for
it, so re-adding it means signing in again; folders you set by hand are never
deleted. You can also pick the account per terminal in the pane's ⚙
dialog, or give a whole workspace a default by right-clicking its row in the
workspace panel. A terminal with an account shows a colored badge in its title.
ymux sets the variables when a terminal starts, so a change applies to
terminals started afterwards (a running shell keeps its account, and its badge
is dimmed until you restart it). Token usage reads every configured account's
folders, and session resume looks in the folder of the account the terminal
ran under.

#### Review changes

Read an agent's work as a diff and send line comments straight back to it.
Open it from the command palette (**"Review changes"**), the Git pane's
**Review changes** button or a worktree row, or **Review** in the fan-out
compare view. It is a full-window overlay for one repository or worktree (Esc
or ✕ closes it; it is kept alive while hidden, and nothing about it is saved to
your layout).

- **Files and hunks**: the changed files are listed on the left, the selected
  file's hunks on the right, in **unified** or **split** view. Untracked files
  show as additions, binary files are marked, and very large diffs are
  truncated (3000 files; 512 KiB of patch per file).
- **Compare with**: `HEAD` (everything uncommitted, the default) or a branch,
  which diffs against the merge-base. **Ignore CR** hides CRLF-only noise. The
  view refreshes when the window regains focus and when the agent finishes.
- **Comments**: click a line to comment, Shift+click to extend a range. Comments
  live only in the overlay (closing it discards them); if the code moves under
  a comment, it is marked "moved or gone" instead of pointing at the wrong line.
- **Send to agent**: pastes your comments (each with its file, line and a few
  lines of context) into the terminal of the agent running in that repository.
  If several agents match, you pick one. ymux presses Enter for you **only if
  "Press Enter after sending" is ticked**; it is off by default. It will not
  paste while the agent is waiting on a permission or trust prompt, nor into a
  terminal that is not accepting pasted text, and when there is no agent at all
  it copies the text to the clipboard instead. A prompt over 32 KiB asks first.

#### Attention inbox

The workspace bar has a bell with a badge counting panes that need attention
(an agent asked a question or needs approval) or have finished while you were
elsewhere. Click it for the list (attention items first), plus the last 20
you have already read; picking a row jumps to that pane. `Ctrl+Shift+U` jumps
straight to the next pane that needs attention. Focusing a pane marks it read.

#### Fan-out

Give one prompt to several agents, each in its own git worktree, and compare
what comes back. Run **"Fan out prompt to agents…"** from the command palette:

| Field | Meaning |
|-------|---------|
| Repository / Base branch | Where the worktrees branch from |
| Agents | 1 to 8 lanes; each is Claude Code or Codex, with an optional account |
| Branch prefix | Default `fanout`. Branches are named `<prefix>/<run>-<agent>-<n>` |
| Prompt | Up to 32 KiB, sent to every lane |
| Open in a new workspace | Put the lanes in a fresh workspace |
| Press Enter automatically | On by default. Off: the prompt is typed and you press Enter |

Each lane gets a new branch and worktree (the same location as worktree panes,
`.ymux-worktrees` unless `worktree_base_dir` is set) and a terminal running the
plain agent command. Fan-out adds **no** permission-bypass flags. Two lanes sit
side by side, up to four in a grid, more in tabs. The prompt is pasted once the
agent is ready, never put on the command line. If the agent shows a trust
dialog or any other question, that lane waits until you answer it in its pane.
After 90 seconds without a ready agent you get **Send now** or **Copy prompt**.
A lane that fails to start is reported and everything already created is kept.

**"Compare fan-out lanes…"** lists each lane's status, files changed (+/−) and
commits ahead, with **Review** (opens the diff against the fan-out base),
**Focus pane** and **Keep…**. Keep never merges: it shows the merge command,
types it in the main worktree for you to run, and offers to clean up the other
lanes. Cleanup removes the worktrees you confirm and deletes branches with
`git branch -d`, so an unmerged branch survives.

### Activity log & notifications

Optional and **off by default**: **Settings → Activity & notifications**. ymux
can keep a local log of what your agents did and send a short report to chat or
a webhook when a turn ends or an agent needs you. It costs no model tokens;
nothing leaves your PC unless you add a destination.

**What is recorded.** One JSON line per prompt, tool call, permission request
and finished turn. Claude Code events come from the hook tracking above (so it
must be on); Codex turns are read from the CLI's own session file, only for
terminals ymux already tracks. A tool record keeps the tool name, the file paths
it touched (clipped), a clipped command line and its exit code. A finished turn
adds duration, changed files with +/− counts from `git diff --stat`, and
whether a test run passed. **Not recorded**: file contents, tool output,
full transcripts, and, unless you tick **Record prompt text**,
your prompts and the assistant's reply text.

**Redaction.** Everything is redacted before it reaches disk, a summarizer or a
webhook: private-key blocks, vendor tokens (`sk-…`, `ghp_…`, `xox…`, AWS and
Google keys), JWTs, `Bearer` headers, `user:pass@` URLs, chat-webhook URLs and
`key=value` secrets. This is pattern matching, not a guarantee.

**Storage.** One file per day, `activity/YYYY-MM-DD.jsonl` in the ymux config
directory (see [Configuration](#configuration)). Logs are deleted after 14 days
(**Keep logs**), the total is capped at 200 MiB (`max_total_mb`) and one day at
20 MiB, dropping tool records first. **Open log folder** and **Clear log** are
in the same section, which also lists recent turns.

**Summaries.** Written when a turn ends, from the facts above, never from
transcripts:

- **Built-in rules** (default): no model, offline. Templates exist for English,
  Korean and Japanese (other languages get English).
- **Ollama**: use **Load models** to list what you have pulled and pick one. A
  small model is plenty. Install [Ollama](https://ollama.com), then for example:

  ```sh
  ollama pull gemma3:1b
  ```

  The default endpoint is `http://127.0.0.1:11434`.
- **OpenAI-compatible** (LM Studio and similar): endpoint, model and an API key
  that is stored encrypted and never shown again.

The endpoint must be on this computer: the host is checked after name
resolution, so a name that points elsewhere is refused. **Allow remote
endpoint** lifts that, after which turn text is sent to that server. Requests
time out after 20 seconds by default, and any failure falls back to the
built-in summary. **Test summary** shows what a model returns.

**Webhook destinations.** Add as many as you like (up to 20), each with its own
settings:

| Format | Notes |
|--------|-------|
| JSON (generic) | Signed (below); multipart when files are attached |
| Slack | Incoming-webhook blocks; attachments are listed by name only |
| Discord | Embeds; files uploaded within Discord's size limit, up to 10 |
| Microsoft Teams | Workflows Adaptive Card; attachments are listed by name only |

Per destination: enabled, name, URL, events (**Turn finished**, **Needs
attention**), detail (**Summary** or **Full**: adds pane, account, working
folder, files and test result), **Include prompts** (off), a batch window
(default 10 s; up to 10 events go in one message), and an optional HMAC secret.
**Send test** and **Preview payload** show what would go out. Failed deliveries
are queued on disk (200 messages or 5 MiB per destination) and retried with
back-off (5 s doubling to 30 min, 8 tries, honoring `Retry-After`); redirects
are never followed. **A webhook URL is a credential**: anyone who has it can
post to that channel, so ymux stores it (and the secret and API key) encrypted
in `secrets.json` (Windows DPAPI; a `0600` file on macOS), never in
`config.toml`, and never shows it again. An `http://` URL is flagged because
the report would cross the network unencrypted.

Verifying the JSON format. Each request carries `X-Ymux-Timestamp` (Unix
seconds), `X-Ymux-Delivery` (a unique id) and, when a secret is set,
`X-Ymux-Signature: sha256=<hex>` with `hex(HMAC-SHA256(secret, timestamp + "." +
body))`. For a multipart request the signed body is the first part, `payload`.

```python
import hmac, hashlib

def verify(secret: str, timestamp: str, body: bytes, signature: str) -> bool:
    mac = hmac.new(secret.encode(), timestamp.encode() + b"." + body, hashlib.sha256)
    return hmac.compare_digest("sha256=" + mac.hexdigest(), signature)
```

Also reject timestamps far from now and repeated delivery ids.

**Attachments.** Per destination, off by default: send the files a turn
produced with its report. Candidates are files the turn created or changed
inside the terminal's repository or worktree, plus anything placed in the
report folder (`.ymux/report` by default, **Report folder**) during the turn.
Allowed types, which you choose (none by default): images (png, jpg, gif, webp),
PDF, audio (mp3, wav, ogg), video (mp4, webm), Office documents (docx, xlsx,
pptx) and text (md, txt, csv). Defaults: 8 MB per file, 8 MB in total, 4 files
(limits 25 MB and 10 files). A file is sent only if it is a regular file, not a
symlink, with a single hard link, whose contents match its extension (a text
file that contains a secret is refused), and whose name is not sensitive
(`.env*`, `*.pem`, `*.key`, `id_rsa*`, `*secret*`, `*credentials*`, and similar,
or anything under `.git`, `.ssh`, `.aws`). It is checked again, from the open
file, at the moment of sending. Anything that fails or exceeds a limit is listed
by name instead; the delivery itself never fails because of it.

**Downgrading.** Versions up to 0.13.9 do not know this feature: opening one
drops the webhook list from `config.toml` (the encrypted secrets stay in
`secrets.json`), so back up your destinations first.

### Side panel: Files and Usage

One panel on the right, shared by all workspaces. Icons at the top switch
between two views, and each keeps its state while hidden. The open state, view
and width persist; drag the left edge to resize.

- **Files**: toggle with `Ctrl+Shift+E`. Follows the active pane's working
  directory as you `cd`. Enter on a file opens it in a reused editor tab, and a
  preview of the selected file or folder sits beneath the list (`Tab` shows and
  hides it).
- **Usage**: switch to it with the panel's usage icon (the toolbar's side-panel
  button reopens the last view) or open it from the command palette. It shows:
  - local token counts for Claude Code and Codex, read from their session logs,
    for the last 5 hours, last 7 days or all recorded history, grouped by AI
    provider, model, project or account, with a total line for the selected
    period (**All recorded usage** shows the grand total);
  - remaining 5-hour and weekly quota with a reset countdown, shown as
    remaining or used (your choice). Quotas are fetched directly from Anthropic
    and OpenAI using the sign-in token those CLIs already stored on your PC
    (see [Privacy](#privacy)).

  Local totals refresh every 10 seconds while visible; account quotas are
  fetched at most once a minute (a failed fetch is retried sooner, and the ↻
  button at most every 30 seconds) and the rest of the time shown from the
  last result. Local totals exclude web chats
  and other devices and can include previously used accounts, so they are not
  the same as your subscription quota. When a quota can't be fetched, a recorded
  snapshot is shown and labelled stale.

### Built-in panes

Rendered by ymux itself, next to terminals. Each opens as a split of the
focused pane in its working directory:

- a terminal's right-click menu: Files, Editor, Git;
- the top bar's **+** launcher: Files, Git, Browser (the **+ Browser** button
  also adds a browser);
- the command palette: **Split: files pane**, **Split: editor pane**,
  **Split: git pane**.

The editor also opens when you press Enter on a file in the side panel.

| Pane | What it does |
|------|--------------|
| **Files** | Browse, preview, rename, create, multi-select, copy/move, overwrite prompts, delete to trash (with confirmation) |
| **Editor** | Syntax-highlighted text editor: save, find/replace, go to line, CRLF/BOM preserved, unsaved-changes guard, crash-safety drafts, external-change detection |
| **Git** | Commit graph, branch list and checkout, worktree add/remove (with confirmation), **Review changes** (diff review) |
| **Browser** | Iframe browser with URL bar and back / forward / reload; the URL persists across switches and restarts |

> **Note:** the browser pane is an HTML `<iframe>`, so sites that refuse
> embedding via `X-Frame-Options` or CSP `frame-ancestors` (for example
> github.com, google.com) won't load. It is meant for local dev servers,
> Storybook, internal dashboards and docs, not general browsing.

### Productivity

- **Command palette**: `Ctrl+Shift+P`, fuzzy-match any built-in action by name
  or keybinding. Some actions live only there: **Rename tab**, **Close other
  tabs**, **New workspace**, **Delete workspace**, rename workspace, and the
  editor / files / git pane splits.
- **System monitor**: a bottom status bar streams CPU / RAM / GPU / disk /
  network every 2 seconds. Values turn amber at 70% and red at 90%.
- **Update notifications**: a background check of GitHub releases every 6 hours
  shows a dismissable banner. Nothing is installed automatically.
- **Support on Ko-fi**: the ☕ Support button next to `⚙` opens
  [ko-fi.com/youngminkim](https://ko-fi.com/youngminkim).

### Customization

- **Settings (⚙)**: General, Accounts, Activity & notifications, Syntax Colors,
  Shortcuts and Config Files sections. Pick the language, manage accounts, set
  up the activity log and webhooks, edit the editor's syntax palette, browse
  the shortcut reference, or open `theme.toml` directly.
- **13 languages**: English, 한국어, 日本語, 中文, हिन्दी, Español, Français,
  العربية, Português, Русский, Türkçe, Deutsch, Tiếng Việt. Switch from the
  selector in the bottom-right status bar.
- **Hardened against embedded web content**: every backend command checks the
  caller's origin, so a page in a browser pane can't invoke ymux's IPC. ymux
  refuses to load if framed by another page, and the built-in browser forwards
  only a small fixed set of harmless shortcuts.

## Keyboard shortcuts

On macOS every `Ctrl` below becomes `Cmd`, which leaves `Ctrl` free for the
shell (`Ctrl+C`, `Ctrl+D`, `Ctrl+R`). Two exceptions are called out in the
table: pane cycling stays on `Ctrl+Tab` because macOS reserves `Cmd+Tab` for
the application switcher, and workspace switching drops the `Alt`.

| Shortcut (Windows)          | macOS              | Action                               |
|-----------------------------|--------------------|--------------------------------------|
| `Ctrl+Shift+H`              | `Cmd+Shift+H`      | Split pane horizontally              |
| `Ctrl+Shift+V`              | `Cmd+Shift+V`      | Split pane vertically                |
| `Ctrl+Shift+W`              | `Cmd+Shift+W`      | Close focused pane                   |
| `Ctrl+Shift+T`              | `Cmd+Shift+T`      | New tab in focused pane              |
| `Ctrl+Shift+[` / `]`        | `Cmd+Shift+[` / `]` | Previous / next tab in pane         |
| `Ctrl+Shift+Z`              | `Cmd+Shift+Z`      | Zoom / unzoom focused pane           |
| `Ctrl+Shift+U`              | `Cmd+Shift+U`      | Jump to next pane needing attention  |
| `Ctrl+Shift+O`              | `Cmd+Shift+O`      | Toggle office view                   |
| `Ctrl+Shift+←/→`            | `Cmd+Shift+←/→`    | Swap pane with previous / next       |
| `Ctrl+Shift+R`              | `Cmd+Shift+R`      | Rename focused pane                  |
| `Ctrl+Shift+P`              | `Cmd+Shift+P`      | Open command palette                 |
| `Ctrl+Alt+N`                | `Cmd+Opt+N`        | Toggle notes for active workspace    |
| `Ctrl+Shift+E`              | `Cmd+Shift+E`      | Toggle the side panel (Files view)   |
| `Ctrl+V`                    | `Cmd+V`            | Paste clipboard text (image → temp-file path) |
| `Ctrl+F`                    | `Cmd+F`            | Search terminal scrollback (find / replace in an editor pane) |
| `Ctrl+S`                    | `Cmd+S`            | Save the file (editor pane)          |
| `Ctrl+G`                    | `Cmd+G`            | Go to line (editor pane)             |
| `Ctrl++` / `Ctrl+-`         | `Cmd++` / `Cmd+-`  | Increase / decrease terminal font size |
| `Ctrl+0`                    | `Cmd+0`            | Reset terminal font size             |
| `Ctrl+Tab`                  | `Ctrl+Tab`         | Focus next pane                      |
| `Ctrl+Shift+Tab`            | `Ctrl+Shift+Tab`   | Focus previous pane                  |
| `Ctrl+Alt+1` .. `Ctrl+Alt+9` | `Cmd+1` .. `Cmd+9` | Switch workspace                    |
| `Ctrl+Click` on a URL/path  | `Cmd+Click`        | Open link, or a printed file path, with the OS default program |
| Double-click workspace name | —                  | Rename workspace                     |
| Drag a workspace row        | —                  | Reorder workspaces                   |
| Right-click in a terminal   | —                  | Context menu: copy/paste, split, open a files / editor / git pane |
| `⚙` button (toolbar)        | —                  | Open Settings (general, accounts, activity & notifications, syntax colors, shortcuts, config files) |

The Usage view has no shortcut: use the panel's usage icon
or the command palette.

## Configuration

`config.toml` stores workspaces, layouts and cached shell profiles. It is
rewritten on every structural change (debounced) and on app close.

`theme.toml` stores the color palette, including the editor's syntax colors.
Edit it with **Settings → Syntax Colors**, or open the file via
**Settings → Config Files → Open**.

Both live in the ymux config directory:

| Platform | Path |
|----------|------|
| Windows  | `%APPDATA%\ymux\` |
| macOS    | `~/Library/Application Support/ymux/` |

On macOS the same directory also holds the generated shell-integration files
(`zsh-init/`, `bash-init.sh`). They are rewritten on every shell detection and
are safe to delete.

## Privacy

ymux has no accounts, analytics or telemetry, and runs no servers. Layouts,
scrollback, notes and settings stay on your PC. The only requests ymux makes on
its own are the GitHub update check and, if you use the Usage view, quota
requests to Anthropic and OpenAI that send the token those CLIs already stored
locally to the provider that issued it. If you turn on the activity log you
may also point ymux at webhook destinations and a summarizer endpoint; data goes
only where you send it (see [Activity log & notifications](#activity-log--notifications)).
See the full [privacy policy](./packaging/microsoft-store/PRIVACY.md).

## Development

Requires Rust (stable), Node 20+ and pnpm.

```sh
pnpm install
pnpm tauri dev          # run in dev mode
pnpm tauri build        # MSI on Windows, .app + .dmg on macOS
pnpm test               # fmt + tsc + clippy + tests
npx tsc --noEmit        # TypeScript type check
cargo clippy --workspace -- -D warnings
cargo test -p ytheme -p ypath
cargo test --no-default-features --lib -p ymux
```

`tauri build` produces the installer for the host you run it on (the MSI bundler
is Windows-only and the DMG bundler macOS-only), which is what the release
workflow does. On Linux the Rust crate still checks cleanly with
`cargo check --no-default-features --lib --tests -p ymux`, but the desktop app is
not shipped for Linux.

Maintainers: the Microsoft Store MSIX package is built with
`scripts/package-msix.ps1`; see
[packaging/microsoft-store/README.md](./packaging/microsoft-store/README.md).

## Status

Actively developed. Every tag goes through a Linux test, Windows MSI and macOS
DMG pipeline. See [Releases](https://github.com/YoungMins/ymux/releases) for the
changelog.

## License

[MIT](./LICENSE) © 2026 Kim YoungMin
