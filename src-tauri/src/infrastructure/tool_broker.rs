use std::collections::HashSet;

use serde::Serialize;
use sha2::{Digest, Sha256};

const MAX_TOOL_ARGUMENT_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    Read,
    Mutation,
    Destructive,
    CredentialSensitive,
    Unknown,
}

impl ToolRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Mutation => "mutation",
            Self::Destructive => "destructive",
            Self::CredentialSensitive => "credential_sensitive",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolDisplayContext {
    pub label: String,
    pub category: String,
    pub target: Option<String>,
}

pub fn tool_display_context(tool_name: &str, arguments: &serde_json::Value) -> ToolDisplayContext {
    let normalized = tool_name.trim().to_ascii_lowercase();
    let category = if matches!(
        normalized.as_str(),
        "read" | "glob" | "grep" | "list" | "ls" | "edit" | "write" | "apply_patch" | "patch"
    ) {
        "files"
    } else if matches!(normalized.as_str(), "bash" | "shell") {
        "commands"
    } else if normalized == "git" || normalized.contains("git_") {
        "git"
    } else if normalized.contains("jira") || normalized.contains("atlassian") {
        "jira"
    } else if normalized.contains("kubernetes")
        || normalized.contains("kube")
        || normalized.starts_with("ocp_")
    {
        "kubernetes"
    } else if normalized.contains("bamboo") {
        "bamboo"
    } else if matches!(normalized.as_str(), "todowrite" | "question" | "skill") {
        "runtime"
    } else {
        "external"
    };
    let target = match category {
        "files" => safe_relative_path(arguments),
        "jira" => safe_identifier(arguments, &["issue_key", "issueKey", "key"]),
        "kubernetes" => safe_identifier(arguments, &["name", "namespace"]),
        "bamboo" => safe_identifier(arguments, &["plan_key", "planKey", "build_key"]),
        _ => None,
    };
    let action = if normalized == "read" {
        "Reading"
    } else if matches!(
        normalized.as_str(),
        "edit" | "write" | "apply_patch" | "patch"
    ) {
        "Updating"
    } else if normalized == "glob" {
        "Finding files in"
    } else if normalized == "grep" {
        "Searching"
    } else if matches!(normalized.as_str(), "bash" | "shell") {
        "Running shell command in"
    } else if category == "git" {
        "Running Git operation in"
    } else if ToolBroker::risk_for_tool(&normalized) == ToolRisk::Destructive {
        "Deleting from"
    } else if ToolBroker::risk_for_tool(&normalized) == ToolRisk::Mutation {
        "Updating"
    } else if ToolBroker::risk_for_tool(&normalized) == ToolRisk::Read {
        "Reading from"
    } else {
        "Using"
    };
    let category_label = match category {
        "files" => "workspace",
        "commands" => "workspace",
        "git" => "repository",
        "jira" => "Jira",
        "kubernetes" => "Kubernetes",
        "bamboo" => "Bamboo",
        "runtime" => "Agent runtime",
        _ => "external tool",
    };
    ToolDisplayContext {
        label: target
            .as_deref()
            .map(|target| format!("{action} {target}"))
            .unwrap_or_else(|| format!("{action} {category_label}")),
        category: category.to_string(),
        target,
    }
}

fn safe_relative_path(arguments: &serde_json::Value) -> Option<String> {
    let value = string_argument(arguments, &["file_path", "filePath", "path"])?;
    let value = value.trim();
    if value.is_empty()
        || value.len() > 180
        || value.starts_with('/')
        || value.as_bytes().get(1) == Some(&b':')
        || value.split(['/', '\\']).any(|part| part == "..")
        || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value.to_string())
}

fn safe_identifier(arguments: &serde_json::Value, keys: &[&str]) -> Option<String> {
    let value = string_argument(arguments, keys)?.trim();
    if value.is_empty()
        || value.len() > 80
        || value
            .chars()
            .any(|character| !(character.is_ascii_alphanumeric() || "-_.:/".contains(character)))
    {
        return None;
    }
    Some(value.to_string())
}

fn string_argument<'a>(arguments: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| arguments.get(*key).and_then(serde_json::Value::as_str))
}

