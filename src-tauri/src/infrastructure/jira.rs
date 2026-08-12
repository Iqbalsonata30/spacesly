use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JiraIssueStatusEvidence {
    pub issue_key: String,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JiraCommentReference {
    pub issue_key: String,
    pub comment_id: String,
    pub content_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JiraCommentEvidence {
    pub issue_key: String,
    pub comment_id: String,
    pub content_matches: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JiraEvidenceError {
    Conflict,
    Unavailable,
}

pub fn canonical_jira_issue_key(value: &str) -> bool {
    let Some((project, number)) = value.split_once('-') else {
        return false;
    };
    !project.is_empty()
        && project.len() <= 32
        && project
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        && project
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_uppercase)
        && !number.is_empty()
        && number.len() <= 12
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && !number.starts_with('0')
}

pub fn valid_jira_status(value: &str) -> bool {
    value.trim() == value
        && !value.is_empty()
        && value.len() <= 128
        && !value.chars().any(char::is_control)
}

pub fn canonical_jira_comment_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub fn canonical_state_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn trusted_jira_comment_tool(tool_name: &str) -> bool {
    matches!(tool_name, "jira_add_comment" | "jira_create_comment")
        || tool_name.ends_with("_jira_add_comment")
        || tool_name.ends_with("_jira_create_comment")
}

/// Captures the exact Jira comment identity and a one-way fingerprint of its desired content.
pub fn extract_created_jira_comment_reference(
    arguments: &Value,
    response: &Value,
) -> Option<JiraCommentReference> {
    let arguments = arguments.as_object()?;
    let issue_key = ["issue_key", "issueKey", "key"]
        .iter()
        .find_map(|key| arguments.get(*key).and_then(Value::as_str))?
        .trim()
        .to_ascii_uppercase();
    if !canonical_jira_issue_key(&issue_key) {
        return None;
    }
    let bodies = ["body", "comment", "text", "content"]
        .iter()
        .filter_map(|key| arguments.get(*key))
        .filter_map(normalized_comment_text)
        .collect::<Vec<_>>();
    let body = unique_value(bodies)?;
    let mut ids = structured_jira_objects(response)
        .iter()
        .filter_map(jira_comment_id)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != 1 {
        return None;
    }
    Some(JiraCommentReference {
        issue_key,
        comment_id: ids.remove(0),
        content_fingerprint: comment_fingerprint(&body),
    })
}

/// Verifies one captured Jira comment against a structured exact-issue read.
pub fn parse_jira_comment_evidence(
    response: &Value,
    expected_issue_key: &str,
    expected_comment_id: &str,
    expected_fingerprint: &str,
) -> Result<JiraCommentEvidence, JiraEvidenceError> {
    if !canonical_jira_issue_key(expected_issue_key)
        || !canonical_jira_comment_id(expected_comment_id)
        || !canonical_state_fingerprint(expected_fingerprint)
    {
        return Err(JiraEvidenceError::Unavailable);
    }
    let expected_issue = expected_issue_key.to_ascii_uppercase();
    let objects = structured_jira_objects(response);
    let issue_keys = objects
        .iter()
        .filter_map(jira_issue_key)
        .map(|key| key.to_ascii_uppercase())
        .collect::<Vec<_>>();
    if issue_keys.iter().any(|key| key != &expected_issue) {
        return Err(JiraEvidenceError::Conflict);
    }
    if !issue_keys.iter().any(|key| key == &expected_issue) {
        return Err(JiraEvidenceError::Unavailable);
    }
    let mut fingerprints = objects
        .iter()
        .filter(|object| jira_comment_id(object).as_deref() == Some(expected_comment_id))
        .filter_map(jira_comment_body)
        .map(|body| comment_fingerprint(&body))
        .collect::<Vec<_>>();
    fingerprints.sort_unstable();
    fingerprints.dedup();
    if fingerprints.len() != 1 {
        return Err(JiraEvidenceError::Unavailable);
    }
    Ok(JiraCommentEvidence {
        issue_key: expected_issue,
        comment_id: expected_comment_id.to_string(),
        content_matches: fingerprints[0] == expected_fingerprint,
    })
}

/// Parses one exact Jira issue status from structured connector output.
pub fn parse_jira_issue_status_evidence(
    response: &Value,
    expected_issue_key: &str,
) -> Result<JiraIssueStatusEvidence, JiraEvidenceError> {
    let expected = expected_issue_key.to_ascii_uppercase();
    let mut statuses = Vec::new();
    let mut conflicting_identity = false;
    for object in structured_jira_objects(response) {
        let Some(issue_key) = jira_issue_key(&object) else {
            continue;
        };
        if issue_key.to_ascii_uppercase() != expected {
            conflicting_identity = true;
            continue;
        }
        let fields = object
            .get("fields")
            .and_then(Value::as_object)
            .unwrap_or(&object);
        let status = jira_status(fields).ok_or(JiraEvidenceError::Unavailable)?;
        statuses.push(status);
    }
    if conflicting_identity {
        return Err(JiraEvidenceError::Conflict);
    }
    statuses.sort_by_key(|status| status.to_ascii_lowercase());
    statuses.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    if statuses.len() != 1 {
        return Err(JiraEvidenceError::Unavailable);
    }
    Ok(JiraIssueStatusEvidence {
        issue_key: expected,
        status: statuses.remove(0),
    })
}

fn jira_issue_key(object: &serde_json::Map<String, Value>) -> Option<String> {
    ["key", "issueKey", "issue_key"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| canonical_jira_issue_key(&value.to_ascii_uppercase()))
        .map(str::to_string)
}

fn jira_status(object: &serde_json::Map<String, Value>) -> Option<String> {
    ["status", "state"]
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| match value {
            Value::String(value) => Some(value.as_str()),
            Value::Object(value) => value.get("name").and_then(Value::as_str),
            _ => None,
        })
        .map(str::trim)
        .filter(|value| valid_jira_status(value))
        .map(str::to_string)
}

fn jira_comment_id(object: &serde_json::Map<String, Value>) -> Option<String> {
    let explicit = ["commentId", "comment_id"]
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(string_or_number);
    let contextual = object.get("id").and_then(string_or_number).filter(|_| {
        ["body", "comment", "content"]
            .iter()
            .any(|key| object.contains_key(*key))
    });
    explicit
        .or(contextual)
        .map(|value| value.trim().to_string())
        .filter(|value| canonical_jira_comment_id(value))
}

fn string_or_number(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn jira_comment_body(object: &serde_json::Map<String, Value>) -> Option<String> {
    ["body", "comment", "content"]
        .iter()
        .filter_map(|key| object.get(*key))
        .filter_map(normalized_comment_text)
        .next()
}

fn normalized_comment_text(value: &Value) -> Option<String> {
    let raw = if let Some(value) = value.as_str() {
        value.to_string()
    } else {
        let mut text = Vec::new();
        collect_adf_text(value, 0, &mut text);
        text.join(" ")
    };
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty() && normalized.len() <= 32_000).then_some(normalized)
}

fn collect_adf_text(value: &Value, depth: usize, output: &mut Vec<String>) {
    if depth > 12 || output.len() > 4_096 {
        return;
    }
    match value {
        Value::Object(object) => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                output.push(text.to_string());
            }
            for (key, value) in object {
                if key != "text" {
                    collect_adf_text(value, depth + 1, output);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_adf_text(value, depth + 1, output);
            }
        }
        _ => {}
    }
}

