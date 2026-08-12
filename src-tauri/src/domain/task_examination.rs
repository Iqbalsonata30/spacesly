//! Deterministic, secret-free examination of an Agent task before worker execution.

use crate::domain::governance::RuleFactsRecord;
use crate::domain::task_session::{AgentTaskObjectiveCheckpoint, TaskSessionEnvelopeV1};
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
pub struct ConnectorCapabilityMapping {
    pub connector_id: String,
    pub reason: String,
    pub planned_tools: Vec<String>,
    pub verified_tools: Vec<String>,
    pub status: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SemanticPlannerEvidence {
    pub status: String,
    pub planner_version: String,
    pub model: Option<String>,
    pub objective_count: usize,
}

/// Deterministic, secret-free repository scope selected before worker execution.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryResolutionRecord {
    pub schema_version: u32,
    pub status: String,
    pub repository_id: Option<String>,
    pub remote_url: Option<String>,
    pub local_path: Option<String>,
    pub backend_path: Option<String>,
    pub frontend_path: Option<String>,
    pub source: String,
    pub source_line: u32,
    pub reason: String,
}

/// Exact deployment environment selected from a user-defined Rules table.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeploymentTargetResolutionRecord {
    pub schema_version: u32,
    pub status: String,
    pub matched_label: Option<String>,
    pub target: Option<String>,
    pub branch: Option<String>,
    pub namespace: Option<String>,
    pub source: String,
    pub source_line: u32,
    pub reason: String,
}

/// Secret-free connector configuration decision compiled before worker execution.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectorConfigurationPreflightRecord {
    pub schema_version: u32,
    pub connector_id: String,
    pub connector_type: Option<String>,
    pub status: String,
    pub base_url: Option<String>,
    pub required_operations: Vec<String>,
    pub verified_tools: Vec<String>,
    pub source: String,
    pub source_line: u32,
    pub reason: String,
}

/// Task-bound, secret-free proof requirements compiled from user Rules.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerificationPolicyBindingRecord {
    pub schema_version: u32,
    pub policy_id: String,
    pub connector_id: String,
    pub status: String,
    pub matched_labels: Vec<String>,
    pub required_operations: Vec<String>,
    pub verified_tools: Vec<String>,
    pub source: String,
    pub source_line: u32,
    pub reason: String,
}

