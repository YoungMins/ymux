//! Agent registry: which coding agents (Claude Code, Codex, …) run in which
//! pane, and what they are doing. A pure state machine — no Tauri, no IO — so
//! it is unit-tested on Linux. Two feeds:
//! - Claude Code http hooks, received by `hook_http`
//!   ([`AgentRegistry::apply_hook`]): precise, and the only source of
//!   subagents.
//! - The 2 s process scan in `agent_scan` ([`AgentRegistry::apply_scan`]):
//!   covers every CLI and decides liveness.

use std::collections::{BTreeMap, HashMap, HashSet};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Working,
    Waiting,
    Done,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSource {
    Hook,
    Process,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Agent {
    pub kind: String,
    pub status: AgentStatus,
    pub source: AgentSource,
    pub tool: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Subagent {
    pub id: String,
    pub agent_type: String,
    pub status: AgentStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PaneAgents {
    pub lead: Option<Agent>,
    pub subagents: Vec<Subagent>,
}

/// What the frontend sees: pane id → agents. Panes with none are absent.
pub type AgentSnapshot = BTreeMap<Uuid, PaneAgents>;

/// One hook invocation, as `hook_http::hook_event` packs it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HookEvent {
    pub pane_id: Uuid,
    pub agent: String,
    pub event: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
}

impl HookEvent {
    /// `None` for anything that isn't a well-formed hook payload (including a
    /// `pane_id` that isn't a UUID).
    pub fn from_payload(payload: &serde_json::Value) -> Option<Self> {
        serde_json::from_value(payload.clone()).ok()
    }
}

#[derive(Debug, Default)]
struct PaneState {
    agents: PaneAgents,
    /// The scan has seen an agent process here, so its disappearance means
    /// the agent exited.
    process_seen: bool,
    /// A hook reported `SessionEnd`: don't resurrect a process lead from a
    /// CLI that is still shutting down. Cleared by the next hook event.
    ended: bool,
    /// The agent's own session id, as the last hook for this pane reported
    /// it. Empty until a hook arrives — the process scan cannot know it, and
    /// `crate::agent_sessions` falls back to reading the CLI's transcripts.
    session_id: String,
}

#[derive(Debug, Default)]
pub struct AgentRegistry {
    panes: HashMap<Uuid, PaneState>,
}

/// Tauri-managed handle: `app.manage(SharedAgents::default())`.
#[derive(Default)]
pub struct SharedAgents(pub Mutex<AgentRegistry>);

fn set_lead(st: &mut PaneState, kind: &str, status: AgentStatus, tool: Option<String>) {
    st.agents.lead = Some(Agent {
        kind: kind.to_string(),
        status,
        source: AgentSource::Hook,
        tool,
    });
}

fn upsert_subagent(st: &mut PaneState, id: &str, agent_type: Option<&str>, status: AgentStatus) {
    let agent_type = agent_type.filter(|t| !t.is_empty());
    match st.agents.subagents.iter_mut().find(|s| s.id == id) {
        Some(s) => {
            s.status = status;
            if let Some(t) = agent_type {
                s.agent_type = t.to_string();
            }
        }
        None => st.agents.subagents.push(Subagent {
            id: id.to_string(),
            agent_type: agent_type.unwrap_or_default().to_string(),
            status,
        }),
    }
}

impl AgentRegistry {
    pub fn snapshot(&self) -> AgentSnapshot {
        self.panes
            .iter()
            .filter(|(_, s)| s.agents.lead.is_some() || !s.agents.subagents.is_empty())
            .map(|(id, s)| (*id, s.agents.clone()))
            .collect()
    }

    /// Apply one Claude Code hook event (spec §1 mapping table). Returns
    /// whether the snapshot changed.
    pub fn apply_hook(&mut self, ev: &HookEvent) -> bool {
        let before = self.snapshot();
        let sub_id = ev.agent_id.as_deref().filter(|s| !s.is_empty());
        let st = self.panes.entry(ev.pane_id).or_default();
        // Any hook other than SessionEnd means a live session again.
        st.ended = ev.event == "SessionEnd";
        // Every hook carries the id; keep the newest non-empty one. A
        // `/clear` or a fresh `claude` in the same pane arrives as a new
        // `SessionStart` with a new id, and overwriting is exactly right —
        // the old conversation is no longer what is on screen.
        if !ev.session_id.is_empty() {
            st.session_id = ev.session_id.clone();
        }
        match ev.event.as_str() {
            "SessionStart" => {
                st.agents.subagents.clear();
                set_lead(st, &ev.agent, AgentStatus::Idle, None);
            }
            "UserPromptSubmit" => {
                set_lead(st, &ev.agent, AgentStatus::Working, None);
                // A new turn: every subagent of the previous one is gone.
                // Clear them all, not just the Done ones — an Esc interrupt
                // fires no Stop / SubagentStop, so interrupted subagents
                // would otherwise sit at "working" forever.
                st.agents.subagents.clear();
            }
            "PreToolUse" | "PostToolUse" => match sub_id {
                Some(id) => upsert_subagent(st, id, ev.agent_type.as_deref(), AgentStatus::Working),
                None => set_lead(st, &ev.agent, AgentStatus::Working, ev.tool_name.clone()),
            },
            "PermissionRequest" => {
                set_lead(st, &ev.agent, AgentStatus::Waiting, ev.tool_name.clone())
            }
            "Stop" | "StopFailure" | "PostCompact" => {
                set_lead(st, &ev.agent, AgentStatus::Done, None)
            }
            "SessionEnd" => st.agents = PaneAgents::default(),
            "SubagentStart" => {
                if let Some(id) = sub_id {
                    upsert_subagent(st, id, ev.agent_type.as_deref(), AgentStatus::Working);
                }
            }
            "SubagentStop" => {
                if let Some(s) =
                    sub_id.and_then(|id| st.agents.subagents.iter_mut().find(|s| s.id == id))
                {
                    s.status = AgentStatus::Done;
                }
            }
            _ => {}
        }
        self.snapshot() != before
    }

    /// The session id the Claude Code hooks reported for `pane_id`, if any.
    /// Exact, unlike the transcript scan, so it wins wherever both exist.
    pub fn hook_session_id(&self, pane_id: Uuid) -> Option<&str> {
        self.panes
            .get(&pane_id)
            .map(|s| s.session_id.as_str())
            .filter(|s| !s.is_empty())
    }

    /// Panes where the agent process is gone but the pane itself is still
    /// open — the user quit the agent rather than closing the terminal.
    ///
    /// Same condition `apply_scan`'s `retain` uses, exposed separately so the
    /// caller can act on it *before* the state is dropped. Deliberately only
    /// panes still in `live`: at app shutdown every PTY dies at once, and
    /// reading that as "the user quit the agent" would deactivate exactly the
    /// records the next launch needs.
    pub fn exited_panes(&self, live: &HashSet<Uuid>, found: &HashMap<Uuid, String>) -> Vec<Uuid> {
        self.panes
            .iter()
            .filter(|(id, st)| st.process_seen && live.contains(id) && !found.contains_key(id))
            .map(|(id, _)| *id)
            .collect()
    }

    /// Apply one process scan. `live`: every pane with a running PTY.
    /// `found`: panes where an agent process was seen, with its kind.
    /// Returns whether the snapshot changed.
    pub fn apply_scan(&mut self, live: &HashSet<Uuid>, found: &HashMap<Uuid, String>) -> bool {
        let before = self.snapshot();
        // Closed panes go; so does any pane whose previously seen agent
        // process has exited (hook-fed and process-fed alike).
        self.panes
            .retain(|id, st| live.contains(id) && (found.contains_key(id) || !st.process_seen));
        // Anything dropped here for lack of a process took its hook-borne
        // session id with it; the store keeps its own copy (see
        // `SessionTracker::note_agent_exit`), so nothing is lost.
        for (id, kind) in found {
            if !live.contains(id) {
                continue;
            }
            let st = self.panes.entry(*id).or_default();
            st.process_seen = true;
            if st.ended {
                continue;
            }
            match &mut st.agents.lead {
                None => {
                    st.agents.lead = Some(Agent {
                        kind: kind.clone(),
                        // The frontend derives a process lead's status from
                        // the pane's own PaneStatusMachine.
                        status: AgentStatus::Idle,
                        source: AgentSource::Process,
                        tool: None,
                    })
                }
                Some(lead) if lead.source == AgentSource::Process => lead.kind = kind.clone(),
                Some(_) => {}
            }
        }
        self.snapshot() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(pane: Uuid, event: &str) -> HookEvent {
        HookEvent {
            pane_id: pane,
            agent: "claude".into(),
            event: event.into(),
            session_id: "s1".into(),
            agent_id: None,
            agent_type: None,
            tool_name: None,
        }
    }

    fn tool_ev(pane: Uuid, event: &str, tool: &str) -> HookEvent {
        HookEvent {
            tool_name: Some(tool.into()),
            ..ev(pane, event)
        }
    }

    fn sub_ev(pane: Uuid, event: &str, id: &str, agent_type: &str) -> HookEvent {
        HookEvent {
            agent_id: Some(id.into()),
            agent_type: Some(agent_type.into()),
            ..ev(pane, event)
        }
    }

    fn lead(reg: &AgentRegistry, pane: Uuid) -> Agent {
        reg.snapshot()[&pane].lead.clone().expect("lead")
    }

    fn set(ids: &[Uuid]) -> HashSet<Uuid> {
        ids.iter().copied().collect()
    }

    fn found(pairs: &[(Uuid, &str)]) -> HashMap<Uuid, String> {
        pairs.iter().map(|(id, k)| (*id, k.to_string())).collect()
    }

    #[test]
    fn session_start_creates_an_idle_hook_lead() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        assert!(reg.apply_hook(&ev(p, "SessionStart")));
        let l = lead(&reg, p);
        assert_eq!(l.kind, "claude");
        assert_eq!(l.status, AgentStatus::Idle);
        assert_eq!(l.source, AgentSource::Hook);
        assert_eq!(l.tool, None);
    }

    #[test]
    fn user_prompt_submit_sets_working_and_drops_all_subagents() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Explore"));
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a2", "executor"));
        reg.apply_hook(&sub_ev(p, "SubagentStop", "a1", "Explore"));
        reg.apply_hook(&ev(p, "UserPromptSubmit"));
        let snap = reg.snapshot();
        assert_eq!(snap[&p].lead.as_ref().unwrap().status, AgentStatus::Working);
        assert!(snap[&p].subagents.is_empty());
    }

    #[test]
    fn prompt_after_an_interrupt_clears_subagents_left_working() {
        // Esc fires neither Stop nor SubagentStop, so an interrupted turn
        // leaves its subagents "working". A new prompt starts a new turn:
        // none of the previous turn's subagents can still be running.
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "UserPromptSubmit"));
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Explore"));
        reg.apply_hook(&sub_ev(p, "PreToolUse", "a1", "Explore"));
        // <Esc> — no hook at all.
        reg.apply_hook(&ev(p, "UserPromptSubmit"));
        let snap = reg.snapshot();
        assert!(snap[&p].subagents.is_empty());
        assert_eq!(snap[&p].lead.as_ref().unwrap().status, AgentStatus::Working);
    }

    #[test]
    fn tool_use_without_agent_id_sets_lead_working_with_tool() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&tool_ev(p, "PreToolUse", "Bash"));
        let l = lead(&reg, p);
        assert_eq!(l.status, AgentStatus::Working);
        assert_eq!(l.tool.as_deref(), Some("Bash"));
        reg.apply_hook(&tool_ev(p, "PostToolUse", "Edit"));
        assert_eq!(lead(&reg, p).tool.as_deref(), Some("Edit"));
    }

    #[test]
    fn tool_use_with_agent_id_marks_only_that_subagent() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "Stop"));
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Explore"));
        reg.apply_hook(&sub_ev(p, "SubagentStop", "a1", "Explore"));
        let mut e = sub_ev(p, "PreToolUse", "a1", "Explore");
        e.tool_name = Some("Grep".into());
        reg.apply_hook(&e);
        let snap = reg.snapshot();
        assert_eq!(snap[&p].subagents[0].status, AgentStatus::Working);
        assert_eq!(snap[&p].lead.as_ref().unwrap().status, AgentStatus::Done);
    }

    #[test]
    fn tool_use_for_an_unknown_subagent_adds_it() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&sub_ev(p, "PostToolUse", "zz", "Plan"));
        let s = &reg.snapshot()[&p].subagents[0];
        assert_eq!((s.id.as_str(), s.agent_type.as_str()), ("zz", "Plan"));
        assert_eq!(s.status, AgentStatus::Working);
    }

    #[test]
    fn permission_request_sets_waiting() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "SessionStart"));
        reg.apply_hook(&tool_ev(p, "PermissionRequest", "Bash"));
        assert_eq!(lead(&reg, p).status, AgentStatus::Waiting);
    }

    #[test]
    fn stop_family_sets_done() {
        for event in ["Stop", "StopFailure", "PostCompact"] {
            let p = Uuid::new_v4();
            let mut reg = AgentRegistry::default();
            reg.apply_hook(&tool_ev(p, "PreToolUse", "Bash"));
            reg.apply_hook(&ev(p, event));
            let l = lead(&reg, p);
            assert_eq!(l.status, AgentStatus::Done, "{event}");
            assert_eq!(l.tool, None, "{event}");
        }
    }

    #[test]
    fn session_end_removes_lead_and_subagents() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "SessionStart"));
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Explore"));
        assert!(reg.apply_hook(&ev(p, "SessionEnd")));
        assert!(!reg.snapshot().contains_key(&p));
    }

    #[test]
    fn subagent_start_replaces_and_stop_marks_done() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Explore"));
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Plan"));
        let snap = reg.snapshot();
        assert_eq!(snap[&p].subagents.len(), 1);
        assert_eq!(snap[&p].subagents[0].agent_type, "Plan");
        reg.apply_hook(&sub_ev(p, "SubagentStop", "a1", "Plan"));
        assert_eq!(reg.snapshot()[&p].subagents[0].status, AgentStatus::Done);
    }

    #[test]
    fn unknown_event_changes_nothing() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        assert!(!reg.apply_hook(&ev(p, "Notification")));
        assert!(reg.snapshot().is_empty());
    }

    #[test]
    fn scan_adds_a_process_lead_when_there_is_no_hook_data() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        assert!(reg.apply_scan(&set(&[p]), &found(&[(p, "codex")])));
        let l = lead(&reg, p);
        assert_eq!((l.kind.as_str(), l.source), ("codex", AgentSource::Process));
        assert_eq!(l.status, AgentStatus::Idle);
        // Unchanged scan → no change reported (no redundant emit).
        assert!(!reg.apply_scan(&set(&[p]), &found(&[(p, "codex")])));
    }

    #[test]
    fn scan_keeps_a_hook_lead_as_is() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&tool_ev(p, "PreToolUse", "Bash"));
        reg.apply_scan(&set(&[p]), &found(&[(p, "claude")]));
        let l = lead(&reg, p);
        assert_eq!(l.source, AgentSource::Hook);
        assert_eq!(l.status, AgentStatus::Working);
    }

    #[test]
    fn scan_removes_the_entry_once_the_agent_process_is_gone() {
        let hook_pane = Uuid::new_v4();
        let proc_pane = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(hook_pane, "SessionStart"));
        let live = set(&[hook_pane, proc_pane]);
        reg.apply_scan(
            &live,
            &found(&[(hook_pane, "claude"), (proc_pane, "aider")]),
        );
        assert!(reg.apply_scan(&live, &HashMap::new()));
        assert!(reg.snapshot().is_empty());
    }

    #[test]
    fn scan_keeps_a_hook_entry_whose_process_was_never_detected() {
        // Detection can miss an unusual install; hook data alone must not
        // flicker away every 2 s. Only a *previously seen* process "going
        // away" removes the entry.
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "SessionStart"));
        assert!(!reg.apply_scan(&set(&[p]), &HashMap::new()));
        assert!(reg.snapshot().contains_key(&p));
    }

    #[test]
    fn scan_drops_closed_panes() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "SessionStart"));
        assert!(reg.apply_scan(&HashSet::new(), &HashMap::new()));
        assert!(reg.snapshot().is_empty());
    }

    #[test]
    fn session_end_is_not_resurrected_by_a_lingering_process() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&ev(p, "SessionStart"));
        reg.apply_scan(&set(&[p]), &found(&[(p, "claude")]));
        reg.apply_hook(&ev(p, "SessionEnd"));
        // The CLI is still shutting down when the next scan runs.
        reg.apply_scan(&set(&[p]), &found(&[(p, "claude")]));
        assert!(!reg.snapshot().contains_key(&p));
        // A new session in the same pane shows up again.
        reg.apply_hook(&ev(p, "SessionStart"));
        assert!(reg.snapshot().contains_key(&p));
    }

    #[test]
    fn from_payload_parses_and_rejects() {
        let p = Uuid::new_v4();
        let ok = serde_json::json!({
            "pane_id": p.to_string(), "agent": "claude", "event": "PreToolUse",
            "session_id": "s", "tool_name": "Bash"
        });
        let parsed = HookEvent::from_payload(&ok).expect("parse");
        assert_eq!(parsed.pane_id, p);
        assert_eq!(parsed.tool_name.as_deref(), Some("Bash"));
        assert_eq!(parsed.agent_id, None);
        let bad =
            serde_json::json!({ "pane_id": "not-a-uuid", "agent": "claude", "event": "Stop" });
        assert!(HookEvent::from_payload(&bad).is_none());
    }

    #[test]
    fn snapshot_serializes_to_the_frontend_shape() {
        let p = Uuid::new_v4();
        let mut reg = AgentRegistry::default();
        reg.apply_hook(&tool_ev(p, "PreToolUse", "Bash"));
        reg.apply_hook(&sub_ev(p, "SubagentStart", "a1", "Explore"));
        let v = serde_json::to_value(reg.snapshot()).expect("json");
        assert_eq!(
            v[p.to_string()],
            serde_json::json!({
                "lead": { "kind": "claude", "status": "working", "source": "hook", "tool": "Bash" },
                "subagents": [{ "id": "a1", "agent_type": "Explore", "status": "working" }]
            })
        );
    }
}

