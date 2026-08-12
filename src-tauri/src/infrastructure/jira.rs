use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JiraIssueStatusEvidence {
    pub issue_key: String,
    pub status: String,
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
}
