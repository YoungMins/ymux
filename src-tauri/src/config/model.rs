//! Serde data model for ymux's on-disk config.
//!
//! The entire user-visible state — workspaces, layouts, panes, and the cached
//! list of detected shell profiles — lives in a single [`Config`] that
//! round-trips through TOML.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current on-disk schema version. Bump when a migration is needed.
///
/// History:
///   1 — initial schema.
///   2 — shell profile args embed the OSC 7 cwd init for PowerShell.
///   3 — cmd.exe switched from `%CD:\=/%` (parse-time) to `$P` (render-time)
///       for dynamic cwd, and Git Bash gained a `--rcfile`-based init that
///       installs a `PROMPT_COMMAND` OSC 7 hook. Any cached shell profile
///       from v1 or v2 is dropped on load so the next bootstrap re-detects.
///   4 — `PaneSpec` gained `kind` (Terminal / Browser), `url`, and
///       `hotkeys`. All new fields have serde defaults so v3 configs load
///       transparently; migration just bumps the version number.
///   5 — Shell detector now enumerates Visual Studio Developer Shells
///       (vswhere) and filters Docker Desktop WSL distros. Cached profiles
///       from v4 are stale; clearing forces a fresh detect.
///   6 — macOS support. `ShellProfile` gained `env` (serde-default, so v5
///       configs still load), and the unix detector now spawns zsh/bash with
///       shell-integration args that emit OSC 7. A v5 cache holds the old
///       argument-free unix profiles, so clearing forces a re-detect.
///   7 — POSIX shells (`sh`, `dash`, `ksh`) gained an `$ENV`-based OSC 7
///       hook. A v6 cache holds an `sh` profile with an empty `env`, so
///       those panes would keep reporting no cwd until a re-detect.
///   8 — Windows shells now start on UTF-8 (code page 65001): cmd.exe
///       chains `chcp`, PowerShell sets `[Console]::OutputEncoding`, and
///       Git Bash is wrapped in a `-c` launcher. The switch lives entirely
///       in the detector's argument lists, so a v7 cache keeps spawning
///       shells on the machine's legacy code page — CP949 on Korean
///       Windows — and every CJK glyph in those panes stays garbled until
///       a re-detect.
///   9 — zsh profile's YMUX_USER_ZDOTDIR semantics changed (empty = none);
///       re-detect shells. A v8 cache can still carry the old `$HOME`
///       fallback, which the zsh shim would take as the user's own ZDOTDIR
///       and then miss their `.zprofile`/`.zshrc` (and every `PATH` edit in
///       them) when their `.zshenv` points ZDOTDIR elsewhere.
pub const CONFIG_VERSION: u32 = 9;

/// Maximum number of workspaces the UI exposes through `Ctrl+1..9`.
pub const MAX_WORKSPACES: u32 = 9;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_active_workspace")]
    pub active_workspace: u32,
    #[serde(default)]
    pub shells: Vec<ShellProfile>,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default = "default_notify_on_bell")]
    pub notify_on_bell: bool,
    #[serde(default = "default_persist_scrollback")]
    pub persist_scrollback: bool,
    /// Draw a terminal whose content is shorter than its pane against the
    /// pane's bottom edge, so the prompt sits on the last row. Presentation
    /// only (the frontend's `bottomAnchor.ts`). Additive with a serde default,
    /// so no `CONFIG_VERSION` bump.
    #[serde(default = "default_bottom_anchor")]
    pub bottom_anchor: bool,
    #[serde(default = "default_paste_image_retention_hours")]
    pub paste_image_retention_hours: u32,
    /// Base directory under which ymux creates git worktrees for panes opened
    /// via "open in worktree". Empty means no base dir configured yet.
    #[serde(default)]
    pub worktree_base_dir: String,
    /// Terminal font size in CSS pixels, shared by every pane. Additive with
    /// a serde default, so older configs load untouched and need no
    /// `CONFIG_VERSION` bump.
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    /// [`ShellProfile::name`] used for newly created panes and workspaces.
    /// Empty means "the first detected shell", which is also the fallback when
    /// the named profile no longer exists (e.g. pwsh was uninstalled).
    /// `String` rather than `Option<String>` per the tagged-enum TOML caveat
    /// the rest of this model follows.
    #[serde(default)]
    pub default_shell: String,
    /// Install ymux's Claude Code hooks into `~/.claude/settings.json` so the
    /// agent tree gets precise per-agent status and subagents. Off by default:
    /// writing another tool's settings file must be opt-in. Additive with a
    /// serde default, so no `CONFIG_VERSION` bump.
    #[serde(default)]
    pub agent_tracking: bool,
    /// Loopback port of the Claude Code hook receiver (`hook_http`). Chosen
    /// once and reused on every launch, because Claude Code's hook URL can't
    /// interpolate env vars — the port is a literal in
    /// `~/.claude/settings.json`. `0` = not chosen yet. Backend-owned like
    /// `agent_tracking` (CLAUDE.md rule 11): never copied in
    /// `merge_layouts_from`. Additive with a serde default, no
    /// `CONFIG_VERSION` bump.
    #[serde(default)]
    pub agent_hook_port: u16,
}

fn default_version() -> u32 {
    CONFIG_VERSION
}
fn default_active_workspace() -> u32 {
    1
}
fn default_notify_on_bell() -> bool {
    true
}
fn default_persist_scrollback() -> bool {
    true
}
fn default_bottom_anchor() -> bool {
    true
}
fn default_paste_image_retention_hours() -> u32 {
    24
}
fn default_font_size() -> u32 {
    13
}

/// Clamp bounds for [`Config::font_size`]. Below ~6px xterm's glyph metrics
/// collapse and `fit()` starts computing absurd column counts; above ~40px a
/// pane can no longer hold a usable terminal.
pub const MIN_FONT_SIZE: u32 = 6;
pub const MAX_FONT_SIZE: u32 = 40;

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            active_workspace: 1,
            shells: Vec::new(),
            workspaces: vec![Workspace::empty(1, "main")],
            notify_on_bell: true,
            persist_scrollback: true,
            bottom_anchor: true,
            paste_image_retention_hours: 24,
            worktree_base_dir: String::new(),
            font_size: default_font_size(),
            default_shell: String::new(),
            agent_tracking: false,
            agent_hook_port: 0,
        }
    }
}

impl Config {
    /// Return the workspace matching `id`, creating it if absent.
    pub fn workspace_mut(&mut self, id: u32) -> &mut Workspace {
        if let Some(idx) = self.workspaces.iter().position(|w| w.id == id) {
            &mut self.workspaces[idx]
        } else {
            self.workspaces
                .push(Workspace::empty(id, format!("workspace-{id}")));
            self.workspaces.last_mut().expect("just pushed")
        }
    }

