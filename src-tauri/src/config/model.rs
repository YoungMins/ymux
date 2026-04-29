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
pub const CONFIG_VERSION: u32 = 5;

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
}

fn default_version() -> u32 {
    CONFIG_VERSION
}
fn default_active_workspace() -> u32 {
    1
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            active_workspace: 1,
            shells: Vec::new(),
            workspaces: vec![Workspace::empty(1, "main")],
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

    /// Apply layout / workspace updates from `incoming` onto `self`, treating
    /// `shells` as a backend-owned detection cache: it is only overwritten
    /// when the incoming config carries a non-empty list. This stops a stale
    /// frontend snapshot (e.g. one captured before shell detection finished)
    /// from clobbering the cache and breaking subsequent `spawn_pane` calls.
    pub fn merge_layouts_from(&mut self, incoming: Config) {
        self.version = incoming.version;
        self.active_workspace = incoming.active_workspace;
        self.workspaces = incoming.workspaces;
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
        active: usize,
        children: Vec<LayoutNode>,
    },
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
            LayoutNode::Tabs { children, active } => {
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
    /// Reference to a [`ShellProfile::name`]. The empty string means "use the
    /// first detected shell".
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
    /// Custom background color for this pane (hex string like "#1a2b3c").
    #[serde(default)]
    pub bg_color: Option<String>,
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
            bg_color: None,
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
            bg_color: None,
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
            bg_color: None,
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
            bg_color: None,
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
            }],
            workspaces: vec![Workspace::empty(1, "main")],
        };
        let frontend_save = Config {
            version: CONFIG_VERSION,
            active_workspace: 2,
            shells: vec![],
            workspaces: vec![Workspace::empty(2, "two")],
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
                        bg_color: None,
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
                        bg_color: None,
                    })),
                },
            }],
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
            }],
            workspaces: vec![Workspace::empty(1, "main")],
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
            }],
            workspaces: vec![Workspace::empty(1, "main")],
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
                },
                ShellProfile {
                    name: "new-b".into(),
                    executable: "/b".into(),
                    args: vec![],
                    icon: None,
                    color: None,
                },
            ],
            workspaces: vec![Workspace::empty(1, "main")],
        };
        backend.merge_layouts_from(frontend_save);
        assert_eq!(backend.shells.len(), 2);
        assert_eq!(backend.shells[0].name, "new-a");
        assert_eq!(backend.shells[1].name, "new-b");
    }
}
