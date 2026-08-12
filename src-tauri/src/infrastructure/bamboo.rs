use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BambooBuildStatus {
    Successful,
    Failed,
    InProgress,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BambooBuildEvidence {
    pub result_key: String,
    pub status: BambooBuildStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BambooEvidenceError {
    Conflict,
    Unavailable,
}

pub fn canonical_bamboo_result_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.contains('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

/// Recognizes the canonical Bamboo trigger capability, including the
/// namespace prefix MCP clients may add to a server tool name.
pub fn trusted_bamboo_trigger_tool(tool_name: &str) -> bool {
    tool_name == "bamboo_trigger_build" || tool_name.ends_with("_bamboo_trigger_build")
}

/// Extracts one exact Bamboo build result key from structured connector output.
pub fn extract_triggered_build_result_key(response: &Value) -> Option<String> {
    let mut identities = structured_bamboo_objects(response)
        .into_iter()
        .filter_map(|object| {
            bamboo_result_key(&object).or_else(|| {
                let plan = ["planKey", "plan_key"]
                    .iter()
                    .find_map(|key| object.get(*key).and_then(Value::as_str))?
                    .trim();
                let number = ["buildNumber", "build_number", "number"]
                    .iter()
                    .find_map(|key| object.get(*key))
                    .and_then(|value| match value {
                        Value::Number(value) => Some(value.to_string()),
                        Value::String(value) => Some(value.trim().to_string()),
                        _ => None,
                    })?;
                let combined = format!("{plan}-{number}");
                canonical_bamboo_result_key(&combined).then_some(combined)
            })
        })
        .map(|key| key.to_ascii_uppercase())
        .collect::<Vec<_>>();
    identities.sort_unstable();
    identities.dedup();
    (identities.len() == 1).then(|| identities.remove(0))
}

/// Parses a structured Bamboo build read and requires the expected result identity.
pub fn parse_bamboo_build_evidence(
    response: &Value,
    expected_result_key: &str,
) -> Result<BambooBuildEvidence, BambooEvidenceError> {
    let expected = expected_result_key.to_ascii_uppercase();
    let mut conflicting_identity = false;
    let mut observed_status = None;
    for object in structured_bamboo_objects(response) {
        let Some(identity) = bamboo_result_key(&object) else {
            continue;
        };
        if identity.to_ascii_uppercase() != expected {
            conflicting_identity = true;
            continue;
        }
        let state = ["buildState", "build_state", "state", "status"]
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str))
            .and_then(normalize_bamboo_build_status)
            .ok_or(BambooEvidenceError::Unavailable)?;
        if observed_status.is_some_and(|observed| observed != state) {
            return Err(BambooEvidenceError::Unavailable);
        }
        observed_status = Some(state);
    }
    if conflicting_identity {
        return Err(BambooEvidenceError::Conflict);
    }
    observed_status
        .map(|status| BambooBuildEvidence {
            result_key: expected,
            status,
        })
        .ok_or(BambooEvidenceError::Unavailable)
}

fn bamboo_result_key(object: &serde_json::Map<String, Value>) -> Option<String> {
    [
        "buildResultKey",
        "build_result_key",
        "resultKey",
        "result_key",
        "buildKey",
        "build_key",
        "key",
    ]
    .iter()
    .find_map(|key| object.get(*key).and_then(Value::as_str))
    .map(str::trim)
    .filter(|value| canonical_bamboo_result_key(value))
    .map(str::to_string)
}

fn structured_bamboo_objects(response: &Value) -> Vec<serde_json::Map<String, Value>> {
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

fn normalize_bamboo_build_status(value: &str) -> Option<BambooBuildStatus> {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-', '_'], "");
    match normalized.as_str() {
        "successful" | "success" | "passed" => Some(BambooBuildStatus::Successful),
        "failed" | "failure" | "error" | "cancelled" | "canceled" => {
            Some(BambooBuildStatus::Failed)
        }
        "inprogress" | "building" | "queued" | "pending" => Some(BambooBuildStatus::InProgress),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_trigger_identity_from_exact_key_or_plan_and_number() {
        assert_eq!(
            extract_triggered_build_result_key(&json!({"buildResultKey": "PAYROLL-DEPLOY-42"})),
            Some("PAYROLL-DEPLOY-42".to_string())
        );
        assert_eq!(
            extract_triggered_build_result_key(&json!({
                "content": [{"type": "text", "text": "{\"planKey\":\"PAYROLL-DEPLOY\",\"buildNumber\":42}"}]
            })),
            Some("PAYROLL-DEPLOY-42".to_string())
        );
    }

    #[test]
    fn trigger_identity_rejects_prose() {
        assert_eq!(
            extract_triggered_build_result_key(&json!({
                "content": [{"type": "text", "text": "Build PAYROLL-DEPLOY-42 started"}]
            })),
            None
        );
        assert_eq!(
            extract_triggered_build_result_key(&json!({
                "results": [
                    {"buildResultKey": "PAYROLL-DEPLOY-42"},
                    {"buildResultKey": "PAYROLL-DEPLOY-43"}
                ]
            })),
            None
        );
    }

    #[test]
    fn recognizes_canonical_and_namespaced_trigger_tools_only() {
        assert!(trusted_bamboo_trigger_tool("bamboo_trigger_build"));
        assert!(trusted_bamboo_trigger_tool(
            "corporate_bamboo_trigger_build"
        ));
        assert!(!trusted_bamboo_trigger_tool("bamboo_get_build"));
        assert!(!trusted_bamboo_trigger_tool("trigger_build"));
    }
}
