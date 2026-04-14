//! Serde data model for ymux's on-disk config.
//!
//! The entire user-visible state — workspaces, layouts, panes, and the cached
//! list of detected shell profiles — lives in a single [`Config`] that
//! round-trips through TOML.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current on-disk schema version. Bump when a migration is needed.
pub const CONFIG_VERSION: u32 = 1;

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
}