    /// Return the workspace with id `active_workspace`, falling back to the
    /// first workspace if the active id is stale.
    pub fn active(&self) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|w| w.id == self.active_workspace)
            .or_else(|| self.workspaces.first())
    }

    /// Look up a cached shell profile by name.
    pub fn shell(&self, name: &str) -> Option<&ShellProfile> {
        self.shells.iter().find(|s| s.name == name)
    }

    /// The shell a pane with no explicit shell should run: the profile named
    /// by `default_shell` if it is still installed, else the first detected
    /// one. `None` while the shell cache is empty.
    pub fn resolved_default_shell(&self) -> Option<&str> {
        self.shell(&self.default_shell)
            .or_else(|| self.shells.first())
            .map(|s| s.name.as_str())
    }

    /// Replace the `""` shell sentinel on every terminal pane with
    /// [`Self::resolved_default_shell`], so a pane keeps the shell it first
    /// opened with instead of following a later `default_shell` change.
    /// Non-empty names (even ones no longer installed) and non-terminal panes
    /// are left alone; a no-op while `shells` is empty. Returns whether any
    /// pane changed.
    pub fn pin_pane_shells(&mut self) -> bool {
        let Some(name) = self.resolved_default_shell().map(str::to_owned) else {
            return false;
        };
        let mut changed = false;
        for ws in &mut self.workspaces {
            ws.root.for_each_pane_mut(&mut |pane| {
                if pane.pane_kind == PaneKind::Terminal && pane.shell.is_empty() {
                    pane.shell = name.clone();
                    changed = true;
                }
            });
        }
        changed
    }

    /// Apply layout / workspace updates from `incoming` onto `self`, treating
    /// `shells` as a backend-owned detection cache: it is only overwritten
    /// when the incoming config carries a non-empty list. This stops a stale
    /// frontend snapshot (e.g. one captured before shell detection finished)
    /// from clobbering the cache and breaking subsequent `spawn_pane` calls.
    pub fn merge_layouts_from(&mut self, incoming: Config) {
        self.version = incoming.version;
        self.active_workspace = incoming.active_workspace;
        self.workspaces = incoming.workspaces;
        // Plain user settings: the frontend owns these outright — it received
        // them at bootstrap and is the only thing that edits them — so they
        // have to be copied back or every save silently reverts the user's
        // choice to whatever was on disk at launch. `shells` below is an
        // exception, being a backend-owned detection cache.
        self.notify_on_bell = incoming.notify_on_bell;
        self.persist_scrollback = incoming.persist_scrollback;
        self.bottom_anchor = incoming.bottom_anchor;
        self.paste_image_retention_hours = incoming.paste_image_retention_hours;
        self.worktree_base_dir = incoming.worktree_base_dir;
        self.font_size = incoming.font_size;
        self.default_shell = incoming.default_shell;
        // `agent_tracking` is deliberately NOT copied, the other exception to
        // the rule above: it is backend-owned. `set_agent_tracking` flips it
        // in the same step that installs/removes the Claude Code hooks, so it
        // must mirror what is actually in settings.json. Copying a (possibly
        // stale) frontend snapshot here would let an unrelated layout save
        // turn tracking off while the hooks stay installed.
        // `agent_hook_port` likewise: the installed hooks' URL carries it.
        if !incoming.shells.is_empty() {
            self.shells = incoming.shells;
        }
    }

    /// Overwrite each pane's `cwd` with the matching entry from `cwds`
    /// (keyed on pane id) if present. Panes not present in the map are left
    /// untouched, which means panes that never reported an OSC 7 cwd keep
    /// whatever they had in config (usually their initial spawn directory).
    pub fn patch_cwds(&mut self, cwds: &std::collections::HashMap<Uuid, String>) {
        for ws in &mut self.workspaces {
            ws.root.for_each_pane_mut(&mut |pane| {
                if let Some(cwd) = cwds.get(&pane.id) {
                    pane.cwd = Some(cwd.clone());
                }
            });
        }
    }

    /// Apply any schema migrations needed to bring an on-disk config up to
    /// the current [`CONFIG_VERSION`]. Called from `ConfigStore::load`.
    pub fn migrate(&mut self) {
        if self.version < CONFIG_VERSION {
            // Every historical schema bump so far has meant the cached
            // shell profile args were incompatible with the latest detector
            // output. Dropping the cache forces re-detection on the next
            // bootstrap, which is cheap and always correct.
            self.shells.clear();
            self.version = CONFIG_VERSION;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: u32,
    pub name: String,
    pub root: LayoutNode,
}

impl Workspace {
    pub fn empty(id: u32, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            root: LayoutNode::Pane(PaneSpec::new_default()),
        }
    }

    /// Iterate over all pane specs in this workspace in depth-first order.
    pub fn panes(&self) -> Vec<&PaneSpec> {
        fn walk<'a>(node: &'a LayoutNode, out: &mut Vec<&'a PaneSpec>) {
            match node {
                LayoutNode::Pane(p) => out.push(p),
                LayoutNode::Split { a, b, .. } => {
                    walk(a, out);
                    walk(b, out);
                }
                LayoutNode::Tabs { children, .. } => {
                    for c in children {
                        walk(c, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }
}

/// Recursive layout tree. Tagged enum so TOML reads `kind = "split" | "pane"
/// | "tabs"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
// PaneSpec has grown several string/optional fields (worktree, scrollback,
// etc.) and now trips clippy::large_enum_variant against the much smaller
// Split/Tabs variants. Boxing PaneSpec would ripple through every
// `LayoutNode::Pane(...)` construction/match across the crate for a
// pane-tree data model that isn't allocated in hot loops, so it's not worth
// the churn right now.
#[allow(clippy::large_enum_variant)]
pub enum LayoutNode {
    Pane(PaneSpec),
    Split {
        direction: SplitDir,
        /// Fraction of the container occupied by `a`. Clamped to [0.05, 0.95]
        /// on load.
        ratio: f32,
        a: Box<LayoutNode>,
        b: Box<LayoutNode>,
    },
    Tabs {
        /// Stable identity for the tab group. Needed because the group is the
        /// unit the UI addresses: the renderer caches one `PaneGroup` (and its
        /// HotKeyBar) per id across layout rebuilds, and the file dock's
        /// "one viewer tab per pane, reused" registry has to survive tabs
        /// being opened and closed around it — which keying on a member pane
        /// id cannot. Defaulted rather than optional, per the tagged-enum
        /// TOML caveat (CLAUDE.md rule 3); a node written without one gets a
        /// fresh id on load, which is harmless because the registry is
        /// runtime-only.
        #[serde(default = "default_tabs_id")]
        id: Uuid,
        active: usize,
        children: Vec<LayoutNode>,
    },
}

fn default_tabs_id() -> Uuid {
    Uuid::new_v4()
}

impl LayoutNode {
    /// Split this node by wrapping it in a new [`LayoutNode::Split`] with a
    /// fresh pane on the `b` side. Returns a reference to the newly added
    /// pane spec.
    pub fn split_with(&mut self, direction: SplitDir, new_pane: PaneSpec) {
        let taken = std::mem::replace(self, LayoutNode::Pane(PaneSpec::placeholder()));
        *self = LayoutNode::Split {
            direction,
            ratio: 0.5,
            a: Box::new(taken),
            b: Box::new(LayoutNode::Pane(new_pane)),
        };
    }

    /// Remove the pane with `id` from the tree. If removing a pane collapses a
    /// split to a single child, the split is replaced by that child. Returns
    /// `true` if the pane was found and removed.
    pub fn remove_pane(&mut self, id: Uuid) -> RemoveResult {
        match self {
            LayoutNode::Pane(p) => {
                if p.id == id {
                    RemoveResult::RemoveSelf
                } else {
                    RemoveResult::NotFound
                }
            }
            LayoutNode::Split { a, b, .. } => match a.remove_pane(id) {
                RemoveResult::RemoveSelf => {
                    let kept = std::mem::replace(b.as_mut(), LayoutNode::placeholder());
                    *self = kept;
                    RemoveResult::Removed
                }
                RemoveResult::Removed => RemoveResult::Removed,
                RemoveResult::NotFound => match b.remove_pane(id) {
                    RemoveResult::RemoveSelf => {
                        let kept = std::mem::replace(a.as_mut(), LayoutNode::placeholder());
                        *self = kept;
                        RemoveResult::Removed
                    }
                    other => other,
                },
            },
            LayoutNode::Tabs {
                children, active, ..
            } => {
                let mut found = RemoveResult::NotFound;
                let mut to_remove: Option<usize> = None;
                for (idx, c) in children.iter_mut().enumerate() {
                    match c.remove_pane(id) {
                        RemoveResult::RemoveSelf => {
                            to_remove = Some(idx);
                            found = RemoveResult::Removed;
                            break;
                        }
                        RemoveResult::Removed => {
                            found = RemoveResult::Removed;
                            break;
                        }
                        RemoveResult::NotFound => {}
                    }
                }
                if let Some(idx) = to_remove {
                    children.remove(idx);
                    if children.is_empty() {
                        return RemoveResult::RemoveSelf;
                    }
                    if *active >= children.len() {
                        *active = children.len() - 1;
                    }
                }
                found
            }
        }
    }

    /// Placeholder used during in-place tree surgery. Never persisted.
    pub(crate) fn placeholder() -> Self {
        LayoutNode::Pane(PaneSpec::placeholder())
    }

    /// Find a pane by id, returning a mutable reference.
    pub fn find_pane_mut(&mut self, id: Uuid) -> Option<&mut PaneSpec> {
        match self {
            LayoutNode::Pane(p) if p.id == id => Some(p),
            LayoutNode::Pane(_) => None,
            LayoutNode::Split { a, b, .. } => a.find_pane_mut(id).or_else(|| b.find_pane_mut(id)),
            LayoutNode::Tabs { children, .. } => {
                children.iter_mut().find_map(|c| c.find_pane_mut(id))
            }
        }
    }

    /// Apply `visit` to every pane in the subtree, depth-first.
    pub fn for_each_pane_mut(&mut self, visit: &mut dyn FnMut(&mut PaneSpec)) {
        match self {
            LayoutNode::Pane(p) => visit(p),
            LayoutNode::Split { a, b, .. } => {
                a.for_each_pane_mut(visit);
                b.for_each_pane_mut(visit);
            }
            LayoutNode::Tabs { children, .. } => {
                for c in children {
                    c.for_each_pane_mut(visit);
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoveResult {
    /// The caller should replace itself with the sibling.
    RemoveSelf,
    /// Pane was removed somewhere deeper; nothing else to do.
    Removed,
    /// Pane not present in this subtree.
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDir {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PaneKind {
    #[default]
    Terminal,
    Browser,
    NativeBrowser,
    EmbeddedBrowser,
    /// A GUI file manager rendered by the frontend (`src/files/FilesPane.ts`).
    /// It has no PTY. The directory it shows is the pane's `cwd`.
    Files,
    /// A text editor rendered by the frontend (`src/editor/EditorPane.ts`).
    /// It has no PTY. The file it has open is the pane's `file_path`.
    Editor,
    /// A git log / branch / worktree view rendered by the frontend
    /// (`src/git/GitPane.ts`). It has no PTY. The repository is the one
    /// containing the pane's `cwd`.
    Git,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HotKeyDef {
    pub label: String,
    /// Raw command string. For `batch = true`, newlines separate discrete
    /// lines sent one-by-one, each terminated with `\r`.
    pub command: String,
    #[serde(default)]
    pub batch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneSpec {
    pub id: Uuid,
    #[serde(default)]
    pub title: Option<String>,
    /// Reference to a [`ShellProfile::name`]. The empty string is a sentinel
    /// ("the default shell") that `load_bootstrap` pins to a concrete name at
    /// boot via [`Config::pin_pane_shells`]; it only survives on non-terminal
    /// panes, or while no shells are detected.
    #[serde(default)]
    pub shell: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub startup_cmd: Option<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Terminal vs Browser. Renamed from `kind` to avoid collision with the
    /// `LayoutNode` tag (also named `kind`) when the pane spec is flattened
    /// into the tagged enum.
    #[serde(default, rename = "pane_kind")]
    pub pane_kind: PaneKind,
    /// Target URL when `kind == Browser`. Ignored for terminal panes.
    #[serde(default)]
    pub url: Option<String>,
    /// User-defined HotKey buttons shown above terminal panes.
    #[serde(default)]
    pub hotkeys: Vec<HotKeyDef>,
    /// Custom background color for this pane (hex like "#1a2b3c", or "" for default).
    #[serde(default)]
    pub bg_color: String,
    /// Non-empty when this pane is rooted in a git worktree created by ymux.
    /// Holds the worktree directory; used to offer cleanup when the pane closes.
    #[serde(default)]
    pub worktree_path: String,
    /// Absolute path of the file an `Editor` pane has open. Empty = untitled
    /// (an editor pane restored with no file shows its empty state).
    /// A `String`, not an `Option<String>`: rule 3 — `Option<T>` inside a
    /// `#[serde(tag = "kind")]` tagged enum does not round-trip through TOML,
    /// and `PaneSpec` is flattened into `LayoutNode::Pane`.
    #[serde(default)]
    pub file_path: String,
}

impl PaneSpec {
    pub fn new_default() -> Self {
        Self {
            id: Uuid::new_v4(),
            title: None,
            shell: String::new(),
            cwd: None,
            startup_cmd: None,
            env: Vec::new(),
            pane_kind: PaneKind::Terminal,
            url: None,
            hotkeys: Vec::new(),
            bg_color: String::new(),
            worktree_path: String::new(),
            file_path: String::new(),
        }
    }

    /// An all-zero [`Uuid`] spec used only as a transient placeholder during
    /// in-place tree surgery. Never written to disk.
    pub(crate) fn placeholder() -> Self {
        Self {
            id: Uuid::nil(),
            title: None,
            shell: String::new(),
            cwd: None,
            startup_cmd: None,
            env: Vec::new(),
            pane_kind: PaneKind::Terminal,
            url: None,
            hotkeys: Vec::new(),
            bg_color: String::new(),
            worktree_path: String::new(),
            file_path: String::new(),
        }
    }

    pub fn new_browser(url: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: None,
            shell: String::new(),
            cwd: None,
            startup_cmd: None,
            env: Vec::new(),
            pane_kind: PaneKind::Browser,
            url: Some(url.into()),
            hotkeys: Vec::new(),
            bg_color: String::new(),
            worktree_path: String::new(),
            file_path: String::new(),
        }
    }

    /// A files pane showing `cwd`. `None` lets the frontend open the home
    /// directory on first show.
    pub fn new_files(cwd: Option<String>) -> Self {
        Self {
            pane_kind: PaneKind::Files,
            cwd,
            ..Self::new_default()
        }
    }

    /// An editor pane with `path` open (empty = untitled).
    pub fn new_editor(path: impl Into<String>) -> Self {
        Self {
            pane_kind: PaneKind::Editor,
            file_path: path.into(),
            ..Self::new_default()
        }
    }

    /// A git pane on the repository containing `cwd`. `None` makes the
    /// frontend follow the active pane's directory from the start.
    pub fn new_git(cwd: Option<String>) -> Self {
        Self {
            pane_kind: PaneKind::Git,
            cwd,
            ..Self::new_default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellProfile {
    pub name: String,
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    /// Environment variables injected into every PTY spawned from this
    /// profile, applied *before* the pane's own `PaneSpec::env` so a pane can
    /// still override them. This is how the macOS zsh profile points
    /// `ZDOTDIR` at ymux's shell-integration shim (see `shell::detect`) — the
    /// only portable way to get an OSC 7 hook into an interactive zsh without
    /// editing the user's own dotfiles.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane_with_id(id: Uuid) -> LayoutNode {
        LayoutNode::Pane(PaneSpec {
            id,
            title: None,
            shell: String::new(),
            cwd: None,
            startup_cmd: None,
            env: Vec::new(),
            pane_kind: PaneKind::Terminal,
            url: None,
            hotkeys: Vec::new(),
            bg_color: String::new(),
            worktree_path: String::new(),
            file_path: String::new(),
        })
    }

    #[test]
    fn split_wraps_existing_pane() {
        let a = Uuid::new_v4();
        let mut node = pane_with_id(a);
        node.split_with(SplitDir::Horizontal, PaneSpec::new_default());
        match node {
            LayoutNode::Split {
                direction,
                a: lhs,
                b: rhs,
                ratio,
            } => {
                assert_eq!(direction, SplitDir::Horizontal);
                assert!((ratio - 0.5).abs() < 1e-6);
                assert!(matches!(*lhs, LayoutNode::Pane(ref p) if p.id == a));
                assert!(matches!(*rhs, LayoutNode::Pane(_)));
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn remove_collapses_split() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut node = LayoutNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.5,
            a: Box::new(pane_with_id(a)),
            b: Box::new(pane_with_id(b)),
        };
        let result = node.remove_pane(a);
        assert_eq!(result, RemoveResult::Removed);
        match node {
            LayoutNode::Pane(p) => assert_eq!(p.id, b),
            _ => panic!("split should have collapsed"),
        }
    }

    #[test]
    fn remove_last_pane_signals_self_removal() {
        let a = Uuid::new_v4();
        let mut node = pane_with_id(a);
        let result = node.remove_pane(a);
        assert_eq!(result, RemoveResult::RemoveSelf);
    }

    #[test]
    fn remove_nonexistent_is_noop() {
        let a = Uuid::new_v4();
        let other = Uuid::new_v4();
        let mut node = pane_with_id(a);
        assert_eq!(node.remove_pane(other), RemoveResult::NotFound);
    }

    #[test]
    fn nested_split_removal_keeps_structure() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let mut node = LayoutNode::Split {
            direction: SplitDir::Horizontal,
            ratio: 0.5,
            a: Box::new(pane_with_id(a)),
            b: Box::new(LayoutNode::Split {
                direction: SplitDir::Vertical,
                ratio: 0.5,
                a: Box::new(pane_with_id(b)),
                b: Box::new(pane_with_id(c)),
            }),
        };
        assert_eq!(node.remove_pane(b), RemoveResult::Removed);
        // The inner split collapses to just `c`, so the outer split is now
        // (a | c).
        match node {
            LayoutNode::Split { a: lhs, b: rhs, .. } => {
                assert!(matches!(*lhs, LayoutNode::Pane(ref p) if p.id == a));
                assert!(matches!(*rhs, LayoutNode::Pane(ref p) if p.id == c));
            }
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn workspace_panes_depth_first() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let ws = Workspace {
            id: 1,
            name: "main".into(),
            root: LayoutNode::Split {
                direction: SplitDir::Horizontal,
                ratio: 0.5,
                a: Box::new(pane_with_id(a)),
                b: Box::new(LayoutNode::Split {
                    direction: SplitDir::Vertical,
                    ratio: 0.5,
                    a: Box::new(pane_with_id(b)),
                    b: Box::new(pane_with_id(c)),
                }),
            },
        };
        let ids: Vec<_> = ws.panes().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![a, b, c]);
    }

    #[test]
    fn config_toml_round_trip() {
        let mut cfg = Config::default();
        cfg.shells.push(ShellProfile {
            name: "PowerShell 7".into(),
            executable: "C:\\Program Files\\PowerShell\\7\\pwsh.exe".into(),
            args: vec!["-NoLogo".into()],
            icon: Some("pwsh".into()),
            color: None,
            env: Vec::new(),
        });
        let serialized = toml::to_string(&cfg).expect("serialize");
        let parsed: Config = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(parsed.version, CONFIG_VERSION);
        assert_eq!(parsed.active_workspace, 1);
        assert_eq!(parsed.shells.len(), 1);
        assert_eq!(parsed.shells[0].name, "PowerShell 7");
        assert_eq!(parsed.workspaces.len(), 1);
    }

    #[test]
    fn notify_on_bell_defaults_true_when_absent() {
        let toml_str = "version = 5\nactive_workspace = 1\n";
        let parsed: Config = toml::from_str(toml_str).expect("deserialize");
        assert!(parsed.notify_on_bell);
    }

    #[test]
    fn persist_scrollback_defaults_true_when_absent() {
        let toml_str = "version = 5\nactive_workspace = 1\n";
        let parsed: Config = toml::from_str(toml_str).expect("deserialize");
        assert!(parsed.persist_scrollback);
    }

    #[test]
    fn bottom_anchor_defaults_true_when_absent() {
        let parsed: Config = toml::from_str("version = 7\n").expect("parse");
        assert!(parsed.bottom_anchor);
    }

    #[test]
    fn bottom_anchor_roundtrips_false() {
        let config = Config {
            bottom_anchor: false,
            ..Config::default()
        };
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        assert!(!loaded.bottom_anchor);
    }

    #[test]
    fn paste_image_retention_hours_defaults_to_24_when_absent() {
        let toml_str = "version = 5\nactive_workspace = 1\n";
        let parsed: Config = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(parsed.paste_image_retention_hours, 24);
    }

    #[test]
    fn default_shell_defaults_to_empty_when_absent() {
        let toml_str = "version = 5\nactive_workspace = 1\n";
        let parsed: Config = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(parsed.default_shell, "");
    }

    #[test]
    fn default_shell_toml_roundtrip() {
        let config = Config {
            font_size: 13,
            default_shell: "Git Bash".into(),
            ..Config::default()
        };
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(loaded.default_shell, "Git Bash");
    }

    /// The frontend is the only editor of the plain settings fields, so a save
    /// has to carry them back onto the backend copy. Before this, toggling
    /// "notify on bell" (or picking a default shell) looked like it worked and
    /// then reverted on the next launch.
    #[test]
    fn merge_layouts_carries_user_settings_back() {
        let mut backend = Config::default();
        let frontend_save = Config {
            notify_on_bell: false,
            persist_scrollback: false,
            bottom_anchor: false,
            paste_image_retention_hours: 72,
            worktree_base_dir: "D:\\wt".into(),
            font_size: 18,
            default_shell: "pwsh".into(),
            ..Config::default()
        };
        backend.merge_layouts_from(frontend_save);
        assert!(!backend.notify_on_bell);
        assert!(!backend.persist_scrollback);
        assert!(!backend.bottom_anchor);
        assert_eq!(backend.paste_image_retention_hours, 72);
        assert_eq!(backend.worktree_base_dir, "D:\\wt");
        assert_eq!(backend.font_size, 18);
        assert_eq!(backend.default_shell, "pwsh");
    }

    /// `agent_tracking` is backend-owned: `set_agent_tracking` flips it
    /// alongside installing/removing the hooks. A layout save carrying a
    /// stale frontend copy must not override it, or tracking silently turns
    /// off while the hooks stay installed (and vice versa).
    #[test]
    fn merge_layouts_does_not_carry_agent_tracking() {
        let mut backend = Config {
            agent_tracking: true,
            ..Config::default()
        };
        let stale_save = Config {
            agent_tracking: false,
            agent_hook_port: 0,
            ..Config::default()
        };
        backend.merge_layouts_from(stale_save);
        assert!(backend.agent_tracking);

        let mut backend_off = Config::default();
        backend_off.merge_layouts_from(Config {
            agent_tracking: true,
            ..Config::default()
        });
        assert!(!backend_off.agent_tracking);
    }

    /// A config written before `font_size` existed must load with the default
    /// rather than 0, which would render an invisible terminal.
    #[test]
    fn font_size_defaults_when_absent() {
        let parsed: Config = toml::from_str("version = 7\n").expect("parse");
        assert_eq!(parsed.font_size, 13);
    }

    #[test]
    fn font_size_roundtrips() {
        let config = Config {
            font_size: 20,
            ..Config::default()
        };
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(loaded.font_size, 20);
    }

    /// Hooks are written into another tool's settings file, so the setting
    /// must be opt-in: a config written before it existed loads as off.
    #[test]
    fn agent_tracking_defaults_off_when_absent() {
        let parsed: Config = toml::from_str("version = 7\n").expect("parse");
        assert!(!parsed.agent_tracking);
    }

    /// The hook receiver's port is backend-owned like `agent_tracking`: it is
    /// written into `~/.claude/settings.json`, so a stale frontend save must
    /// never move it away from the URL the installed hooks point at.
    #[test]
    fn merge_layouts_does_not_carry_agent_hook_port() {
        let mut backend = Config {
            agent_hook_port: 41234,
            ..Config::default()
        };
        backend.merge_layouts_from(Config {
            agent_hook_port: 0,
            ..Config::default()
        });
        assert_eq!(backend.agent_hook_port, 41234);
    }

    #[test]
    fn agent_hook_port_defaults_to_unset_and_roundtrips() {
        let parsed: Config = toml::from_str("version = 7\n").expect("parse");
        assert_eq!(parsed.agent_hook_port, 0);
        let config = Config {
            agent_hook_port: 41234,
            ..Config::default()
        };
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(loaded.agent_hook_port, 41234);
    }

    #[test]
    fn agent_tracking_roundtrips() {
        let config = Config {
            agent_tracking: true,
            ..Config::default()
        };
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        assert!(loaded.agent_tracking);
    }

    #[test]
    fn max_workspaces_is_nine() {
        assert_eq!(MAX_WORKSPACES, 9);
    }

    #[test]
    fn merge_layouts_preserves_shells_when_incoming_is_empty() {
        // Regression: a stale frontend snapshot saved with `shells: []` used
        // to wipe the backend's detected shell cache, breaking the next
        // spawn_pane call with "unknown shell profile".
        let mut backend = Config {
            version: CONFIG_VERSION,
            active_workspace: 1,
            shells: vec![ShellProfile {
                name: "Windows PowerShell".into(),
                executable: "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".into(),
                args: vec!["-NoLogo".into()],
                icon: None,
                color: None,
                env: Vec::new(),
            }],
            workspaces: vec![Workspace::empty(1, "main")],
            notify_on_bell: true,
            persist_scrollback: true,
            bottom_anchor: true,
            paste_image_retention_hours: 24,
            worktree_base_dir: String::new(),
            font_size: 13,
            default_shell: String::new(),
            agent_tracking: false,
            agent_hook_port: 0,
        };
        let frontend_save = Config {
            version: CONFIG_VERSION,
            active_workspace: 2,
            shells: vec![],
            workspaces: vec![Workspace::empty(2, "two")],
            notify_on_bell: true,
            persist_scrollback: true,
            bottom_anchor: true,
            paste_image_retention_hours: 24,
            worktree_base_dir: String::new(),
            font_size: 13,
            default_shell: String::new(),
            agent_tracking: false,
            agent_hook_port: 0,
        };
        backend.merge_layouts_from(frontend_save);
        assert_eq!(backend.active_workspace, 2);
        assert_eq!(backend.workspaces.len(), 1);
        assert_eq!(backend.workspaces[0].id, 2);
        // Crucial: shells survived.
        assert_eq!(backend.shells.len(), 1);
        assert_eq!(backend.shells[0].name, "Windows PowerShell");
    }

    #[test]
    fn patch_cwds_updates_matching_panes_only() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut cfg = Config {
            version: CONFIG_VERSION,
            active_workspace: 1,
            shells: vec![],
            workspaces: vec![Workspace {
                id: 1,
                name: "main".into(),
                root: LayoutNode::Split {
                    direction: SplitDir::Horizontal,
                    ratio: 0.5,
                    a: Box::new(LayoutNode::Pane(PaneSpec {
                        id: a,
                        title: None,
                        shell: "PowerShell 7".into(),
                        cwd: Some("C:\\old".into()),
                        startup_cmd: None,
                        env: vec![],
                        pane_kind: PaneKind::Terminal,
                        url: None,
                        hotkeys: vec![],
                        bg_color: String::new(),
                        worktree_path: String::new(),
                        file_path: String::new(),
                    })),
                    b: Box::new(LayoutNode::Pane(PaneSpec {
                        id: b,
                        title: None,
                        shell: "PowerShell 7".into(),
                        cwd: None,
                        startup_cmd: None,
                        env: vec![],
                        pane_kind: PaneKind::Terminal,
                        url: None,
                        hotkeys: vec![],
                        bg_color: String::new(),
                        worktree_path: String::new(),
                        file_path: String::new(),
                    })),
                },
            }],
            notify_on_bell: true,
            persist_scrollback: true,
            bottom_anchor: true,
            paste_image_retention_hours: 24,
            worktree_base_dir: String::new(),
            font_size: 13,
            default_shell: String::new(),
            agent_tracking: false,
            agent_hook_port: 0,
        };
        let mut cwds = std::collections::HashMap::new();
        cwds.insert(a, "C:\\Users\\alice\\dev".to_string());
        // Note: no entry for `b` — it should remain untouched.
        cfg.patch_cwds(&cwds);

        let a_pane = cfg.workspaces[0].root.find_pane_mut(a).unwrap();
        assert_eq!(a_pane.cwd.as_deref(), Some("C:\\Users\\alice\\dev"));
        let b_pane = cfg.workspaces[0].root.find_pane_mut(b).unwrap();
        assert_eq!(b_pane.cwd, None);
    }

    #[test]
    fn migrate_v1_to_v2_clears_shells() {
        let mut cfg = Config {
            version: 1,
            active_workspace: 1,
            shells: vec![ShellProfile {
                name: "stale".into(),
                executable: "/stale".into(),
                args: vec!["-old".into()],
                icon: None,
                color: None,
                env: Vec::new(),
            }],
            workspaces: vec![Workspace::empty(1, "main")],
            notify_on_bell: true,
            persist_scrollback: true,
            bottom_anchor: true,
            paste_image_retention_hours: 24,
            worktree_base_dir: String::new(),
            font_size: 13,
            default_shell: String::new(),
            agent_tracking: false,
            agent_hook_port: 0,
        };
        cfg.migrate();
        assert_eq!(cfg.version, CONFIG_VERSION);
        assert!(cfg.shells.is_empty(), "stale v1 shells should be cleared");
    }

    #[test]
    fn migrate_is_noop_on_current_version() {
        let mut cfg = Config::default();
        cfg.shells.push(ShellProfile {
            name: "keep".into(),
            executable: "/keep".into(),
            args: vec![],
            icon: None,
            color: None,
            env: Vec::new(),
        });
        cfg.migrate();
        assert_eq!(cfg.shells.len(), 1);
    }

    #[test]
    fn for_each_pane_mut_visits_all_panes() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let mut node = LayoutNode::Split {
            direction: SplitDir::Horizontal,
            ratio: 0.5,
            a: Box::new(pane_with_id(a)),
            b: Box::new(LayoutNode::Split {
                direction: SplitDir::Vertical,
                ratio: 0.5,
                a: Box::new(pane_with_id(b)),
                b: Box::new(pane_with_id(c)),
            }),
        };
        let mut visited = Vec::new();
        node.for_each_pane_mut(&mut |p| {
            visited.push(p.id);
            p.title = Some("visited".to_string());
        });
        assert_eq!(visited, vec![a, b, c]);
        // Mutation should have persisted.
        assert_eq!(
            node.find_pane_mut(a).unwrap().title.as_deref(),
            Some("visited")
        );
    }

    #[test]
    fn migrate_v3_to_v4_defaults_panes_to_terminal() {
        // v3 configs have no `pane_kind`, `url`, or `hotkeys` fields. Loading
        // them should default every pane to Terminal with no hotkeys.
        let toml_v3 = r#"
version = 3
active_workspace = 1

[[workspaces]]
id = 1
name = "main"

[workspaces.root]
kind = "pane"
id = "00000000-0000-0000-0000-000000000001"
shell = "PowerShell 7"
"#;
        let mut cfg: Config = toml::from_str(toml_v3).expect("parse v3");
        cfg.migrate();
        assert_eq!(cfg.version, CONFIG_VERSION);
        let pane = cfg.workspaces[0].panes()[0];
        assert_eq!(pane.pane_kind, PaneKind::Terminal);
        assert!(pane.url.is_none());
        assert!(pane.hotkeys.is_empty());
    }

    #[test]
    fn browser_pane_round_trips_through_toml() {
        // The `LayoutNode` tag `kind` and the `PaneSpec.pane_kind` field must
        // coexist without collision — `pane_kind` is explicitly renamed for
        // exactly this reason.
        let mut cfg = Config::default();
        cfg.workspaces[0].root = LayoutNode::Pane(PaneSpec::new_browser("https://example.com"));
        let serialized = toml::to_string(&cfg).expect("serialize");
        let parsed: Config = toml::from_str(&serialized).expect("deserialize");
        let pane = parsed.workspaces[0].panes()[0];
        assert_eq!(pane.pane_kind, PaneKind::Browser);
        assert_eq!(pane.url.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn hotkey_def_serde_round_trip() {
        let spec = PaneSpec {
            hotkeys: vec![
                HotKeyDef {
                    label: "git status".into(),
                    command: "git status".into(),
                    batch: false,
                },
                HotKeyDef {
                    label: "pull+install".into(),
                    command: "git pull\npnpm install".into(),
                    batch: true,
                },
            ],
            ..PaneSpec::new_default()
        };
        let toml_str = toml::to_string(&spec).expect("serialize");
        let parsed: PaneSpec = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(parsed.hotkeys.len(), 2);
        assert_eq!(parsed.hotkeys[0].label, "git status");
        assert!(!parsed.hotkeys[0].batch);
        assert!(parsed.hotkeys[1].batch);
    }

    #[test]
    fn merge_layouts_replaces_shells_when_incoming_is_nonempty() {
        let mut backend = Config {
            version: CONFIG_VERSION,
            active_workspace: 1,
            shells: vec![ShellProfile {
                name: "old".into(),
                executable: "/old".into(),
                args: vec![],
                icon: None,
                color: None,
                env: Vec::new(),
            }],
            workspaces: vec![Workspace::empty(1, "main")],
            notify_on_bell: true,
            persist_scrollback: true,
            bottom_anchor: true,
            paste_image_retention_hours: 24,
            worktree_base_dir: String::new(),
            font_size: 13,
            default_shell: String::new(),
            agent_tracking: false,
            agent_hook_port: 0,
        };
        let frontend_save = Config {
            version: CONFIG_VERSION,
            active_workspace: 1,
            shells: vec![
                ShellProfile {
                    name: "new-a".into(),
                    executable: "/a".into(),
                    args: vec![],
                    icon: None,
                    color: None,
                    env: Vec::new(),
                },
                ShellProfile {
                    name: "new-b".into(),
                    executable: "/b".into(),
                    args: vec![],
                    icon: None,
                    color: None,
                    env: Vec::new(),
                },
            ],
            workspaces: vec![Workspace::empty(1, "main")],
            notify_on_bell: true,
            persist_scrollback: true,
            bottom_anchor: true,
            paste_image_retention_hours: 24,
            worktree_base_dir: String::new(),
            font_size: 13,
            default_shell: String::new(),
            agent_tracking: false,
            agent_hook_port: 0,
        };
        backend.merge_layouts_from(frontend_save);
        assert_eq!(backend.shells.len(), 2);
        assert_eq!(backend.shells[0].name, "new-a");
        assert_eq!(backend.shells[1].name, "new-b");
    }

    /// Regression: bg_color must survive a full Config → TOML → Config round-trip.
    /// This caught the Option<String> deserialization bug in tagged enums.
    #[test]
    fn bg_color_toml_roundtrip() {
        let mut config = Config::default();
        let ws = config.workspace_mut(1);
        if let LayoutNode::Pane(ref mut p) = ws.root {
            p.bg_color = "#5b6e86".to_string();
        }
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        assert!(
            toml_str.contains("bg_color = \"#5b6e86\""),
            "TOML must contain bg_color"
        );
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        let pane = loaded.workspaces[0].panes()[0];
        assert_eq!(pane.bg_color, "#5b6e86", "bg_color lost in round-trip");
    }

    /// Verify that all PaneSpec fields survive TOML round-trip (catches
    /// the "forgot to add new field" class of bugs).
    #[test]
    fn panespec_all_fields_roundtrip() {
        let spec = PaneSpec {
            id: Uuid::new_v4(),
            title: Some("test".into()),
            shell: "pwsh".into(),
            cwd: Some("C:\\Users".into()),
            startup_cmd: Some("echo hi".into()),
            env: vec![("FOO".into(), "bar".into())],
            pane_kind: PaneKind::Terminal,
            url: None,
            hotkeys: vec![HotKeyDef {
                label: "build".into(),
                command: "cargo build".into(),
                batch: true,
            }],
            bg_color: "#1a2b3c".to_string(),
            worktree_path: "C:\\wt\\agent-1".to_string(),
            file_path: "C:\\src\\작업\\main.rs".to_string(),
        };
        let config = Config {
            version: CONFIG_VERSION,
            active_workspace: 1,
            shells: vec![],
            workspaces: vec![Workspace {
                id: 1,
                name: "test".into(),
                root: LayoutNode::Pane(spec.clone()),
            }],
            notify_on_bell: true,
            persist_scrollback: true,
            bottom_anchor: true,
            paste_image_retention_hours: 24,
            worktree_base_dir: String::new(),
            font_size: 13,
            default_shell: String::new(),
            agent_tracking: false,
            agent_hook_port: 0,
        };
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        let loaded_spec = loaded.workspaces[0].panes()[0];

        assert_eq!(loaded_spec.id, spec.id);
        assert_eq!(loaded_spec.title, spec.title);
        assert_eq!(loaded_spec.shell, spec.shell);
        assert_eq!(loaded_spec.cwd, spec.cwd);
        assert_eq!(loaded_spec.startup_cmd, spec.startup_cmd);
        assert_eq!(loaded_spec.env, spec.env);
        assert_eq!(loaded_spec.hotkeys.len(), 1);
        assert_eq!(loaded_spec.hotkeys[0].label, "build");
        assert!(loaded_spec.hotkeys[0].batch);
        assert_eq!(loaded_spec.bg_color, "#1a2b3c");
        assert_eq!(loaded_spec.worktree_path, "C:\\wt\\agent-1");
        assert_eq!(loaded_spec.file_path, "C:\\src\\작업\\main.rs");
    }

    /// An editor pane keeps its kind and its file through TOML, nested in a
    /// split *and* in a tab group — rule 3's two tagged-enum shapes. The
    /// file is the editor pane's only persisted state (spec §0.2); losing it
    /// reopens the pane empty.
    #[test]
    fn editor_pane_kind_and_file_path_roundtrip_nested() {
        let split = PaneSpec::new_editor("D:\\작업\\src\\lib.rs");
        let in_tabs = PaneSpec::new_editor("/home/me/notes.md");
        let untitled = PaneSpec::new_editor("");
        let mut config = Config::default();
        config.workspaces[0].root = LayoutNode::Split {
            direction: SplitDir::Horizontal,
            ratio: 0.5,
            a: Box::new(LayoutNode::Pane(split.clone())),
            b: Box::new(LayoutNode::Tabs {
                id: Uuid::new_v4(),
                active: 1,
                children: vec![
                    LayoutNode::Pane(PaneSpec::new_default()),
                    LayoutNode::Pane(in_tabs.clone()),
                    LayoutNode::Pane(untitled.clone()),
                ],
            }),
        };
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        assert!(toml_str.contains("pane_kind = \"editor\""), "{toml_str}");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        let panes = loaded.workspaces[0].panes();
        let find = |id: Uuid| *panes.iter().find(|p| p.id == id).unwrap();
        assert_eq!(find(split.id).pane_kind, PaneKind::Editor);
        assert_eq!(find(split.id).file_path, "D:\\작업\\src\\lib.rs");
        assert_eq!(find(in_tabs.id).pane_kind, PaneKind::Editor);
        assert_eq!(find(in_tabs.id).file_path, "/home/me/notes.md");
        assert_eq!(find(untitled.id).file_path, "");
    }

    /// A config written before `file_path` existed loads with it empty.
    #[test]
    fn file_path_defaults_to_empty_for_old_configs() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let stripped: String = toml_str
            .lines()
            .filter(|l| !l.trim_start().starts_with("file_path"))
            .map(|l| format!("{l}\n"))
            .collect();
        let loaded: Config = toml::from_str(&stripped).expect("deserialize");
        assert_eq!(loaded.workspaces[0].panes()[0].file_path, "");
    }

    /// A git pane keeps its kind and its repository directory (`cwd`)
    /// through TOML, nested in a split and in a tab group — the two
    /// tagged-enum shapes rule 3 warns about. It needs no field of its own
    /// (spec §0.2): the repository is found from `cwd`.
    #[test]
    fn git_pane_kind_and_cwd_roundtrip_nested() {
        let git = PaneSpec::new_git(Some("D:\\작업\\ymux".into()));
        let in_tabs = PaneSpec::new_git(None);
        let mut config = Config::default();
        config.workspaces[0].root = LayoutNode::Split {
            direction: SplitDir::Vertical,
            ratio: 0.4,
            a: Box::new(LayoutNode::Pane(git.clone())),
            b: Box::new(LayoutNode::Tabs {
                id: Uuid::new_v4(),
                active: 1,
                children: vec![
                    LayoutNode::Pane(PaneSpec::new_default()),
                    LayoutNode::Pane(in_tabs.clone()),
                ],
            }),
        };
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        assert!(toml_str.contains("pane_kind = \"git\""), "{toml_str}");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        let panes = loaded.workspaces[0].panes();
        let a = panes.iter().find(|p| p.id == git.id).unwrap();
        let b = panes.iter().find(|p| p.id == in_tabs.id).unwrap();
        assert_eq!(a.pane_kind, PaneKind::Git);
        assert_eq!(a.cwd.as_deref(), Some("D:\\작업\\ymux"));
        assert_eq!(b.pane_kind, PaneKind::Git);
        assert_eq!(b.cwd, None);
    }

    /// A files pane keeps its kind and its directory (`cwd`) through TOML,
    /// nested in a split *and* in a tab group: the two tagged-enum shapes
    /// rule 3 warns about. The directory is the files pane's only state.
    #[test]
    fn files_pane_kind_and_cwd_roundtrip_nested() {
        let files = PaneSpec::new_files(Some("D:\\작업\\src".into()));
        let in_tabs = PaneSpec::new_files(Some("/home/me".into()));
        let mut config = Config::default();
        config.workspaces[0].root = LayoutNode::Split {
            direction: SplitDir::Horizontal,
            ratio: 0.5,
            a: Box::new(LayoutNode::Pane(files.clone())),
            b: Box::new(LayoutNode::Tabs {
                id: Uuid::new_v4(),
                active: 0,
                children: vec![
                    LayoutNode::Pane(in_tabs.clone()),
                    LayoutNode::Pane(PaneSpec::new_default()),
                ],
            }),
        };
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        assert!(toml_str.contains("pane_kind = \"files\""), "{toml_str}");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        let panes = loaded.workspaces[0].panes();
        let a = panes.iter().find(|p| p.id == files.id).unwrap();
        let b = panes.iter().find(|p| p.id == in_tabs.id).unwrap();
        assert_eq!(a.pane_kind, PaneKind::Files);
        assert_eq!(a.cwd.as_deref(), Some("D:\\작업\\src"));
        assert_eq!(b.pane_kind, PaneKind::Files);
        assert_eq!(b.cwd.as_deref(), Some("/home/me"));
    }

    /// `cwd` and `title` must survive the round-trip for a pane nested inside
    /// a split, not just for a lone root pane.
    ///
    /// This is the shape that actually matters — the moment you split, every
    /// pane lives under `LayoutNode::Split`, and `LayoutNode` is a
    /// `#[serde(tag = "kind")]` enum, which is the construct the TOML caveat
    /// in CLAUDE.md warns about for `Option<T>`. "Reopen where you left off"
    /// is exactly `cwd` making it back out of this file.
    #[test]
    fn nested_pane_cwd_and_title_roundtrip() {
        let mut a = PaneSpec::new_default();
        a.cwd = Some("/Users/alice/projects".into());
        a.title = Some("build".into());
        let mut b = PaneSpec::new_default();
        b.cwd = Some("/tmp".into());

        let mut config = Config::default();
        config.workspaces[0].root = LayoutNode::Split {
            direction: SplitDir::Horizontal,
            ratio: 0.5,
            a: Box::new(LayoutNode::Pane(a.clone())),
            b: Box::new(LayoutNode::Split {
                direction: SplitDir::Vertical,
                ratio: 0.5,
                a: Box::new(LayoutNode::Pane(b.clone())),
                b: Box::new(LayoutNode::Pane(PaneSpec::new_default())),
            }),
        };

        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        let panes = loaded.workspaces[0].panes();
        assert_eq!(panes.len(), 3);
        assert_eq!(panes[0].cwd.as_deref(), Some("/Users/alice/projects"));
        assert_eq!(panes[0].title.as_deref(), Some("build"));
        assert_eq!(panes[1].cwd.as_deref(), Some("/tmp"));
        assert_eq!(panes[2].cwd, None);
    }

    /// Regression: worktree_path must survive a full Config → TOML → Config
    /// round-trip, mirroring the bg_color coverage above.
    #[test]
    fn panespec_worktree_field_roundtrip() {
        let mut config = Config::default();
        let ws = config.workspace_mut(1);
        if let LayoutNode::Pane(ref mut p) = ws.root {
            p.worktree_path = "C:\\wt\\agent-1".to_string();
        }
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        // The toml crate's pretty printer emits TOML literal strings (single
        // quotes) for values containing backslashes, so no escaping needed.
        assert!(toml_str.contains("worktree_path = 'C:\\wt\\agent-1'"));
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(
            loaded.workspaces[0].panes()[0].worktree_path,
            "C:\\wt\\agent-1"
        );
    }

    /// Empty bg_color (default) must round-trip as empty string, not disappear.
    #[test]
    fn empty_bg_color_roundtrip() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        let pane = loaded.workspaces[0].panes()[0];
        assert_eq!(pane.bg_color, "", "empty bg_color must stay empty");
    }

    /// Empty worktree_path (default) must round-trip as empty string, not disappear.
    #[test]
    fn empty_worktree_path_roundtrip() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        let pane = loaded.workspaces[0].panes()[0];
        assert_eq!(
            pane.worktree_path, "",
            "empty worktree_path must stay empty"
        );
    }

    /// Split layout with mixed bg_color values must preserve each pane's color.
    #[test]
    fn split_layout_bg_color_roundtrip() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let config = Config {
            version: CONFIG_VERSION,
            active_workspace: 1,
            shells: vec![],
            workspaces: vec![Workspace {
                id: 1,
                name: "main".into(),
                root: LayoutNode::Split {
                    direction: SplitDir::Horizontal,
                    ratio: 0.5,
                    a: Box::new(LayoutNode::Pane(PaneSpec {
                        id: a,
                        bg_color: "#ff0000".to_string(),
                        ..PaneSpec::new_default()
                    })),
                    b: Box::new(LayoutNode::Pane(PaneSpec {
                        id: b,
                        bg_color: String::new(),
                        ..PaneSpec::new_default()
                    })),
                },
            }],
            notify_on_bell: true,
            persist_scrollback: true,
            bottom_anchor: true,
            paste_image_retention_hours: 24,
            worktree_base_dir: String::new(),
            font_size: 13,
            default_shell: String::new(),
            agent_tracking: false,
            agent_hook_port: 0,
        };
        let toml_str = toml::to_string_pretty(&config).expect("serialize");
        let loaded: Config = toml::from_str(&toml_str).expect("deserialize");
        let panes = loaded.workspaces[0].panes();
        let pa = panes.iter().find(|p| p.id == a).unwrap();
        let pb = panes.iter().find(|p| p.id == b).unwrap();
        assert_eq!(pa.bg_color, "#ff0000");
        assert_eq!(pb.bg_color, "");
    }

    fn tab_pane(title: &str) -> LayoutNode {
        let mut p = PaneSpec::new_default();
        p.title = Some(title.to_string());
        p.shell = "pwsh".into();
        p.cwd = Some("C:/work".into());
        LayoutNode::Pane(p)
    }

    #[test]
    fn tabs_node_roundtrips_through_toml() {
        let mut cfg = Config::default();
        let group = Uuid::new_v4();
        cfg.workspace_mut(1).root = LayoutNode::Tabs {
            id: group,
            active: 1,
            children: vec![tab_pane("first"), tab_pane("second"), tab_pane("third")],
        };
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&text).expect("deserialize");
        match &back.workspaces[0].root {
            LayoutNode::Tabs {
                id,
                active,
                children,
            } => {
                assert_eq!(*id, group, "the group id survives a round trip");
                assert_eq!(*active, 1, "the active index survives a round trip");
                assert_eq!(children.len(), 3);
                let titles: Vec<&str> = children
                    .iter()
                    .map(|c| match c {
                        LayoutNode::Pane(p) => p.title.as_deref().unwrap_or(""),
                        _ => panic!("child is not a pane"),
                    })
                    .collect();
                assert_eq!(titles, vec!["first", "second", "third"]);
            }
            other => panic!("expected a tabs node, got {other:?}"),
        }
    }

    #[test]
    fn tabs_node_roundtrips_nested_in_a_split() {
        let mut cfg = Config::default();
        cfg.workspace_mut(1).root = LayoutNode::Split {
            direction: SplitDir::Horizontal,
            ratio: 0.5,
            a: Box::new(LayoutNode::Tabs {
                id: Uuid::new_v4(),
                active: 0,
                children: vec![tab_pane("t1"), tab_pane("t2")],
            }),
            b: Box::new(tab_pane("plain")),
        };
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&text).expect("deserialize");
        assert_eq!(back.workspaces[0].panes().len(), 3, "every tab is a pane");
    }

    #[test]
    fn a_tabs_node_written_without_an_id_gets_one() {
        // Old hand-written configs (and anything produced before the field
        // existed) must still load; the fresh id is fine because the only
        // consumer, the viewer-tab registry, is runtime-only.
        let text = r#"
version = 7
active_workspace = 1

[[workspaces]]
id = 1
name = "main"

[workspaces.root]
kind = "tabs"
active = 0

[[workspaces.root.children]]
kind = "pane"
id = "11111111-1111-4111-8111-111111111111"
shell = "pwsh"

[[workspaces.root.children]]
kind = "pane"
id = "22222222-2222-4222-8222-222222222222"
shell = "cmd"
"#;
        let cfg: Config = toml::from_str(text).expect("deserialize");
        match &cfg.workspaces[0].root {
            LayoutNode::Tabs { id, children, .. } => {
                assert!(!id.is_nil(), "a missing id is filled in, not left nil");
                assert_eq!(children.len(), 2);
            }
            other => panic!("expected a tabs node, got {other:?}"),
        }
    }

    #[test]
    fn removing_a_tab_keeps_the_others() {
        let mut node = LayoutNode::Tabs {
            id: Uuid::new_v4(),
            active: 2,
            children: vec![tab_pane("a"), tab_pane("b"), tab_pane("c")],
        };
        let victim = match &node {
            LayoutNode::Tabs { children, .. } => match &children[0] {
                LayoutNode::Pane(p) => p.id,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        assert_eq!(node.remove_pane(victim), RemoveResult::Removed);
        match &node {
            LayoutNode::Tabs {
                active, children, ..
            } => {
                assert_eq!(children.len(), 2);
                assert!(*active < children.len(), "active stays in range");
            }
            other => panic!("expected a tabs node, got {other:?}"),
        }
    }

    // --- pin_pane_shells -------------------------------------------------

    const MAC_SHELLS: [&str; 3] = ["zsh", "bash", "fish"];
    const WIN_SHELLS: [&str; 3] = ["PowerShell 7", "Windows PowerShell", "Command Prompt"];

    fn profiles(names: &[&str]) -> Vec<ShellProfile> {
        names
            .iter()
            .map(|n| ShellProfile {
                name: (*n).to_string(),
                executable: format!("/bin/{n}"),
                args: Vec::new(),
                icon: None,
                color: None,
                env: Vec::new(),
            })
            .collect()
    }

    fn terminal(shell: &str) -> PaneSpec {
        PaneSpec {
            shell: shell.to_string(),
            ..PaneSpec::new_default()
        }
    }

    /// A workspace with empty-shell terminals at the root of a split and
    /// inside a tab group, next to non-terminal panes that also carry "".
    fn mixed_config(names: &[&str]) -> Config {
        let mut cfg = Config {
            shells: profiles(names),
            ..Config::default()
        };
        cfg.workspace_mut(1).root = LayoutNode::Split {
            direction: SplitDir::Horizontal,
            ratio: 0.5,
            a: Box::new(LayoutNode::Pane(terminal(""))),
            b: Box::new(LayoutNode::Tabs {
                id: Uuid::new_v4(),
                active: 0,
                children: vec![
                    LayoutNode::Pane(terminal("")),
                    LayoutNode::Pane(PaneSpec::new_browser("https://example.com")),
                    LayoutNode::Pane(PaneSpec::new_editor("/tmp/x.txt")),
                    LayoutNode::Pane(PaneSpec::new_files(None)),
                    LayoutNode::Pane(terminal("gone-shell")),
                ],
            }),
        };
        cfg
    }

    fn shells_by_kind(cfg: &Config) -> Vec<(PaneKind, String)> {
        cfg.workspaces
            .iter()
            .flat_map(|w| w.panes())
            .map(|p| (p.pane_kind, p.shell.clone()))
            .collect()
    }

    #[test]
    fn pin_fills_only_empty_terminal_panes() {
        for names in [&MAC_SHELLS[..], &WIN_SHELLS[..]] {
            let mut cfg = mixed_config(names);
            assert!(cfg.pin_pane_shells(), "{names:?}: first call pins");
            let first = names[0].to_string();
            let got = shells_by_kind(&cfg);
            assert!(got.contains(&(PaneKind::Browser, String::new())));
            assert!(got.contains(&(PaneKind::Editor, String::new())));
            assert!(got.contains(&(PaneKind::Files, String::new())));
            assert!(got.contains(&(PaneKind::Terminal, "gone-shell".to_string())));
            let pinned = got
                .iter()
                .filter(|(k, s)| *k == PaneKind::Terminal && *s == first)
                .count();
            assert_eq!(pinned, 2, "{names:?}: both empty terminals pinned");
            assert!(
                !got.iter()
                    .any(|(k, s)| *k == PaneKind::Terminal && s.is_empty()),
                "{names:?}: no empty terminal left"
            );
            assert!(!cfg.pin_pane_shells(), "{names:?}: second call is a no-op");
        }
    }

    #[test]
    fn pin_prefers_existing_default_shell_else_first() {
        for names in [&MAC_SHELLS[..], &WIN_SHELLS[..]] {
            let mut cfg = mixed_config(names);
            cfg.default_shell = names[1].to_string();
            assert_eq!(cfg.resolved_default_shell(), Some(names[1]));
            cfg.pin_pane_shells();
            assert_eq!(cfg.workspaces[0].panes()[0].shell, names[1]);

            let mut cfg = mixed_config(names);
            cfg.default_shell = "not-installed".to_string();
            assert_eq!(cfg.resolved_default_shell(), Some(names[0]));
            cfg.pin_pane_shells();
            assert_eq!(cfg.workspaces[0].panes()[0].shell, names[0]);
        }
    }

    #[test]
    fn pinned_shell_survives_default_change_and_toml_roundtrip() {
        for names in [&MAC_SHELLS[..], &WIN_SHELLS[..]] {
            let mut cfg = mixed_config(names);
            cfg.default_shell = names[0].to_string();
            cfg.pin_pane_shells();
            // The user later picks a different default shell.
            cfg.default_shell = names[1].to_string();
            let toml_str = toml::to_string_pretty(&cfg).unwrap();
            let mut loaded: Config = toml::from_str(&toml_str).unwrap();
            assert!(!loaded.pin_pane_shells(), "{names:?}: nothing left to pin");
            let panes = loaded.workspaces[0].panes();
            assert_eq!(panes[0].shell, names[0], "{names:?}");
            assert_eq!(panes[1].shell, names[0], "{names:?}");
        }
    }

    /// v9 exists to re-detect the zsh profile (whose cached
    /// `YMUX_USER_ZDOTDIR` changed meaning), so migrating a v8 config must
    /// drop the shell cache — but leave every pane's pinned shell name alone,
    /// so the names still resolve once detection repopulates the cache.
    #[test]
    fn migrate_v8_to_v9_clears_shells() {
        assert_eq!(CONFIG_VERSION, 9);
        for names in [&MAC_SHELLS[..], &WIN_SHELLS[..]] {
            let mut cfg = mixed_config(names);
            cfg.pin_pane_shells();
            cfg.version = 8;
            let before = shells_by_kind(&cfg);

            cfg.migrate();
            assert_eq!(cfg.version, 9, "{names:?}");
            assert!(cfg.shells.is_empty(), "{names:?}: v8 shells cleared");
            assert_eq!(shells_by_kind(&cfg), before, "{names:?}: panes untouched");

            // Re-detection produces the same profile names.
            cfg.shells = profiles(names);
            for (kind, shell) in shells_by_kind(&cfg) {
                if kind == PaneKind::Terminal && shell != "gone-shell" {
                    assert!(cfg.shell(&shell).is_some(), "{names:?}: {shell:?}");
                }
            }
            assert!(!cfg.pin_pane_shells(), "{names:?}: nothing left to pin");
        }
    }

    #[test]
    fn pin_is_noop_without_shells() {
        for names in [&MAC_SHELLS[..], &WIN_SHELLS[..]] {
            let mut cfg = mixed_config(names);
            cfg.default_shell = names[0].to_string();
            cfg.shells.clear();
            assert_eq!(cfg.resolved_default_shell(), None);
            assert!(!cfg.pin_pane_shells());
            assert_eq!(cfg.workspaces[0].panes()[0].shell, "");
        }
    }
}
