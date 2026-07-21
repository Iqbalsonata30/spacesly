use std::collections::HashSet;

const MAX_TOOL_ARGUMENT_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolAuthorization {
    Allowed { capability: String },
    ApprovalRequired { capability: String },
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
        let capability = self
            .connector_capability(&normalized)
            .or_else(|| builtin_capability(&normalized))
            .unwrap_or_else(|| format!("tool:{normalized}"));
        if capability == "runtime_internal" || self.granted.contains(&capability) {
            ToolAuthorization::Allowed { capability }
        } else {
            ToolAuthorization::ApprovalRequired { capability }
        }
    }

    pub fn validate_mcp_call(
        tool_name: &str,
        exposed_tools: &[String],
        arguments: &serde_json::Value,
    ) -> Result<(), String> {
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
        Ok(())
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
            }
        );
        assert_eq!(
            broker.authorize("bash"),
            ToolAuthorization::ApprovalRequired {
                capability: "shell".to_string(),
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
}
