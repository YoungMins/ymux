//! Ollama (and LM Studio's Ollama-compatible mode): request builders and
//! response parsers. `base` is e.g. `http://127.0.0.1:11434`.

use serde_json::{json, Value};

use super::{clean_output, join, system_prompt, HttpRequest};

/// Longest facts JSON forwarded to the model.
const MAX_FACTS_CHARS: usize = 8000;

/// `GET /api/tags`.
pub fn list_models_request(base: &str) -> HttpRequest {
    HttpRequest {
        method: "GET",
        url: join(base, "api/tags"),
        headers: Vec::new(),
        body: None,
    }
}

/// `models[].name`, deduplicated, at most 200 entries.
pub fn parse_models(body: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for m in v
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(n) = m.get("name").and_then(Value::as_str) {
            let n: String = n.chars().filter(|c| !c.is_control()).take(200).collect();
            if !n.is_empty() && !out.contains(&n) {
                out.push(n);
            }
        }
        if out.len() >= 200 {
            break;
        }
    }
    out
}

/// `POST /api/chat`, non-streaming. `facts_json` is the redacted facts JSON.
pub fn chat_request(base: &str, model: &str, lang: &str, facts_json: &str) -> HttpRequest {
    let facts: String = facts_json.chars().take(MAX_FACTS_CHARS).collect();
    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system_prompt(lang)},
            {"role": "user", "content": facts},
        ],
        "stream": false,
        "options": {"num_predict": 200, "temperature": 0.2},
    });
    HttpRequest {
        method: "POST",
        url: join(base, "api/chat"),
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: Some(body.to_string()),
    }
}

/// `message.content`, cleaned and clipped.
pub fn parse_chat(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    clean_output(v.get("message")?.get("content")?.as_str()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_request() {
        let r = list_models_request("http://127.0.0.1:11434/");
        assert_eq!(
            (r.method, r.url.as_str()),
            ("GET", "http://127.0.0.1:11434/api/tags")
        );
        assert!(r.body.is_none());
    }

    #[test]
    fn models_parse() {
        let b = r#"{"models":[{"name":"gemma3:1b"},{"name":"qwen3:4b"},{"name":"gemma3:1b"},{"x":1},{"name":""}]}"#;
        assert_eq!(parse_models(b), vec!["gemma3:1b", "qwen3:4b"]);
        assert!(parse_models("not json").is_empty());
        assert!(parse_models("{}").is_empty());
    }

    #[test]
    fn chat_request_shape() {
        let r = chat_request("http://localhost:11434", "gemma3:1b", "ko", "{\"files\":2}");
        assert_eq!(r.url, "http://localhost:11434/api/chat");
        let v: Value = serde_json::from_str(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(v["model"], "gemma3:1b");
        assert_eq!(v["stream"], false);
        assert_eq!(v["options"]["num_predict"], 200);
        assert_eq!(v["options"]["temperature"], 0.2);
        assert_eq!(v["messages"][0]["role"], "system");
        assert!(v["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Korean"));
        assert_eq!(v["messages"][1]["content"], "{\"files\":2}");
    }

    #[test]
    fn chat_parse() {
        let b = r#"{"message":{"role":"assistant","content":" Edited 2 files.\n"},"done":true}"#;
        assert_eq!(parse_chat(b).as_deref(), Some("Edited 2 files."));
        assert_eq!(parse_chat(r#"{"message":{"content":""}}"#), None);
        assert_eq!(parse_chat(r#"{"message":{}}"#), None);
        assert_eq!(parse_chat("garbage"), None);
    }

    #[test]
    fn oversized_facts_are_clipped() {
        let r = chat_request("http://h", "m", "en", &"a".repeat(50_000));
        assert!(r.body.unwrap().len() < 10_000);
    }
}
