//! Deterministic, secret-free examination of an Agent task before worker execution.

use crate::domain::governance::RuleFactsRecord;
use crate::domain::task_session::TaskSessionEnvelopeV1;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use url::Url;

pub const TASK_EXAMINATION_SCHEMA_VERSION: u32 = 1;
pub const TASK_EXAMINER_VERSION: &str = "agent-task-examiner-v1";
const MAX_EXAMINATION_ITEMS: usize = 64;
const MAX_CONNECTOR_TOOLS: usize = 128;
const MAX_DISCOVERED_TOOLS: usize = 1024;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskExaminationStatus {
    Ready,
    #[default]
    Blocked,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskCapabilityRecord {
    pub capability: String,
    pub provider: String,
    pub connector_id: Option<String>,
    pub discovery: String,
    pub granted: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskResourceReference {
    pub kind: String,
    pub value: String,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorDiscoveryStatus {
    #[default]
    Declared,
    Available,
    Unavailable,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoveredToolCapability {
    pub name: String,
    pub risk: String,
    pub argument_names: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectorCapabilitySnapshot {
    pub connector_id: String,
    pub status: ConnectorDiscoveryStatus,
    pub tools: Vec<DiscoveredToolCapability>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskExaminationRecord {
    pub schema_version: u32,
    pub examiner_version: String,
    pub contract_digest: String,
    pub status: TaskExaminationStatus,
    pub objectives: Vec<String>,
    pub resources: Vec<TaskResourceReference>,
    pub capability_catalog: Vec<TaskCapabilityRecord>,
    #[serde(default)]
    pub connector_capabilities: Vec<ConnectorCapabilitySnapshot>,
    pub required_capabilities: Vec<String>,
    pub unresolved_requirements: Vec<String>,
    pub mutations: Vec<String>,
    pub approval_boundaries: Vec<String>,
    pub warnings: Vec<String>,
}

impl TaskExaminationRecord {
    pub fn validate(&self, contract_digest: &str) -> Result<(), String> {
        if self.schema_version == 0 {
            // Retained manifests created before task examination remain readable.
            return Ok(());
        }
        if self.schema_version != TASK_EXAMINATION_SCHEMA_VERSION
            || self.examiner_version != TASK_EXAMINER_VERSION
            || self.contract_digest != contract_digest
        {
            return Err("Task Examination is stale or unsupported.".to_string());
        }
        if [
            self.objectives.len(),
            self.resources.len(),
            self.capability_catalog.len(),
            self.connector_capabilities.len(),
            self.required_capabilities.len(),
            self.unresolved_requirements.len(),
            self.mutations.len(),
            self.approval_boundaries.len(),
            self.warnings.len(),
        ]
        .into_iter()
        .any(|count| count > MAX_EXAMINATION_ITEMS)
        {
            return Err("Task Examination exceeds its bounded limits.".to_string());
        }
        if self
            .connector_capabilities
            .iter()
            .map(|connector| connector.tools.len())
            .sum::<usize>()
            > MAX_DISCOVERED_TOOLS
            || self.connector_capabilities.iter().any(|connector| {
                !canonical_inventory_name(&connector.connector_id, false)
                    || connector.tools.len() > MAX_CONNECTOR_TOOLS
                    || connector.warnings.len() > MAX_EXAMINATION_ITEMS
                    || connector.error.as_ref().is_some_and(|error| {
                        error.len() > 160 || error.chars().any(char::is_control)
                    })
                    || connector.tools.iter().any(|tool| {
                        !canonical_inventory_name(&tool.name, true)
                            || !matches!(
                                tool.risk.as_str(),
                                "read"
                                    | "mutation"
                                    | "destructive"
                                    | "credential_sensitive"
                                    | "unknown"
                            )
                            || tool.argument_names.len() > MAX_EXAMINATION_ITEMS
                            || tool
                                .argument_names
                                .iter()
                                .any(|name| !canonical_inventory_name(name, true))
                    })
            })
        {
            return Err("Task Examination connector inventory is invalid.".to_string());
        }
        Ok(())
    }
}

fn canonical_inventory_name(value: &str, namespaced: bool) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.contains("..")
        && !value.starts_with('/')
        && !value.ends_with('/')
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'_' | b'-' | b'.')
                || (namespaced && matches!(byte, b'/' | b':'))
        })
}

/// Examines any execution contract using only immutable task inputs and durable authorities.
/// Domain-specific facts enrich this record but are not required for unknown future connectors.
pub fn examine_task(
    contract: &Value,
    contract_digest: &str,
    envelope: &TaskSessionEnvelopeV1,
    granted_capabilities: &HashSet<&str>,
    rule_facts: &RuleFactsRecord,
    connector_capabilities: &[ConnectorCapabilitySnapshot],
) -> TaskExaminationRecord {
    let mut objectives = BTreeSet::new();
    insert_path_string(contract, &["objective", "summary"], &mut objectives);
    insert_path_strings(
        contract,
        &["objective", "success_criteria"],
        &mut objectives,
    );
    if let Some(steps) = contract.get("workflow").and_then(Value::as_array) {
        for step in steps
            .iter()
            .filter(|step| step.get("status").and_then(Value::as_str) != Some("completed"))
        {
            insert_path_string(step, &["title"], &mut objectives);
        }
    }

    let mut resources = resource_references(contract);
    resources.truncate(MAX_EXAMINATION_ITEMS);

    let mut required_capabilities = envelope.requested_capabilities.clone();
    required_capabilities.sort();
    required_capabilities.dedup();
    let capability_catalog = required_capabilities
        .iter()
        .map(|capability| {
            let connector_id = capability
                .strip_prefix("external_tools:")
                .map(str::to_string);
            let discovery = connector_id
                .as_deref()
                .and_then(|connector_id| {
                    connector_capabilities
                        .iter()
                        .find(|snapshot| snapshot.connector_id == connector_id)
                })
                .map(|snapshot| match snapshot.status {
                    ConnectorDiscoveryStatus::Declared => "declared",
                    ConnectorDiscoveryStatus::Available => "live",
                    ConnectorDiscoveryStatus::Unavailable => "unavailable",
                })
                .unwrap_or("declared");
            TaskCapabilityRecord {
                capability: capability.clone(),
                provider: if connector_id.is_some() {
                    "mcp_connector".to_string()
                } else {
                    "workspace".to_string()
                },
                connector_id,
                discovery: discovery.to_string(),
                granted: granted_capabilities.contains(capability.as_str()),
            }
        })
        .collect::<Vec<_>>();
    let mut unresolved_requirements = capability_catalog
        .iter()
        .filter(|capability| !capability.granted)
        .map(|capability| format!("Capability '{}' is not granted.", capability.capability))
        .collect::<Vec<_>>();
    unresolved_requirements.extend(
        connector_capabilities
            .iter()
            .filter(|snapshot| snapshot.status == ConnectorDiscoveryStatus::Unavailable)
            .map(|snapshot| {
                format!(
                    "Connector '{}' did not expose a usable live tool inventory.",
                    snapshot.connector_id
                )
            }),
    );
    unresolved_requirements.sort();
    unresolved_requirements.dedup();

    let mut mutations = BTreeSet::new();
    if contract_bool(contract, &["constraints", "may_modify_files"]) {
        mutations.insert("workspace_files".to_string());
    }
    if contract_bool(contract, &["constraints", "may_update_jira"]) {
        mutations.insert("jira".to_string());
    }

    let branch = contract_path(contract, &["repository", "branch"]).and_then(Value::as_str);
    let mut approval_boundaries = BTreeSet::new();
    if let Some(branch) = branch {
        for policy in &rule_facts.protected_branches {
            if policy.approval_required
                && policy.branches.iter().any(|candidate| candidate == branch)
            {
                approval_boundaries.insert(format!("protected_branch:{branch}"));
            }
        }
    }

    let mut warnings = BTreeSet::new();
    if capability_catalog
        .iter()
        .any(|capability| capability.connector_id.is_some() && capability.discovery == "declared")
    {
        warnings.insert(
            "Connector capabilities are declared at examination time; live MCP tool schemas are discovered by the fenced worker."
                .to_string(),
        );
    }
    warnings.extend(rule_facts.warnings.iter().cloned());
    warnings.extend(
        connector_capabilities
            .iter()
            .flat_map(|connector| connector.warnings.iter().cloned()),
    );

    TaskExaminationRecord {
        schema_version: TASK_EXAMINATION_SCHEMA_VERSION,
        examiner_version: TASK_EXAMINER_VERSION.to_string(),
        contract_digest: contract_digest.to_string(),
        status: if unresolved_requirements.is_empty() {
            TaskExaminationStatus::Ready
        } else {
            TaskExaminationStatus::Blocked
        },
        objectives: objectives.into_iter().take(MAX_EXAMINATION_ITEMS).collect(),
        resources,
        capability_catalog,
        connector_capabilities: connector_capabilities.to_vec(),
        required_capabilities,
        unresolved_requirements,
        mutations: mutations.into_iter().collect(),
        approval_boundaries: approval_boundaries.into_iter().collect(),
        warnings: warnings.into_iter().take(MAX_EXAMINATION_ITEMS).collect(),
    }
}

fn contract_path<'a>(contract: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(contract, |value, key| value.get(*key))
}

fn contract_bool(contract: &Value, path: &[&str]) -> bool {
    contract_path(contract, path)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn insert_path_string(value: &Value, path: &[&str], target: &mut BTreeSet<String>) {
    if let Some(value) = contract_path(value, path)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        target.insert(value.chars().take(512).collect());
    }
}

fn insert_path_strings(value: &Value, path: &[&str], target: &mut BTreeSet<String>) {
    if let Some(values) = contract_path(value, path).and_then(Value::as_array) {
        for value in values.iter().filter_map(Value::as_str) {
            let value = value.trim();
            if !value.is_empty() {
                target.insert(value.chars().take(512).collect());
            }
        }
    }
}

fn resource_references(contract: &Value) -> Vec<TaskResourceReference> {
    let mut resources = BTreeSet::new();
    for (kind, source, path) in [
        ("ticket_key", "ticket.key", &["ticket", "key"][..]),
        ("ticket_url", "ticket.url", &["ticket", "url"][..]),
        (
            "repository_root",
            "repository.root_path",
            &["repository", "root_path"][..],
        ),
        (
            "repository_branch",
            "repository.branch",
            &["repository", "branch"][..],
        ),
    ] {
        if let Some(value) = contract_path(contract, path)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            resources.insert((
                kind.to_string(),
                if kind.ends_with("_url") {
                    secret_free_url(value)
                } else {
                    value.to_string()
                },
                source.to_string(),
            ));
        }
    }
    let url = Regex::new(r#"https?://[^\s<>\"']+"#).expect("static URL regex");
    collect_urls(contract, "$", &url, &mut resources);
    resources
        .into_iter()
        .take(MAX_EXAMINATION_ITEMS)
        .map(|(kind, value, source)| TaskResourceReference {
            kind,
            value,
            source,
        })
        .collect()
}

fn collect_urls(
    value: &Value,
    path: &str,
    regex: &Regex,
    resources: &mut BTreeSet<(String, String, String)>,
) {
    if resources.len() >= MAX_EXAMINATION_ITEMS {
        return;
    }
    match value {
        Value::String(text) => {
            for matched in regex.find_iter(text) {
                resources.insert((
                    "url".to_string(),
                    secret_free_url(matched.as_str().trim_end_matches(['.', ',', ')'])),
                    path.to_string(),
                ));
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_urls(value, &format!("{path}[{index}]"), regex, resources);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                collect_urls(value, &format!("{path}.{key}"), regex, resources);
            }
        }
        _ => {}
    }
}

fn secret_free_url(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return value
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::governance::ProtectedBranchRulePolicy;
    use crate::domain::task_session::TaskSessionKind;

    fn envelope() -> TaskSessionEnvelopeV1 {
        TaskSessionEnvelopeV1 {
            workspace_id: "workspace-1".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: None,
            conversation_id: Some("conversation-1".to_string()),
            execution_run_id: Some("run-1".to_string()),
            context_digest: "sha256:contract".to_string(),
            runtime_profile_id: "agent-profile".to_string(),
            model: "openai/model".to_string(),
            connector_ids: vec!["future-system".to_string()],
            requested_capabilities: vec![
                "external_tools:future-system".to_string(),
                "workspace_read".to_string(),
            ],
            prompt_template_version: "agent-task-v1".to_string(),
            context_revision: Some("1".to_string()),
            rules_revision: Some("rules".to_string()),
            skills_revision: Some("skills".to_string()),
        }
    }

    #[test]
    fn examines_unknown_connector_without_domain_hard_coding() {
        let contract = serde_json::json!({
            "objective": {
                "summary": "Examine a future system",
                "success_criteria": ["Evidence is recorded"]
            },
            "task_context": {
                "description": "Read https://user:secret@future.example/items/42?token=hidden#part",
                "execution_detail": ""
            },
            "workflow": [{ "title": "Inspect item", "status": "current" }],
            "repository": { "root_path": null, "branch": null },
            "constraints": { "may_modify_files": false, "may_update_jira": false }
        });
        let granted = HashSet::from(["external_tools:future-system", "workspace_read"]);
        let connector_capabilities = vec![ConnectorCapabilitySnapshot {
            connector_id: "future-system".to_string(),
            status: ConnectorDiscoveryStatus::Available,
            tools: vec![DiscoveredToolCapability {
                name: "future_search".to_string(),
                risk: "read".to_string(),
                argument_names: vec!["query".to_string()],
            }],
            error: None,
            warnings: Vec::new(),
        }];
        let examined = examine_task(
            &contract,
            "sha256:contract",
            &envelope(),
            &granted,
            &RuleFactsRecord::default(),
            &connector_capabilities,
        );

        assert_eq!(examined.status, TaskExaminationStatus::Ready);
        assert!(examined.capability_catalog.iter().any(|capability| {
            capability.connector_id.as_deref() == Some("future-system")
                && capability.granted
                && capability.discovery == "live"
        }));
        assert_eq!(examined.connector_capabilities[0].tools.len(), 1);
        assert!(examined
            .resources
            .iter()
            .any(|resource| resource.value == "https://future.example/items/42"));
        let encoded = serde_json::to_string(&examined).expect("examination JSON");
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("hidden"));
    }

    #[test]
    fn blocks_missing_grant_and_applies_protected_branch_policy() {
        let contract = serde_json::json!({
            "objective": { "summary": "Update deployment", "success_criteria": [] },
            "workflow": [],
            "repository": { "root_path": "/repo", "branch": "eks-green" },
            "constraints": { "may_modify_files": true, "may_update_jira": false }
        });
        let facts = RuleFactsRecord {
            protected_branches: vec![ProtectedBranchRulePolicy {
                branches: vec!["eks-green".to_string()],
                operations: vec!["modify".to_string()],
                approval_required: true,
            }],
            ..Default::default()
        };
        let examined = examine_task(
            &contract,
            "sha256:contract",
            &envelope(),
            &HashSet::from(["workspace_read"]),
            &facts,
            &[],
        );

        assert_eq!(examined.status, TaskExaminationStatus::Blocked);
        assert_eq!(examined.mutations, vec!["workspace_files"]);
        assert_eq!(
            examined.approval_boundaries,
            vec!["protected_branch:eks-green"]
        );
        assert_eq!(examined.unresolved_requirements.len(), 1);
    }
}