fn unique_value(mut values: Vec<String>) -> Option<String> {
    values.sort_unstable();
    values.dedup();
    (values.len() == 1).then(|| values.remove(0))
}

fn comment_fingerprint(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn structured_jira_objects(response: &Value) -> Vec<serde_json::Map<String, Value>> {
    let mut roots = vec![response];
    let decoded = response
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .collect::<Vec<_>>();
    if let Some(structured) = response.get("structuredContent") {
        roots.push(structured);
    }

    let mut objects = Vec::new();
    for root in roots.into_iter().chain(decoded.iter()) {
        let mut stack = vec![(root, 0_usize)];
        let mut visited = 0_usize;
        while let Some((value, depth)) = stack.pop() {
            visited = visited.saturating_add(1);
            if visited > 128 || depth > 6 {
                continue;
            }
            if let Some(object) = value.as_object() {
                objects.push(object.clone());
                stack.extend(object.values().map(|value| (value, depth + 1)));
            } else if let Some(values) = value.as_array() {
                stack.extend(values.iter().map(|value| (value, depth + 1)));
            }
        }
    }
    objects
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_exact_issue_status_from_structured_json() {
        let evidence = parse_jira_issue_status_evidence(
            &json!({
                "content": [{
                    "type": "text",
                    "text": "{\"key\":\"OPS-42\",\"fields\":{\"status\":{\"name\":\"In Progress\"}}}"
                }]
            }),
            "OPS-42",
        )
        .expect("exact issue parses");
        assert_eq!(evidence.issue_key, "OPS-42");
        assert_eq!(evidence.status, "In Progress");
    }

    #[test]
    fn rejects_prose_conflicts_and_ambiguous_statuses() {
        assert_eq!(
            parse_jira_issue_status_evidence(
                &json!({"content": [{"type": "text", "text": "OPS-42 is Done"}]}),
                "OPS-42",
            ),
            Err(JiraEvidenceError::Unavailable)
        );
        assert_eq!(
            parse_jira_issue_status_evidence(&json!({"key": "OPS-43", "status": "Done"}), "OPS-42",),
            Err(JiraEvidenceError::Conflict)
        );
        assert_eq!(
            parse_jira_issue_status_evidence(
                &json!({"results": [
                    {"key": "OPS-42", "status": "Done"},
                    {"key": "OPS-42", "status": "In Progress"}
                ]}),
                "OPS-42",
            ),
            Err(JiraEvidenceError::Unavailable)
        );
    }

    #[test]
    fn captures_comment_identity_and_verifies_plain_or_adf_content() {
        let reference = extract_created_jira_comment_reference(
            &json!({"issue_key": "OPS-42", "comment": "Deployment  completed"}),
            &json!({"commentId": "10042"}),
        )
        .expect("comment reference captured");
        assert_eq!(reference.issue_key, "OPS-42");
        assert_eq!(reference.comment_id, "10042");
        assert!(canonical_state_fingerprint(&reference.content_fingerprint));

        let evidence = parse_jira_comment_evidence(
            &json!({
                "key": "OPS-42",
                "fields": {"comment": {"comments": [{
                    "id": "10042",
                    "body": {"type": "doc", "content": [{
                        "type": "paragraph",
                        "content": [{"type": "text", "text": "Deployment completed"}]
                    }]}
                }]}}
            }),
            "OPS-42",
            "10042",
            &reference.content_fingerprint,
        )
        .expect("comment evidence parses");
        assert!(evidence.content_matches);
    }

    #[test]
    fn comment_capture_and_verification_reject_ambiguity_and_drift() {
        assert_eq!(
            extract_created_jira_comment_reference(
                &json!({"issue_key": "OPS-42", "body": "one", "comment": "two"}),
                &json!({"commentId": "10042"}),
            ),
            None
        );
        assert_eq!(
            extract_created_jira_comment_reference(
                &json!({"issue_key": "OPS-42", "body": "one"}),
                &json!({"results": [{"commentId": "10042"}, {"commentId": "10043"}]}),
            ),
            None
        );
        let reference = extract_created_jira_comment_reference(
            &json!({"issue_key": "OPS-42", "body": "expected"}),
            &json!({"comment_id": "10042"}),
        )
        .expect("reference");
        let drift = parse_jira_comment_evidence(
            &json!({"key": "OPS-42", "comments": [{"id": "10042", "body": "changed"}]}),
            "OPS-42",
            "10042",
            &reference.content_fingerprint,
        )
        .expect("drift is observable evidence");
        assert!(!drift.content_matches);
        assert_eq!(
            parse_jira_comment_evidence(
                &json!({"key": "OPS-43", "comments": [{"id": "10042", "body": "expected"}]}),
                "OPS-42",
                "10042",
                &reference.content_fingerprint,
            ),
            Err(JiraEvidenceError::Conflict)
        );
    }
}