/// Secret-free explanation of contradictory authoritative facts relevant to one task.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuleContradictionRecord {
    pub schema_version: u32,
    pub domain: String,
    pub key: String,
    pub source_references: Vec<String>,
    pub reason: String,
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
    #[serde(default)]
    pub capability_mappings: Vec<ConnectorCapabilityMapping>,
    #[serde(default)]
    pub semantic_planner: Option<SemanticPlannerEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_resolution: Option<RepositoryResolutionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment_target_resolution: Option<DeploymentTargetResolutionRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connector_configuration_preflights: Vec<ConnectorConfigurationPreflightRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verification_policy_bindings: Vec<VerificationPolicyBindingRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_contradictions: Vec<RuleContradictionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_repair: Option<CapabilityRepairGuidance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objective_checkpoints: Vec<AgentTaskObjectiveCheckpoint>,
    pub required_capabilities: Vec<String>,
    pub unresolved_requirements: Vec<String>,
    pub mutations: Vec<String>,
    pub approval_boundaries: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilityRepairGuidance {
    pub schema_version: u32,
    pub connector_id: String,
    pub failed_tool: String,
    pub allowed_alternatives: Vec<String>,
    pub reason: String,
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
            self.capability_mappings.len(),
            usize::from(self.repository_resolution.is_some()),
            usize::from(self.deployment_target_resolution.is_some()),
            self.connector_configuration_preflights.len(),
            self.objective_checkpoints.len(),
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
        if self.capability_mappings.iter().any(|mapping| {
            !canonical_inventory_name(&mapping.connector_id, false)
                || mapping.planned_tools.len() > MAX_EXAMINATION_ITEMS
                || mapping.verified_tools.len() > MAX_EXAMINATION_ITEMS
                || !matches!(
                    mapping.reason.as_str(),
                    "explicit_domain"
                        | "live_operation"
                        | "configured_intent"
                        | "structured_constraint"
                )
                || !matches!(
                    mapping.status.as_str(),
                    "declared" | "connector_verified" | "tools_verified" | "stale"
                )
                || mapping
                    .planned_tools
                    .iter()
                    .chain(mapping.verified_tools.iter())
                    .any(|tool| !canonical_inventory_name(tool, true))
        }) {
            return Err("Task Examination capability mapping is invalid.".to_string());
        }
        if self.semantic_planner.as_ref().is_some_and(|planner| {
            !matches!(planner.status.as_str(), "model" | "fallback")
                || planner.planner_version.trim().is_empty()
                || planner.planner_version.len() > 128
                || planner.model.as_ref().is_some_and(|model| {
                    model.trim().is_empty()
                        || model.len() > 256
                        || model.chars().any(char::is_control)
                })
                || planner.objective_count == 0
                || planner.objective_count > 8
        }) {
            return Err("Task Examination semantic planner evidence is invalid.".to_string());
        }
        if self
            .repository_resolution
            .as_ref()
            .is_some_and(|resolution| {
                resolution.schema_version != 1
                    || !matches!(
                        resolution.status.as_str(),
                        "resolved"
                            | "ambiguous"
                            | "conflict"
                            | "missing_checkout"
                            | "invalid_checkout"
                            | "outside_workspace"
                    )
                    || resolution.repository_id.as_ref().is_some_and(|value| {
                        value.trim().is_empty()
                            || value.len() > 253
                            || value.chars().any(char::is_control)
                    })
                    || resolution.remote_url.as_ref().is_some_and(|value| {
                        value.trim().is_empty()
                            || value.len() > 2_000
                            || value.chars().any(char::is_control)
                    })
                    || resolution.source.trim().is_empty()
                    || resolution.source.len() > 128
                    || resolution.reason.trim().is_empty()
                    || resolution.reason.len() > 512
                    || resolution
                        .local_path
                        .as_ref()
                        .is_some_and(|path| path.is_empty() || path.len() > 4_096)
                    || resolution
                        .backend_path
                        .iter()
                        .chain(resolution.frontend_path.iter())
                        .any(|path| path.is_empty() || path.len() > 1_024)
            })
        {
            return Err("Task Examination repository resolution is invalid.".to_string());
        }
        if self
            .deployment_target_resolution
            .as_ref()
            .is_some_and(|resolution| {
                resolution.schema_version != 1
                    || !matches!(
                        resolution.status.as_str(),
                        "resolved" | "ambiguous" | "invalid"
                    )
                    || resolution.source.trim().is_empty()
                    || resolution.source.len() > 128
                    || resolution.reason.trim().is_empty()
                    || resolution.reason.len() > 512
                    || [
                        resolution.matched_label.as_ref(),
                        resolution.target.as_ref(),
                        resolution.branch.as_ref(),
                        resolution.namespace.as_ref(),
                    ]
                    .into_iter()
                    .flatten()
                    .any(|value| {
                        value.trim().is_empty()
                            || value.len() > 253
                            || value.chars().any(char::is_control)
                    })
                    || (resolution.status == "resolved"
                        && (resolution.matched_label.is_none()
                            || resolution.target.is_none()
                            || resolution.branch.is_none()
                            || resolution.namespace.is_none()))
            })
        {
            return Err("Task Examination deployment target resolution is invalid.".to_string());
        }
        if self
            .connector_configuration_preflights
            .iter()
            .any(|record| {
                record.schema_version != 1
                    || !canonical_inventory_name(&record.connector_id, false)
                    || !matches!(
                        record.status.as_str(),
                        "ready"
                            | "missing_rule"
                            | "invalid_rule"
                            | "missing_configuration"
                            | "url_mismatch"
                            | "connector_unavailable"
                            | "missing_operations"
                            | "ambiguous_operation"
                    )
                    || record
                        .connector_type
                        .as_ref()
                        .is_some_and(|value| !canonical_inventory_name(value, false))
                    || record.base_url.as_ref().is_some_and(|value| {
                        value.is_empty()
                            || value.len() > 2_000
                            || value.chars().any(char::is_control)
                    })
                    || record.required_operations.len() > MAX_EXAMINATION_ITEMS
                    || record.verified_tools.len() > MAX_EXAMINATION_ITEMS
                    || record
                        .required_operations
                        .iter()
                        .chain(record.verified_tools.iter())
                        .any(|operation| !canonical_inventory_name(operation, true))
                    || record.source.trim().is_empty()
                    || record.source.len() > 128
                    || record.reason.trim().is_empty()
                    || record.reason.len() > 512
            })
        {
            return Err(
                "Task Examination connector configuration preflight is invalid.".to_string(),
            );
        }
        if self.verification_policy_bindings.iter().any(|record| {
            record.schema_version != 1
                || !canonical_inventory_name(&record.policy_id, false)
                || !canonical_inventory_name(&record.connector_id, false)
                || !matches!(
                    record.status.as_str(),
                    "ready" | "invalid_rule" | "missing_operations"
                )
                || record.matched_labels.len() > MAX_EXAMINATION_ITEMS
                || (record.status == "ready" && record.required_operations.is_empty())
                || record.required_operations.len() > MAX_EXAMINATION_ITEMS
                || (record.status == "ready"
                    && record.verified_tools.len() != record.required_operations.len())
                || record
                    .required_operations
                    .iter()
                    .chain(record.verified_tools.iter())
                    .any(|value| !canonical_inventory_name(value, true))
                || record.matched_labels.iter().any(|label| {
                    label.trim().is_empty()
                        || label.len() > 128
                        || label.chars().any(char::is_control)
                })
                || record.source.trim().is_empty()
                || record.source.len() > 128
                || record.reason.trim().is_empty()
                || record.reason.len() > 512
        }) {
            return Err("Task Examination verification policy binding is invalid.".to_string());
        }
        if self.rule_contradictions.len() > MAX_EXAMINATION_ITEMS
            || self.rule_contradictions.iter().any(|record| {
                record.schema_version != 1
                    || !matches!(
                        record.domain.as_str(),
                        "repository" | "deployment_target" | "connector" | "verification"
                    )
                    || record.key.trim().is_empty()
                    || record.key.len() > 128
                    || record.key.chars().any(char::is_control)
                    || record.source_references.len() < 2
                    || record.source_references.len() > MAX_EXAMINATION_ITEMS
                    || record.source_references.iter().any(|source| {
                        source.trim().is_empty()
                            || source.len() > 160
                            || source.chars().any(char::is_control)
                    })
                    || record.reason.trim().is_empty()
                    || record.reason.len() > 512
            })
        {
            return Err("Task Examination Rule contradiction record is invalid.".to_string());
        }
        if self.runtime_repair.as_ref().is_some_and(|repair| {
            repair.schema_version != 1
                || !canonical_inventory_name(&repair.connector_id, false)
                || !canonical_inventory_name(&repair.failed_tool, true)
                || repair.allowed_alternatives.is_empty()
                || repair.allowed_alternatives.len() > 5
                || repair
                    .allowed_alternatives
                    .iter()
                    .any(|tool| !canonical_inventory_name(tool, true))
                || repair
                    .allowed_alternatives
                    .iter()
                    .any(|tool| tool == &repair.failed_tool)
                || !self.connector_capabilities.iter().any(|connector| {
                    connector.connector_id == repair.connector_id
                        && connector.status == ConnectorDiscoveryStatus::Available
                        && repair.allowed_alternatives.iter().all(|alternative| {
                            connector
                                .tools
                                .iter()
                                .any(|tool| tool.name == *alternative && tool.risk == "read")
                        })
                })
                || repair.reason.trim().is_empty()
                || repair.reason.len() > 256
        }) {
            return Err("Task Examination runtime repair guidance is invalid.".to_string());
        }
        let mut checkpoint_ids = HashSet::new();
        if self.objective_checkpoints.iter().any(|checkpoint| {
            let mut tool_call_ids = HashSet::new();
            !canonical_inventory_name(&checkpoint.objective_id, true)
                || !checkpoint_ids.insert(checkpoint.objective_id.as_str())
                || checkpoint.evidence.is_empty()
                || checkpoint.evidence.len() > 12
                || checkpoint
                    .evidence
                    .iter()
                    .any(|evidence| evidence.trim().is_empty() || evidence.len() > 2_000)
                || checkpoint.tool_receipts.len() > 32
                || checkpoint.tool_receipts.iter().any(|receipt| {
                    receipt.tool_call_id.trim().is_empty()
                        || receipt.tool_call_id.len() > 256
                        || !tool_call_ids.insert(receipt.tool_call_id.as_str())
                        || !canonical_inventory_name(&receipt.tool_name, true)
                        || !matches!(
                            receipt.risk.as_str(),
                            "read"
                                | "mutation"
                                | "destructive"
                                | "credential_sensitive"
                                | "unknown"
                        )
                        || receipt.arguments_digest.len() != 64
                        || !receipt
                            .arguments_digest
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit())
                })
                || checkpoint.source_attempt_id == 0
                || checkpoint.recorded_at == 0
        }) {
            return Err("Task Examination objective checkpoints are invalid.".to_string());
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
    if let Some(semantic_objectives) = contract
        .get("semantic_plan")
        .and_then(|plan| plan.get("objectives"))
        .and_then(Value::as_array)
    {
        for objective in semantic_objectives.iter().take(8) {
            insert_path_string(objective, &["summary"], &mut objectives);
            insert_path_string(objective, &["success_evidence"], &mut objectives);
        }
    }
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
    let (capability_mappings, mapping_requirements) =
        capability_mappings(contract, envelope, connector_capabilities);
    unresolved_requirements.extend(mapping_requirements);
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
    let semantic_planner = semantic_planner_evidence(contract);

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
        capability_mappings,
        semantic_planner,
        repository_resolution: None,
        deployment_target_resolution: None,
        connector_configuration_preflights: Vec::new(),
        verification_policy_bindings: Vec::new(),
        rule_contradictions: Vec::new(),
        runtime_repair: None,
        objective_checkpoints: Vec::new(),
        required_capabilities,
        unresolved_requirements,
        mutations: mutations.into_iter().collect(),
        approval_boundaries: approval_boundaries.into_iter().collect(),
        warnings: warnings.into_iter().take(MAX_EXAMINATION_ITEMS).collect(),
    }
}

