//! Per-pane "auto-continue": answer Claude Code's `AskUserQuestion` from the
//! `PreToolUse` http hook, so an unattended agent never stalls on a question.
//!
//! Pure `std` + `serde_json`, not desktop-gated. `hook_http` calls
//! [`decide`] on the already-parsed hook body and, only for a pane in
//! [`AutoPanes`], answers with [`Decision::body`] — the single exception to
//! "2xx means an empty body" (CLAUDE.md rule 13).
//!
//! The `tool_input` shape and the `answers` format are taken from Claude
//! Code's docs, not yet confirmed against a live payload; the one place that
//! encodes how several selected labels become one value is [`join_labels`].

use std::collections::HashSet;

use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use uuid::Uuid;

/// The only tool auto-continue answers.
pub const TOOL: &str = "AskUserQuestion";
/// An option whose label ends with this (case-insensitive, trimmed) is the
/// one Claude recommends.
pub const RECOMMENDED_SUFFIX: &str = "(recommended)";
/// Why a question set with no recommended option is denied. The text goes
/// back to the model, so it tells it what to do instead.
pub const DENY_REASON: &str = "yMux auto-continue: no option was marked (Recommended). \
Choose the best option yourself and continue without asking.";

/// Why a call with two questions of identical text is denied: answers are
/// keyed by question text, so one would silently overwrite the other.
pub const DUPLICATE_REASON: &str = "yMux auto-continue: two questions have the same text, so \
their answers cannot be told apart. Make each question unique, or choose yourself and continue \
without asking.";

/// Longest question/answer (in chars) carried in a logged event.
pub const LOG_CLIP_CHARS: usize = 200;

/// Clip `s` to at most [`LOG_CLIP_CHARS`] chars (never splitting a char),
/// ending in `…` when cut. For the frontend log only, never the decision.
pub fn clip_for_log(s: &str) -> String {
    match s.char_indices().nth(LOG_CLIP_CHARS) {
        None => s.to_string(),
        Some((i, _)) => format!("{}…", &s[..i]),
    }
}

/// How the labels chosen for a `multiSelect` question become its one answer
/// string. Change here if a live check shows another separator.
pub fn join_labels(labels: &[&str]) -> String {
    labels.join(", ")
}

/// Panes with auto-continue on. Runtime only: never persisted, so a restart
/// turns it off everywhere. Tiny lock, held for one set operation.
#[derive(Debug, Default)]
pub struct AutoPanes(Mutex<HashSet<Uuid>>);

impl AutoPanes {
    pub fn set(&self, pane: Uuid, on: bool) {
        let mut set = self.0.lock();
        if on {
            set.insert(pane);
        } else {
            set.remove(&pane);
        }
    }

    pub fn contains(&self, pane: Uuid) -> bool {
        self.0.lock().contains(&pane)
    }
}

/// What to tell Claude Code about one `AskUserQuestion`.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Allow, with the original input plus the chosen `answers`.
    Answer {
        updated_input: Value,
        /// `(question, answer)` in question order, for the log.
        answers: Vec<(String, String)>,
    },
    /// Deny with [`DENY_REASON`].
    Deny,
    /// Deny with [`DUPLICATE_REASON`].
    DenyDuplicate,
}

impl Decision {
    /// The hook's JSON response body.
    pub fn body(&self) -> String {
        let out = match self {
            Decision::Answer { updated_input, .. } => json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "updatedInput": updated_input,
            }}),
            Decision::Deny => deny_body(DENY_REASON),
            Decision::DenyDuplicate => deny_body(DUPLICATE_REASON),
        };
        out.to_string()
    }
}

fn deny_body(reason: &str) -> Value {
    json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": reason,
    }})
}

fn is_recommended(label: &str) -> bool {
    label.trim().to_lowercase().ends_with(RECOMMENDED_SUFFIX)
}