pub fn argument_digest(arguments: &serde_json::Value) -> Result<String, String> {
    let canonical = canonical_json(arguments);
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|error| format!("Failed to encode tool arguments: {error}"))?;
    Ok(hex_digest(&bytes))
}

pub fn operation_id(
    run_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    risk: ToolRisk,
    arguments_digest: &str,
) -> String {
    let identity = format!(
        "{run_id}\n{tool_call_id}\n{}\n{}\n{arguments_digest}",
        tool_name.trim().to_ascii_lowercase(),
        risk.as_str(),
    );
    format!("op_{}", hex_digest(identity.as_bytes()))
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolApproval {
    pub run_id: String,
    pub operation_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub risk: ToolRisk,
    pub arguments_digest: String,
    pub expires_at: u64,
}

#[allow(dead_code)]
impl ToolApproval {
    pub fn validate(
        &self,
        run_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        risk: ToolRisk,
        arguments_digest: &str,
        now: u64,
    ) -> Result<(), String> {
        if self.expires_at <= now {
            return Err("Tool approval has expired.".to_string());
        }
        if self.run_id != run_id
            || self.tool_call_id != tool_call_id
            || self.tool_name != tool_name
            || self.risk != risk
            || self.arguments_digest != arguments_digest
        {
            return Err("Tool approval does not match the requested operation.".to_string());
        }
        let expected = operation_id(run_id, tool_call_id, tool_name, risk, arguments_digest);
        if self.operation_id != expected {
            return Err("Tool approval operation identity is invalid.".to_string());
        }
        Ok(())
    }
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        value => value.clone(),
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolAuthorization {
    Allowed { capability: String, risk: ToolRisk },
    ApprovalRequired { capability: String, risk: ToolRisk },
}

#[derive(Clone, Debug)]
pub struct ToolBroker {
    granted: HashSet<String>,
    connectors: Vec<(String, String)>,
}

impl ToolBroker {
    pub fn new(
        granted: HashSet<String>,
        connectors: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self {
            granted,
            connectors: connectors.into_iter().collect(),
        }
    }

    pub fn authorize(&self, tool_name: &str) -> ToolAuthorization {
        let normalized = tool_name.trim().to_ascii_lowercase();
        let risk = Self::risk_for_tool(&normalized);
        let capability = self
            .connector_capability(&normalized)
            .or_else(|| builtin_capability(&normalized))
            .unwrap_or_else(|| format!("tool:{normalized}"));
        if capability == "runtime_internal" || self.granted.contains(&capability) {
            ToolAuthorization::Allowed { capability, risk }
        } else {
            ToolAuthorization::ApprovalRequired { capability, risk }
        }
    }

    pub fn risk_for_tool(tool_name: &str) -> ToolRisk {
        let normalized = tool_name.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "read" | "glob" | "grep" | "list" | "ls" | "todowrite" | "question" | "skill" => {
                return ToolRisk::Read
            }
            "edit" | "write" | "apply_patch" | "patch" | "bash" | "shell" | "git" => {
                return ToolRisk::Mutation;
            }
            _ => {}
        }
        let tokens = normalized
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>();
        if contains_any(
            &tokens,
            &[
                "secret",
                "secrets",
                "credential",
                "credentials",
                "token",
                "password",
                "env",
            ],
        ) {
            ToolRisk::CredentialSensitive
        } else if contains_any(
            &tokens,
            &["delete", "remove", "purge", "destroy", "terminate"],
        ) {
            ToolRisk::Destructive
        } else if contains_any(
            &tokens,
            &[
                "add",
                "assign",
                "comment",
                "create",
                "deploy",
                "edit",
                "execute",
                "link",
                "move",
                "patch",
                "apply",
                "restart",
                "run",
                "scale",
                "transition",
                "trigger",
                "update",
                "upload",
                "write",
            ],
        ) {
            ToolRisk::Mutation
        } else if contains_any(
            &tokens,
            &[
                "describe", "download", "fetch", "get", "list", "log", "logs", "read", "search",
                "status", "view",
            ],
        ) {
            ToolRisk::Read
        } else {
            ToolRisk::Unknown
        }
    }

    pub fn validate_mcp_call(
        tool_name: &str,
        exposed_tools: &[String],
        arguments: &serde_json::Value,
    ) -> Result<ToolRisk, String> {
        if tool_name.trim().is_empty() || !exposed_tools.iter().any(|name| name == tool_name) {
            return Err("MCP tool call was not present in the server tools list.".to_string());
        }
        if !arguments.is_object() {
            return Err("MCP tool arguments must be a JSON object.".to_string());
        }
        let argument_bytes = serde_json::to_vec(arguments)
            .map_err(|error| format!("Failed to serialize MCP tool arguments: {error}"))?;
        if argument_bytes.len() > MAX_TOOL_ARGUMENT_BYTES {
            return Err(format!(
                "MCP tool arguments exceeded the {MAX_TOOL_ARGUMENT_BYTES} byte limit."
            ));
        }
        Ok(Self::risk_for_tool(tool_name))
    }

    fn connector_capability(&self, tool_name: &str) -> Option<String> {
        self.connectors.iter().find_map(|(name, secret_id)| {
            let name = name.trim().to_ascii_lowercase();
            if secret_id.trim().is_empty() {
                return None;
            }
            let matches = tool_name == name
                || tool_name.starts_with(&format!("{name}_"))
                || tool_name.starts_with(&format!("{name}."))
                || tool_name.starts_with(&format!("{name}/"));
            matches.then(|| format!("external_tools:{}", secret_id.trim()))
        })
    }
}

fn contains_any(tokens: &[&str], values: &[&str]) -> bool {
    tokens.iter().any(|token| values.contains(token))
}

fn builtin_capability(tool_name: &str) -> Option<String> {
    let capability = match tool_name {
        "read" | "glob" | "grep" | "list" | "ls" => "workspace_read",
        "edit" | "write" | "apply_patch" | "patch" => "workspace_write",
        "bash" | "shell" => "shell",
        "git" => "git",
        "todowrite" | "question" | "skill" => "runtime_internal",
        _ => return None,
    };
    Some(capability.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorizes_builtin_tools_from_explicit_grants() {
        let broker = ToolBroker::new(HashSet::from(["workspace_read".to_string()]), []);

        assert_eq!(
            broker.authorize("read"),
            ToolAuthorization::Allowed {
                capability: "workspace_read".to_string(),
                risk: ToolRisk::Read,
            }
        );
        assert_eq!(
            broker.authorize("bash"),
            ToolAuthorization::ApprovalRequired {
                capability: "shell".to_string(),
                risk: ToolRisk::Mutation,
            }
        );
    }

    #[test]
    fn scopes_external_tools_to_the_connector_grant() {
        let broker = ToolBroker::new(
            HashSet::from(["external_tools:jira".to_string()]),
            [("atlassian".to_string(), "jira".to_string())],
        );

        assert!(matches!(
            broker.authorize("atlassian_jira_search"),
            ToolAuthorization::Allowed { .. }
        ));
        assert_eq!(
            broker.authorize("unknown_tool"),
            ToolAuthorization::ApprovalRequired {
                capability: "tool:unknown_tool".to_string(),
                risk: ToolRisk::Unknown,
            }
        );
    }

    #[test]
    fn validates_mcp_calls_against_discovered_tools_and_argument_limits() {
        let tools = vec!["jira_search".to_string()];
        assert!(ToolBroker::validate_mcp_call(
            "jira_search",
            &tools,
            &serde_json::json!({"jql": "project = APP"}),
        )
        .is_ok());
        assert!(
            ToolBroker::validate_mcp_call("jira_delete", &tools, &serde_json::json!({}),).is_err()
        );
        assert!(ToolBroker::validate_mcp_call(
            "jira_search",
            &tools,
            &serde_json::json!("not-an-object"),
        )
        .is_err());
    }

    #[test]
    fn classifies_operation_risk_conservatively() {
        assert_eq!(ToolBroker::risk_for_tool("jira_get_issue"), ToolRisk::Read);
        assert_eq!(
            ToolBroker::risk_for_tool("jira_transition_issue"),
            ToolRisk::Mutation
        );
        assert_eq!(
            ToolBroker::risk_for_tool("jira_delete_issue"),
            ToolRisk::Destructive
        );
        assert_eq!(
            ToolBroker::risk_for_tool("vault_get_secret"),
            ToolRisk::CredentialSensitive
        );
        assert_eq!(
            ToolBroker::risk_for_tool("custom_action"),
            ToolRisk::Unknown
        );
        assert_eq!(
            ToolBroker::risk_for_tool("kubernetes_resources_list"),
            ToolRisk::Read
        );
        assert_eq!(
            ToolBroker::risk_for_tool("kubernetes_resources_create"),
            ToolRisk::Mutation
        );
        assert_eq!(
            ToolBroker::risk_for_tool("kubernetes_resources_patch"),
            ToolRisk::Mutation
        );
        assert_eq!(
            ToolBroker::risk_for_tool("kubernetes_resources_delete"),
            ToolRisk::Destructive
        );
    }

    #[test]
    fn argument_digest_is_stable_across_object_key_order() {
        let first = serde_json::json!({"jql": "project = APP", "limit": 10});
        let second = serde_json::json!({"limit": 10, "jql": "project = APP"});

        assert_eq!(
            argument_digest(&first).unwrap(),
            argument_digest(&second).unwrap()
        );
    }

    #[test]
    fn display_context_exposes_only_allowlisted_safe_targets() {
        assert_eq!(
            tool_display_context("read", &serde_json::json!({"filePath": "src/main.rs"})),
            ToolDisplayContext {
                label: "Reading src/main.rs".to_string(),
                category: "files".to_string(),
                target: Some("src/main.rs".to_string()),
            }
        );
        let shell = tool_display_context(
            "bash",
            &serde_json::json!({"command": "deploy --token super-secret"}),
        );
        assert_eq!(shell.category, "commands");
        assert_eq!(shell.target, None);
        assert!(!shell.label.contains("token"));
        assert!(!shell.label.contains("secret"));
        let absolute = tool_display_context(
            "read",
            &serde_json::json!({"path": "/home/user/private.txt"}),
        );
        assert_eq!(absolute.target, None);
    }

    #[test]
    fn display_context_identifies_safe_connector_resources() {
        let jira = tool_display_context(
            "atlassian_jira_transition_issue",
            &serde_json::json!({"issue_key": "APP-123", "comment": "private"}),
        );
        assert_eq!(jira.category, "jira");
        assert_eq!(jira.target.as_deref(), Some("APP-123"));
        assert!(!jira.label.contains("private"));

        let openshift = tool_display_context(
            "ocp_restart_deployment",
            &serde_json::json!({"namespace": "payments", "name": "api"}),
        );
        assert_eq!(openshift.category, "kubernetes");
        assert_eq!(openshift.target.as_deref(), Some("api"));
    }

    #[test]
    fn approval_rejects_argument_replay_and_expiry() {
        let arguments = serde_json::json!({"issue": "APP-1", "status": "Done"});
        let digest = argument_digest(&arguments).unwrap();
        let approval = ToolApproval {
            run_id: "run-1".to_string(),
            operation_id: operation_id(
                "run-1",
                "call-1",
                "transition",
                ToolRisk::Mutation,
                &digest,
            ),
            tool_call_id: "call-1".to_string(),
            tool_name: "transition".to_string(),
            risk: ToolRisk::Mutation,
            arguments_digest: digest.clone(),
            expires_at: 100,
        };

        assert!(approval
            .validate(
                "run-1",
                "call-1",
                "transition",
                ToolRisk::Mutation,
                &digest,
                99
            )
            .is_ok());
        assert!(approval
            .validate(
                "run-1",
                "call-1",
                "transition",
                ToolRisk::Mutation,
                &argument_digest(&serde_json::json!({"issue": "APP-2"})).unwrap(),
                99,
            )
            .is_err());
        assert!(approval
            .validate(
                "run-1",
                "call-1",
                "transition",
                ToolRisk::Mutation,
                &digest,
                100,
            )
            .is_err());
    }
}
