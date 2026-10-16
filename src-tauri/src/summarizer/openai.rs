//! OpenAI-compatible endpoints (LM Studio, llama.cpp server, vLLM, hosted):
//! request builders and response parsers. `base` may or may not already end
//! in `/v1`.

use serde_json::{json, Value};

use super::{clean_output, header_value, join, system_prompt, HttpRequest};

const MAX_FACTS_CHARS: usize = 8000;

fn v1(base: &str, path: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    if b.ends_with("/v1") {
        join(b, path)
    } else {
        join(b, &format!("v1/{path}"))
    }
}

fn headers(api_key: &str, json_body: bool) -> Vec<(String, String)> {
    let mut h = Vec::new();
    if json_body {
        h.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    let key = header_value(api_key);
    if !key.is_empty() {
        h.push(("Authorization".to_string(), format!("Bearer {key}")));
    }
    h
}

/// `GET /v1/models`.
pub fn list_models_request(base: &str, api_key: &str) -> HttpRequest {
    HttpRequest {
        method: "GET",
        url: v1(base, "models"),
        headers: headers(api_key, false),
        body: None,
    }
}

/// `data[].id`, deduplicated, at most 200 entries.
pub fn parse_models(body: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for m in v
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(n) = m.get("id").and_then(Value::as_str) {
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

/// `POST /v1/chat/completions`. `facts_json` is the redacted facts JSON.
pub fn chat_request(
    base: &str,
    api_key: &str,
    model: &str,
    lang: &str,
    facts_json: &str,
) -> HttpRequest {
    let facts: String = facts_json.chars().take(MAX_FACTS_CHARS).collect();
    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system_prompt(lang)},
            {"role": "user", "content": facts},
        ],
        "max_tokens": 200,
        "temperature": 0.2,
    });
    HttpRequest {
        method: "POST",
        url: v1(base, "chat/completions"),
        headers: headers(api_key, true),
        body: Some(body.to_string()),
    }
}

/// `choices[0].message.content`, cleaned and clipped.
pub fn parse_chat(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    clean_output(
        v.get("choices")?
            .get(0)?
            .get("message")?
            .get("content")?
            .as_str()?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_with_and_without_v1() {
        assert_eq!(
            list_models_request("http://localhost:1234", "").url,
            "http://localhost:1234/v1/models"
        );
        assert_eq!(
            list_models_request("http://localhost:1234/v1/", "").url,
            "http://localhost:1234/v1/models"
        );
        assert_eq!(
            chat_request("https://api.x.com/v1", "", "m", "en", "{}").url,
            "https://api.x.com/v1/chat/completions"
        );
    }

    #[test]
    fn auth_header_only_with_key_and_is_sanitised() {
        assert!(list_models_request("http://h", "").headers.is_empty());
        let r = list_models_request("http://h", " sk-1\r\nX-Evil: 1 ");
        assert_eq!(
            r.headers,
            vec![(
                "Authorization".to_string(),
                "Bearer sk-1X-Evil: 1".to_string()
            )]
        );
    }

    #[test]
    fn models_parse() {
        let b = r#"{"object":"list","data":[{"id":"gpt-x"},{"id":"local/m"},{"id":"gpt-x"},{}]}"#;
        assert_eq!(parse_models(b), vec!["gpt-x", "local/m"]);
        assert!(parse_models("[]").is_empty());
    }

    #[test]
    fn chat_request_shape() {
        let r = chat_request("http://h", "k", "gpt-x", "ja", "{}");
        let v: Value = serde_json::from_str(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(v["model"], "gpt-x");
        assert_eq!(v["max_tokens"], 200);
        assert_eq!(v["temperature"], 0.2);
        assert!(v.get("stream").is_none());
        assert!(v["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Japanese"));
        assert!(r.headers.iter().any(|(k, _)| k == "Content-Type"));
    }

    #[test]
    fn chat_parse() {
        let b = r#"{"choices":[{"message":{"role":"assistant","content":"All good.\u0007"}}]}"#;
        assert_eq!(parse_chat(b).as_deref(), Some("All good."));
        assert_eq!(parse_chat(r#"{"choices":[]}"#), None);
        assert_eq!(
            parse_chat(r#"{"choices":[{"message":{"content":null}}]}"#),
            None
        );
        let long = format!(
            r#"{{"choices":[{{"message":{{"content":"{}"}}}}]}}"#,
            "y".repeat(3000)
        );
        assert_eq!(parse_chat(&long).unwrap().chars().count(), 600);
    }
}