/// Decide a parsed hook body. `None` (no decision: normal UI) unless it is a
/// `PreToolUse` for [`TOOL`] with a well-formed `tool_input`
/// (`questions[] = {question, options[] = {label}, multiSelect?}`).
pub fn decide(hook: &Value) -> Option<Decision> {
    if hook.get("hook_event_name")?.as_str()? != "PreToolUse"
        || hook.get("tool_name")?.as_str()? != TOOL
    {
        return None;
    }
    let tool_input = hook.get("tool_input")?.as_object()?;
    let questions = tool_input.get("questions")?.as_array()?;
    if questions.is_empty() {
        return None;
    }
    let mut answers: Vec<(String, String)> = Vec::with_capacity(questions.len());
    let mut missing = false;
    for q in questions {
        let text = q.get("question")?.as_str()?;
        let multi = q
            .get("multiSelect")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut labels = Vec::new();
        for opt in q.get("options")?.as_array()? {
            labels.push(opt.get("label")?.as_str()?);
        }
        let mut picked = labels.into_iter().filter(|l| is_recommended(l));
        let chosen = if multi {
            let all: Vec<&str> = picked.collect();
            (!all.is_empty()).then(|| join_labels(&all))
        } else {
            picked.next().map(str::to_string)
        };
        match chosen {
            Some(c) => answers.push((text.to_string(), c)),
            None => missing = true,
        }
    }
    let mut seen = HashSet::new();
    if !answers.iter().all(|(q, _)| seen.insert(q.as_str())) {
        return Some(Decision::DenyDuplicate);
    }
    if missing {
        return Some(Decision::Deny);
    }
    let mut updated = tool_input.clone();
    let map: Map<String, Value> = answers
        .iter()
        .map(|(q, a)| (q.clone(), Value::String(a.clone())))
        .collect();
    updated.insert("answers".into(), Value::Object(map));
    Some(Decision::Answer {
        updated_input: Value::Object(updated),
        answers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(tool_input: Value) -> Value {
        json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
               "session_id": "s", "tool_input": tool_input})
    }

    fn q(text: &str, multi: bool, labels: &[&str]) -> Value {
        let options: Vec<Value> = labels
            .iter()
            .map(|l| json!({"label": l, "description": "d"}))
            .collect();
        json!({"question": text, "header": "H", "multiSelect": multi, "options": options})
    }

    #[test]
    fn picks_the_recommended_single_option() {
        let input = json!({"questions": [q("Which db?", false,
            &["SQLite", "Postgres (Recommended)", "Redis"])], "extra": 1});
        let Some(Decision::Answer {
            updated_input,
            answers,
        }) = decide(&hook(input))
        else {
            panic!("expected an answer");
        };
        assert_eq!(
            answers,
            vec![(
                "Which db?".to_string(),
                "Postgres (Recommended)".to_string()
            )]
        );
        // The original fields survive; `answers` is added.
        assert_eq!(updated_input["extra"], 1);
        assert_eq!(updated_input["questions"][0]["header"], "H");
        assert_eq!(
            updated_input["answers"]["Which db?"],
            "Postgres (Recommended)"
        );
    }

    #[test]
    fn recommended_is_case_insensitive_and_trimmed() {
        let input = json!({"questions": [q("A?", false, &["x", "  yes (RECOMMENDED)  "])]});
        let Some(Decision::Answer { answers, .. }) = decide(&hook(input)) else {
            panic!()
        };
        assert_eq!(answers[0].1, "  yes (RECOMMENDED)  ");
    }

    #[test]
    fn multi_select_takes_every_recommended_option() {
        let input = json!({"questions": [q("Which?", true,
            &["a (Recommended)", "b", "c (recommended)"])]});
        let Some(Decision::Answer { answers, .. }) = decide(&hook(input)) else {
            panic!()
        };
        assert_eq!(answers[0].1, "a (Recommended), c (recommended)");
    }

    #[test]
    fn several_questions_each_get_an_answer() {
        let input = json!({"questions": [
            q("One?", false, &["x (Recommended)"]),
            q("Two?", false, &["y", "z (Recommended)"]),
        ]});
        let Some(Decision::Answer { answers, .. }) = decide(&hook(input)) else {
            panic!()
        };
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[1].0, "Two?");
    }

    #[test]
    fn any_question_without_a_recommendation_denies() {
        let input = json!({"questions": [
            q("One?", false, &["x (Recommended)"]),
            q("Two?", false, &["y", "z"]),
        ]});
        assert_eq!(decide(&hook(input)), Some(Decision::Deny));
        let multi = json!({"questions": [q("M?", true, &["a", "b"])]});
        assert_eq!(decide(&hook(multi)), Some(Decision::Deny));
        // "(Recommended)" must be a suffix, not just mentioned.
        let inner = json!({"questions": [q("I?", false, &["(Recommended) first", "b"])]});
        assert_eq!(decide(&hook(inner)), Some(Decision::Deny));
    }

    #[test]
    fn malformed_or_foreign_input_is_no_decision() {
        let bad = [
            json!(null),
            json!([]),
            json!({}),
            json!({"questions": []}),
            json!({"questions": "x"}),
            json!({"questions": [{"options": []}]}),
            json!({"questions": [{"question": "q"}]}),
            json!({"questions": [{"question": "q", "options": [{"nolabel": 1}]}]}),
            json!({"questions": [{"question": 5, "options": []}]}),
        ];
        for input in bad {
            assert_eq!(decide(&hook(input.clone())), None, "{input}");
        }
        let ok = json!({"questions": [q("A?", false, &["a (Recommended)"])]});
        let mut other_tool = hook(ok.clone());
        other_tool["tool_name"] = json!("Bash");
        assert_eq!(decide(&other_tool), None);
        let mut other_event = hook(ok.clone());
        other_event["hook_event_name"] = json!("PostToolUse");
        assert_eq!(decide(&other_event), None);
        assert_eq!(decide(&json!({"hook_event_name": "PreToolUse"})), None);
        assert_eq!(decide(&json!("str")), None);
    }

    #[test]
    fn response_json_has_the_documented_shape() {
        let input = json!({"questions": [q("A?", false, &["a (Recommended)"])]});
        let d = decide(&hook(input)).unwrap();
        let v: Value = serde_json::from_str(&d.body()).unwrap();
        let out = &v["hookSpecificOutput"];
        assert_eq!(out["hookEventName"], "PreToolUse");
        assert_eq!(out["permissionDecision"], "allow");
        assert_eq!(out["updatedInput"]["answers"]["A?"], "a (Recommended)");
        assert!(out.get("permissionDecisionReason").is_none());

        let v: Value = serde_json::from_str(&Decision::Deny.body()).unwrap();
        let out = &v["hookSpecificOutput"];
        assert_eq!(out["hookEventName"], "PreToolUse");
        assert_eq!(out["permissionDecision"], "deny");
        assert_eq!(out["permissionDecisionReason"], DENY_REASON);
    }

    #[test]
    fn duplicate_question_text_denies_instead_of_collapsing() {
        let input = json!({"questions": [
            q("Same?", false, &["x (Recommended)"]),
            q("Same?", false, &["y (Recommended)"]),
        ]});
        let d = decide(&hook(input)).unwrap();
        assert_eq!(d, Decision::DenyDuplicate);
        let v: Value = serde_json::from_str(&d.body()).unwrap();
        let out = &v["hookSpecificOutput"];
        assert_eq!(out["permissionDecision"], "deny");
        assert_eq!(out["permissionDecisionReason"], DUPLICATE_REASON);
    }

    #[test]
    fn clip_for_log_counts_chars_not_bytes() {
        assert_eq!(clip_for_log("short"), "short");
        let exact = "한".repeat(LOG_CLIP_CHARS);
        assert_eq!(clip_for_log(&exact), exact);
        let long = "한".repeat(LOG_CLIP_CHARS + 50);
        let clipped = clip_for_log(&long);
        assert_eq!(clipped.chars().count(), LOG_CLIP_CHARS + 1);
        assert!(clipped.starts_with(&exact) && clipped.ends_with('…'));
        // The decision body itself is never clipped.
        let input = json!({"questions": [q(&long, false, &["a (Recommended)"])]});
        let d = decide(&hook(input)).unwrap();
        assert!(d.body().contains(&long));
    }

    #[test]
    fn auto_panes_toggle() {
        let set = AutoPanes::default();
        let p = Uuid::new_v4();
        assert!(!set.contains(p));
        set.set(p, true);
        assert!(set.contains(p));
        set.set(p, false);
        assert!(!set.contains(p));
    }
}