fn semantic_planner_evidence(contract: &Value) -> Option<SemanticPlannerEvidence> {
    let plan = contract.get("semantic_plan")?;
    let status = plan.get("status")?.as_str()?;
    let planner_version = plan.get("planner_version")?.as_str()?;
    let objectives = plan.get("objectives")?.as_array()?;
    if !matches!(status, "model" | "fallback") || objectives.is_empty() || objectives.len() > 8 {
        return None;
    }
    Some(SemanticPlannerEvidence {
        status: status.to_string(),
        planner_version: planner_version.to_string(),
        model: plan
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        objective_count: objectives.len(),
    })
}

fn capability_mappings(
    contract: &Value,
    envelope: &TaskSessionEnvelopeV1,
    snapshots: &[ConnectorCapabilitySnapshot],
) -> (Vec<ConnectorCapabilityMapping>, Vec<String>) {
    let Some(plan) = contract.get("capability_plan") else {
        return (Vec::new(), Vec::new());
    };
    let mut unresolved = plan
        .get("unresolved_domains")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|domain| canonical_inventory_name(domain, true).then_some(domain))
        .map(|domain| format!("No configured connector satisfies domain '{domain}'."))
        .collect::<Vec<_>>();
    let mappings = plan
        .get("connectors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_EXAMINATION_ITEMS)
        .filter_map(|entry| {
            let connector_id = entry.get("connector_id")?.as_str()?;
            let reason = entry.get("reason")?.as_str()?;
            if !canonical_inventory_name(connector_id, false)
                || !matches!(
                    reason,
                    "explicit_domain"
                        | "live_operation"
                        | "configured_intent"
                        | "structured_constraint"
                )
            {
                return None;
            }
            let mut planned_tools = entry
                .get("matched_tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|tool| canonical_inventory_name(tool, true))
                .map(str::to_string)
                .take(MAX_EXAMINATION_ITEMS)
                .collect::<Vec<_>>();
            planned_tools.sort();
            planned_tools.dedup();
            if !envelope.connector_ids.iter().any(|id| id == connector_id) {
                unresolved.push(format!(
                    "Capability plan references unassigned connector '{connector_id}'."
                ));
            }
            let snapshot = snapshots
                .iter()
                .find(|snapshot| snapshot.connector_id == connector_id);
            let live_tools = snapshot
                .filter(|snapshot| snapshot.status == ConnectorDiscoveryStatus::Available)
                .map(|snapshot| {
                    snapshot
                        .tools
                        .iter()
                        .map(|tool| tool.name.as_str())
                        .collect::<HashSet<_>>()
                });
            let verified_tools = planned_tools
                .iter()
                .filter(|tool| {
                    live_tools
                        .as_ref()
                        .is_some_and(|live| live.contains(tool.as_str()))
                })
                .cloned()
                .collect::<Vec<_>>();
            for missing in planned_tools
                .iter()
                .filter(|tool| !verified_tools.contains(tool))
            {
                if snapshot.is_some() {
                    unresolved.push(format!(
                        "Connector '{connector_id}' no longer exposes planned tool '{missing}'."
                    ));
                }
            }
            let status = match snapshot {
                None => "declared",
                Some(snapshot) if snapshot.status != ConnectorDiscoveryStatus::Available => "stale",
                Some(_)
                    if planned_tools.len() == verified_tools.len() && !planned_tools.is_empty() =>
                {
                    "tools_verified"
                }
                Some(_) if planned_tools.is_empty() => "connector_verified",
                Some(_) => "stale",
            };
            Some(ConnectorCapabilityMapping {
                connector_id: connector_id.to_string(),
                reason: reason.to_string(),
                planned_tools,
                verified_tools,
                status: status.to_string(),
            })
        })
        .collect::<Vec<_>>();
    (mappings, unresolved)
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
            "semantic_plan": {
                "status": "model",
                "planner_version": "agent-semantic-planner-v1",
                "model": "openai/test",
                "objectives": [{
                    "summary": "Inspect the future system item",
                    "success_evidence": "The item is returned",
                    "operation_hints": ["search item"],
                    "resource_hints": ["item"],
                    "mutation_expected": false
                }]
            },
            "capability_plan": {
                "schema_version": 1,
                "planner_version": "agent-capability-plan-v1",
                "connectors": [{
                    "connector_id": "future-system",
                    "reason": "live_operation",
                    "matched_domains": [],
                    "matched_intents": [],
                    "matched_tools": ["future_search"]
                }],
                "unresolved_domains": []
            },
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
        assert_eq!(examined.capability_mappings[0].status, "tools_verified");
        assert_eq!(
            examined
                .semantic_planner
                .as_ref()
                .map(|planner| planner.status.as_str()),
            Some("model")
        );
        assert_eq!(
            examined.capability_mappings[0].verified_tools,
            vec!["future_search"]
        );
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

    #[test]
    fn blocks_when_persisted_tool_plan_is_stale_against_live_inventory() {
        let contract = serde_json::json!({
            "objective": { "summary": "Publish artifact", "success_criteria": [] },
            "workflow": [],
            "repository": { "root_path": null, "branch": null },
            "constraints": { "may_modify_files": false, "may_update_jira": false },
            "capability_plan": {
                "connectors": [{
                    "connector_id": "future-system",
                    "reason": "live_operation",
                    "matched_tools": ["publish_artifact"]
                }],
                "unresolved_domains": []
            }
        });
        let snapshots = vec![ConnectorCapabilitySnapshot {
            connector_id: "future-system".to_string(),
            status: ConnectorDiscoveryStatus::Available,
            tools: vec![DiscoveredToolCapability {
                name: "read_artifact".to_string(),
                risk: "read".to_string(),
                argument_names: Vec::new(),
            }],
            error: None,
            warnings: Vec::new(),
        }];
        let examined = examine_task(
            &contract,
            "sha256:contract",
            &envelope(),
            &HashSet::from(["external_tools:future-system", "workspace_read"]),
            &RuleFactsRecord::default(),
            &snapshots,
        );

        assert_eq!(examined.status, TaskExaminationStatus::Blocked);
        assert_eq!(examined.capability_mappings[0].status, "stale");
        assert!(examined.unresolved_requirements[0].contains("no longer exposes"));
    }
}
