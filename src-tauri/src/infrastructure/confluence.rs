use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Secret-free proof that a connector returned one exact Confluence page identity.
pub struct ConfluencePageEvidence {
    pub page_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Fail-closed classification for exact-page connector evidence.
pub enum ConfluenceEvidenceError {
    Conflict,
    Unavailable,
}

/// Returns whether a value is a bounded canonical numeric Confluence page ID.
pub fn canonical_confluence_page_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Parses structured connector output and requires the exact expected page identity.
pub fn parse_confluence_page_evidence(
    response: &Value,
    expected_page_id: &str,
) -> Result<ConfluencePageEvidence, ConfluenceEvidenceError> {
    if !canonical_confluence_page_id(expected_page_id) {
        return Err(ConfluenceEvidenceError::Unavailable);
    }
    let object = exact_page_object(response).ok_or(ConfluenceEvidenceError::Unavailable)?;
    let page_id = page_id(&object).ok_or(ConfluenceEvidenceError::Unavailable)?;
    if page_id != expected_page_id {
        return Err(ConfluenceEvidenceError::Conflict);
    }
    if !object
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|title| {
            !title.is_empty() && title.len() <= 512 && !title.chars().any(char::is_control)
        })
    {
        return Err(ConfluenceEvidenceError::Unavailable);
    }
    Ok(ConfluencePageEvidence { page_id })
}

fn page_id(object: &serde_json::Map<String, Value>) -> Option<String> {
    ["page_id", "pageId", "id"]
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| match value {
            Value::String(value) => Some(value.trim().to_string()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
        .filter(|value| canonical_confluence_page_id(value))
}

fn exact_page_object(response: &Value) -> Option<serde_json::Map<String, Value>> {
    if response.get("error").is_some() {
        return None;
    }
    if let Some(structured) = response.get("structuredContent") {
        return structured.as_object().cloned();
    }
    let mut decoded = response
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .collect::<Vec<_>>();
    if decoded.len() == 1 {
        return decoded.remove(0).as_object().cloned();
    }
    response.as_object().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_exact_structured_page_and_rejects_prose_or_conflicts() {
        let evidence = parse_confluence_page_evidence(
            &json!({
                "content": [{
                    "type": "text",
                    "text": "{\"id\":\"123456\",\"title\":\"Release Runbook\",\"body\":\"sensitive body\"}"
                }]
            }),
            "123456",
        )
        .expect("exact page parses");
        assert_eq!(evidence.page_id, "123456");
        let retained = serde_json::to_string(&evidence).expect("evidence serializes");
        assert!(!retained.contains("sensitive body"));
        assert_eq!(
            parse_confluence_page_evidence(
                &json!({"content": [{"type": "text", "text": "Page 123456 exists"}]}),
                "123456"
            ),
            Err(ConfluenceEvidenceError::Unavailable)
        );
        assert_eq!(
            parse_confluence_page_evidence(&json!({"id": "999999", "title": "Other"}), "123456"),
            Err(ConfluenceEvidenceError::Conflict)
        );
        assert_eq!(
            parse_confluence_page_evidence(
                &json!({
                    "result": {"id": "123456", "title": "Nested reference"},
                    "request": {"id": "999999", "title": "Metadata"}
                }),
                "123456"
            ),
            Err(ConfluenceEvidenceError::Unavailable)
        );
        assert_eq!(
            parse_confluence_page_evidence(
                &json!({"structuredContent": {"id": "123456", "title": ""}}),
                "123456"
            ),
            Err(ConfluenceEvidenceError::Unavailable)
        );
        assert_eq!(
            parse_confluence_page_evidence(
                &json!({
                    "content": [
                        {"type": "text", "text": "{\"id\":\"123456\",\"title\":\"One\"}"},
                        {"type": "text", "text": "{\"id\":\"123456\",\"title\":\"Two\"}"}
                    ]
                }),
                "123456"
            ),
            Err(ConfluenceEvidenceError::Unavailable)
        );
    }
}