#[cfg(test)]
mod session_id_tests {
    use super::*;

    fn hook(pane: Uuid, event: &str, session_id: &str) -> HookEvent {
        HookEvent {
            pane_id: pane,
            agent: "claude".into(),
            event: event.into(),
            session_id: session_id.into(),
            agent_id: None,
            agent_type: None,
            tool_name: None,
        }
    }

    #[test]
    fn hooks_carry_the_session_id_through() {
        let pane = Uuid::from_u128(1);
        let mut reg = AgentRegistry::default();
        assert_eq!(reg.hook_session_id(pane), None);
        reg.apply_hook(&hook(pane, "SessionStart", "sess-one"));
        assert_eq!(reg.hook_session_id(pane), Some("sess-one"));
        // A hook that omits the id must not erase the one we have.
        reg.apply_hook(&hook(pane, "UserPromptSubmit", ""));
        assert_eq!(reg.hook_session_id(pane), Some("sess-one"));
        // A new conversation in the same pane replaces it.
        reg.apply_hook(&hook(pane, "SessionStart", "sess-two"));
        assert_eq!(reg.hook_session_id(pane), Some("sess-two"));
    }

    #[test]
    fn exited_panes_names_a_quit_agent_but_not_a_closed_pane() {
        let quit = Uuid::from_u128(1);
        let closed = Uuid::from_u128(2);
        let mut reg = AgentRegistry::default();
        let live: HashSet<Uuid> = [quit, closed].into_iter().collect();
        let found: HashMap<Uuid, String> =
            [(quit, "claude".to_string()), (closed, "claude".to_string())]
                .into_iter()
                .collect();
        reg.apply_scan(&live, &found);
        assert!(reg.exited_panes(&live, &found).is_empty());

        // The agent in `quit` exited; its pane is still open.
        let found2: HashMap<Uuid, String> = [(closed, "claude".to_string())].into_iter().collect();
        assert_eq!(reg.exited_panes(&live, &found2), vec![quit]);

        // Closing the whole app: every pane leaves `live` at once, and none
        // of them counts as "the user quit the agent".
        let nothing = HashSet::new();
        assert!(reg.exited_panes(&nothing, &HashMap::new()).is_empty());
    }
}
