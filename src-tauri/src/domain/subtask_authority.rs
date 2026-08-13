//! Provider-neutral authority contracts for future isolated Agent subtasks.
//!
//! This module prepares durable contracts only. It intentionally does not dispatch another
//! worker; execution must remain disabled until the scheduler owns independent subtask attempts
//! and fences.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const SUBTASK_CONTRACT_SCHEMA_VERSION: u32 = 1;
const MAX_SUBTASKS: usize = 8;
const TOTAL_WALL_CLOCK_SECONDS: u64 = 3_600;
const TOTAL_TOOL_CALLS: u32 = 64;
const TOTAL_MUTATION_CALLS: u32 = 8;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubtaskBudget {
    pub wall_clock_seconds: u64,
    pub max_tool_calls: u32,
    pub max_mutation_calls: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreparedSubtaskContract {
    pub schema_version: u32,
    pub contract_id: String,
    pub parent_contract_digest: String,
    pub objective_id: String,
    pub granted_capabilities: Vec<String>,
    pub budget: SubtaskBudget,
    pub evidence_requirement_digest: String,
    pub evidence_source: String,
    pub delegation_depth: u8,
    pub may_delegate: bool,
    pub execution_enabled: bool,
}

/// Durable scheduler identity reserved for one future isolated subtask attempt.
///
/// A dormant fence is an audit identity, not execution authority. Tool authorization must reject
/// it until the scheduler explicitly activates a later supported attempt state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DormantSubtaskFence {
    pub subtask_id: u64,
    pub subtask_attempt_id: u64,
    pub attempt: u32,
    pub fencing_token: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SchedulerPreparedSubtask {
    pub session_id: u64,
    pub objective_id: String,
    pub contract: PreparedSubtaskContract,
    pub state: String,
    pub fence: DormantSubtaskFence,
    pub tool_calls_used: u32,
    pub mutation_calls_used: u32,
    pub authority_active: bool,
    pub created_at: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtaskContractRequest {
    pub objective_id: String,
    pub requested_capabilities: Vec<String>,
    pub budget: SubtaskBudget,
    pub may_delegate: bool,
}

pub fn prepare_subtask_contracts(
    contract: &Value,
    parent_contract_digest: &str,
    parent_capabilities: &[String],
) -> Result<Vec<PreparedSubtaskContract>, String> {
    if !valid_parent_digest(parent_contract_digest) {
        return Err("Subtask parent contract digest is invalid.".to_string());
    }
    let Some(objectives) = contract
        .get("semantic_plan")
        .and_then(|plan| plan.get("objectives"))
        .and_then(Value::as_array)
    else {
        // Retained Agent contracts created before semantic planning remain executable by the
        // existing single Worker. They do not gain synthetic subtask authority.
        return Ok(Vec::new());
    };
    if objectives.is_empty() || objectives.len() > MAX_SUBTASKS {
        return Err("Subtask preparation requires between 1 and 8 objectives.".to_string());
    }
    let parent_capabilities = normalized_capabilities(parent_capabilities)?;
    let objective_count = u64::try_from(objectives.len()).unwrap_or(MAX_SUBTASKS as u64);
    let budget = SubtaskBudget {
        wall_clock_seconds: TOTAL_WALL_CLOCK_SECONDS / objective_count,
        max_tool_calls: TOTAL_TOOL_CALLS / u32::try_from(objective_count).unwrap_or(8),
        max_mutation_calls: 0,
    };
    let mut seen = BTreeSet::new();
    objectives
        .iter()
        .map(|objective| {
            let objective_id = objective
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| canonical_name(value))
                .ok_or_else(|| "Subtask objective identity is invalid.".to_string())?;
            if !seen.insert(objective_id.to_string()) {
                return Err("Subtask objective identities must be unique.".to_string());
            }
            let evidence = objective
                .get("success_evidence")
                .and_then(Value::as_str)
                .filter(|value| {
                    !value.trim().is_empty()
                        && value.len() <= 500
                        && !value.chars().any(char::is_control)
                })
                .ok_or_else(|| "Subtask evidence requirement is invalid.".to_string())?;
            let mutation_expected = objective
                .get("mutation_expected")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let objective_capabilities =
                narrow_objective_capabilities(contract, objective, &parent_capabilities)?;
            compile_subtask_contract(
                parent_contract_digest,
                &parent_capabilities,
                evidence,
                SubtaskContractRequest {
                    objective_id: objective_id.to_string(),
                    requested_capabilities: objective_capabilities,
                    budget: SubtaskBudget {
                        max_mutation_calls: if mutation_expected {
                            (TOTAL_MUTATION_CALLS / u32::try_from(objective_count).unwrap_or(8))
                                .max(1)
                        } else {
                            0
                        },
                        ..budget.clone()
                    },
                    may_delegate: false,
                },
            )
        })
        .collect()
}

/// Narrows external connector authority from immutable, secret-free routing evidence.
///
/// Model-produced objective hints may only remove grants from the parent set. A connector is
/// retained when the objective and exactly one connector-plan entry share a non-generic signal.
/// Ambiguous or malformed routing evidence grants no external connector. Built-in capabilities
/// additionally require objective-local resource and operation evidence.
fn narrow_objective_capabilities(
    contract: &Value,
    objective: &Value,
    parent_capabilities: &[String],
) -> Result<Vec<String>, String> {
    let mut selected = narrow_builtin_capabilities(objective, parent_capabilities);
    let objective_signals = objective_signal_tokens(objective);
    let connectors = contract
        .get("capability_plan")
        .and_then(|plan| plan.get("connectors"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(64)
        .filter_map(|connector| {
            let connector_id = connector.get("connector_id")?.as_str()?;
            let capability = format!("external_tools:{connector_id}");
            if !canonical_connector_id(connector_id) || !parent_capabilities.contains(&capability) {
                return None;
            }
            let mut signals = signal_tokens(connector_id);
            for field in ["matched_domains", "matched_intents", "matched_tools"] {
                for value in connector
                    .get(field)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .take(64)
                {
                    signals.extend(signal_tokens(value));
                }
            }
            Some((capability, signals))
        })
        .collect::<Vec<_>>();
    let mut signal_owners = BTreeMap::<String, usize>::new();
    for (_, signals) in &connectors {
        for signal in signals {
            *signal_owners.entry(signal.clone()).or_default() += 1;
        }
    }
    for (capability, signals) in connectors {
        if signals.iter().any(|signal| {
            objective_signals.contains(signal) && signal_owners.get(signal) == Some(&1)
        }) {
            selected.insert(capability);
        }
    }
    Ok(selected.into_iter().collect())
}

fn narrow_builtin_capabilities(
    objective: &Value,
    parent_capabilities: &[String],
) -> BTreeSet<String> {
    let signals = objective_all_signal_tokens(objective);
    let mutation_expected = objective
        .get("mutation_expected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let local_scope = contains_signal(
        &signals,
        &[
            "chart",
            "code",
            "config",
            "configuration",
            "directory",
            "file",
            "helm",
            "repository",
            "source",
            "template",
            "values",
            "workspace",
            "yaml",
        ],
    );
    let mut selected = BTreeSet::new();
    if local_scope
        && parent_capabilities
            .iter()
            .any(|value| value == "workspace_read")
    {
        selected.insert("workspace_read".to_string());
    }
    if local_scope
        && mutation_expected
        && contains_signal(
            &signals,
            &[
                "apply", "create", "edit", "modify", "patch", "replace", "update", "write",
            ],
        )
        && parent_capabilities
            .iter()
            .any(|value| value == "workspace_write")
    {
        selected.insert("workspace_write".to_string());
    }
    if local_scope
        && mutation_expected
        && contains_signal(
            &signals,
            &[
                "build", "command", "compile", "lint", "run", "script", "shell", "test",
            ],
        )
        && parent_capabilities.iter().any(|value| value == "shell")
    {
        selected.insert("shell".to_string());
    }
    if local_scope
        && contains_signal(
            &signals,
            &[
                "branch", "checkout", "commit", "git", "merge", "pull", "push", "rebase", "stage",
                "status",
            ],
        )
        && parent_capabilities.iter().any(|value| value == "git")
    {
        selected.insert("git".to_string());
    }
    selected
}

fn contains_signal(signals: &BTreeSet<String>, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| signals.contains(*candidate))
}

fn objective_signal_tokens(objective: &Value) -> BTreeSet<String> {
    objective_all_signal_tokens(objective)
        .into_iter()
        .filter(|token| !generic_signal(token))
        .collect()
}

fn objective_all_signal_tokens(objective: &Value) -> BTreeSet<String> {
    let mut signals = BTreeSet::new();
    for field in ["summary", "success_evidence"] {
        if let Some(value) = objective.get(field).and_then(Value::as_str) {
            signals.extend(all_signal_tokens(value));
        }
    }
    for field in ["operation_hints", "resource_hints"] {
        for value in objective
            .get(field)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(16)
        {
            signals.extend(all_signal_tokens(value));
        }
    }
    signals
}

fn signal_tokens(value: &str) -> BTreeSet<String> {
    all_signal_tokens(value)
        .into_iter()
        .filter(|token| !generic_signal(token))
        .collect()
}

fn all_signal_tokens(value: &str) -> BTreeSet<String> {
    let mut normalized = String::with_capacity(value.len());
    let mut previous_was_lower_or_digit = false;
    for character in value.chars() {
        if character.is_ascii_uppercase() && previous_was_lower_or_digit {
            normalized.push(' ');
        }
        normalized.push(character.to_ascii_lowercase());
        previous_was_lower_or_digit = character.is_ascii_lowercase() || character.is_ascii_digit();
    }
    normalized
        .split(|character: char| !character.is_ascii_alphanumeric())
        .map(str::to_string)
        .filter(|token| token.len() >= 3)
        .take(64)
        .collect()
}

fn generic_signal(value: &str) -> bool {
    matches!(
        value,
        "api"
            | "create"
            | "delete"
            | "get"
            | "inspect"
            | "list"
            | "mcp"
            | "read"
            | "search"
            | "tool"
            | "trigger"
            | "update"
            | "verify"
    )
}

fn canonical_connector_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value == value.trim()
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub fn compile_subtask_contract(
    parent_contract_digest: &str,
    parent_capabilities: &[String],
    evidence_requirement: &str,
    request: SubtaskContractRequest,
) -> Result<PreparedSubtaskContract, String> {
    if !valid_parent_digest(parent_contract_digest)
        || !canonical_name(&request.objective_id)
        || evidence_requirement.trim().is_empty()
        || evidence_requirement.len() > 500
        || evidence_requirement.chars().any(char::is_control)
    {
        return Err("Subtask contract identity or evidence requirement is invalid.".to_string());
    }
    if request.may_delegate {
        return Err("Subtask authority is non-delegable.".to_string());
    }
    if request.budget.wall_clock_seconds == 0
        || request.budget.wall_clock_seconds > TOTAL_WALL_CLOCK_SECONDS
        || request.budget.max_tool_calls == 0
        || request.budget.max_tool_calls > TOTAL_TOOL_CALLS
        || request.budget.max_mutation_calls > TOTAL_MUTATION_CALLS
        || request.budget.max_mutation_calls > request.budget.max_tool_calls
    {
        return Err("Subtask budget exceeds its bounded authority.".to_string());
    }
    let parent = normalized_capabilities(parent_capabilities)?;
    let requested = normalized_capabilities(&request.requested_capabilities)?;
    if requested
        .iter()
        .any(|capability| !parent.contains(capability))
    {
        return Err("Subtask requested authority that the parent does not possess.".to_string());
    }
    let evidence_requirement_digest = digest(evidence_requirement.as_bytes());
    let identity = serde_json::json!({
        "schema_version": SUBTASK_CONTRACT_SCHEMA_VERSION,
        "parent_contract_digest": parent_contract_digest,
        "objective_id": request.objective_id,
        "granted_capabilities": requested,
        "budget": request.budget,
        "evidence_requirement_digest": evidence_requirement_digest,
        "delegation_depth": 1,
        "may_delegate": false,
        "execution_enabled": false,
    });
    let encoded = serde_json::to_vec(&identity)
        .map_err(|_| "Failed to encode the subtask authority identity.".to_string())?;
    Ok(PreparedSubtaskContract {
        schema_version: SUBTASK_CONTRACT_SCHEMA_VERSION,
        contract_id: digest(&encoded),
        parent_contract_digest: parent_contract_digest.to_string(),
        objective_id: request.objective_id,
        granted_capabilities: requested,
        budget: request.budget,
        evidence_requirement_digest,
        evidence_source: "semantic_objective_success_evidence".to_string(),
        delegation_depth: 1,
        may_delegate: false,
        execution_enabled: false,
    })
}

fn normalized_capabilities(values: &[String]) -> Result<Vec<String>, String> {
    if values.len() > 64 || values.iter().any(|value| !canonical_capability(value)) {
        return Err("Subtask capability authority is invalid.".to_string());
    }
    Ok(values
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

fn canonical_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value == value.trim()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn canonical_capability(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value == value.trim()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
}

fn valid_parent_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        !digest.is_empty()
            && digest.len() <= 128
            && digest.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

fn digest(value: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent_digest() -> &'static str {
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }

    fn budget() -> SubtaskBudget {
        SubtaskBudget {
            wall_clock_seconds: 900,
            max_tool_calls: 16,
            max_mutation_calls: 1,
        }
    }

    #[test]
    fn prepares_independent_non_executable_contracts_for_semantic_objectives() {
        let contracts = prepare_subtask_contracts(
            &serde_json::json!({"semantic_plan": {"objectives": [
                {"id": "objective-1", "success_evidence": "page exists", "mutation_expected": false},
                {"id": "objective-2", "success_evidence": "deployment applied", "mutation_expected": true}
            ]}}),
            parent_digest(),
            &["workspace_read".to_string(), "external_tools:confluence".to_string()],
        )
        .expect("subtask contracts prepared");

        assert_eq!(contracts.len(), 2);
        assert_ne!(contracts[0].contract_id, contracts[1].contract_id);
        assert_eq!(contracts[0].budget.max_mutation_calls, 0);
        assert!(contracts[1].budget.max_mutation_calls > 0);
        assert!(contracts.iter().all(|contract| {
            !contract.execution_enabled && !contract.may_delegate && contract.delegation_depth == 1
        }));
    }

    #[test]
    fn narrows_external_connectors_per_objective_without_expanding_parent_authority() {
        let contracts = prepare_subtask_contracts(
            &serde_json::json!({
                "semantic_plan": {"objectives": [
                    {
                        "id": "read-page",
                        "summary": "Read the Confluence page",
                        "success_evidence": "Page content is observed",
                        "operation_hints": ["read page"],
                        "resource_hints": ["confluence page"],
                        "mutation_expected": false
                    },
                    {
                        "id": "trigger-build",
                        "summary": "Trigger the Bamboo build",
                        "success_evidence": "Build result is observed",
                        "operation_hints": ["trigger build"],
                        "resource_hints": ["bamboo plan"],
                        "mutation_expected": true
                    }
                ]},
                "capability_plan": {"connectors": [
                    {
                        "connector_id": "confluence",
                        "matched_domains": ["confluence"],
                        "matched_intents": ["page"],
                        "matched_tools": ["confluence_get_page"]
                    },
                    {
                        "connector_id": "bamboo",
                        "matched_domains": ["bamboo"],
                        "matched_intents": ["build"],
                        "matched_tools": ["bamboo_trigger_build"]
                    },
                    {
                        "connector_id": "ungranted",
                        "matched_domains": ["ungranted"],
                        "matched_intents": [],
                        "matched_tools": ["ungranted_mutate"]
                    }
                ]}
            }),
            parent_digest(),
            &[
                "workspace_read".to_string(),
                "external_tools:confluence".to_string(),
                "external_tools:bamboo".to_string(),
            ],
        )
        .expect("objective grants narrow");

        assert_eq!(
            contracts[0].granted_capabilities,
            vec!["external_tools:confluence".to_string()]
        );
        assert_eq!(
            contracts[1].granted_capabilities,
            vec!["external_tools:bamboo".to_string()]
        );
        assert!(contracts.iter().all(|contract| !contract
            .granted_capabilities
            .iter()
            .any(|capability| capability == "external_tools:ungranted")));
    }

    #[test]
    fn ambiguous_or_missing_connector_evidence_grants_no_external_authority() {
        let contract = serde_json::json!({
            "semantic_plan": {"objectives": [{
                "id": "inspect-item",
                "summary": "Inspect the shared item",
                "success_evidence": "Item is observed",
                "operation_hints": ["read item"],
                "resource_hints": ["item"],
                "mutation_expected": false
            }]},
            "capability_plan": {"connectors": [
                {
                    "connector_id": "system-one",
                    "matched_domains": [],
                    "matched_intents": ["item"],
                    "matched_tools": ["read_item"]
                },
                {
                    "connector_id": "system-two",
                    "matched_domains": [],
                    "matched_intents": ["item"],
                    "matched_tools": ["read_item"]
                }
            ]}
        });
        let capabilities = vec![
            "workspace_read".to_string(),
            "external_tools:system-one".to_string(),
            "external_tools:system-two".to_string(),
        ];
        let first = prepare_subtask_contracts(&contract, parent_digest(), &capabilities)
            .expect("ambiguous routing narrows closed");
        let reversed = prepare_subtask_contracts(
            &contract,
            parent_digest(),
            &capabilities.into_iter().rev().collect::<Vec<_>>(),
        )
        .expect("parent order does not change narrowing");

        assert!(first[0].granted_capabilities.is_empty());
        assert_eq!(first[0].contract_id, reversed[0].contract_id);
    }

    #[test]
    fn unknown_connector_tools_use_provider_neutral_camel_case_signals() {
        let contracts = prepare_subtask_contracts(
            &serde_json::json!({
                "semantic_plan": {"objectives": [{
                    "id": "promote-release",
                    "summary": "Promote the release",
                    "success_evidence": "Release promotion is observed",
                    "operation_hints": ["promote release"],
                    "resource_hints": [],
                    "mutation_expected": true
                }]},
                "capability_plan": {"connectors": [{
                    "connector_id": "future-system",
                    "matched_domains": [],
                    "matched_intents": [],
                    "matched_tools": ["promoteRelease"]
                }]}
            }),
            parent_digest(),
            &["external_tools:future-system".to_string()],
        )
        .expect("unknown connector signal narrows deterministically");

        assert_eq!(
            contracts[0].granted_capabilities,
            vec!["external_tools:future-system".to_string()]
        );
    }

    #[test]
    fn narrows_builtin_authority_by_local_scope_operation_and_mutation_class() {
        let contracts = prepare_subtask_contracts(
            &serde_json::json!({
                "semantic_plan": {"objectives": [
                    {
                        "id": "inspect-template",
                        "summary": "Inspect the Helm template",
                        "success_evidence": "Template is understood",
                        "operation_hints": ["read file"],
                        "resource_hints": ["helm template"],
                        "mutation_expected": false
                    },
                    {
                        "id": "modify-values",
                        "summary": "Update the values YAML file",
                        "success_evidence": "Values file contains the desired configuration",
                        "operation_hints": ["update file"],
                        "resource_hints": ["values yaml"],
                        "mutation_expected": true
                    },
                    {
                        "id": "run-tests",
                        "summary": "Run repository tests",
                        "success_evidence": "The source test command passes",
                        "operation_hints": ["run test command"],
                        "resource_hints": ["source repository"],
                        "mutation_expected": true
                    },
                    {
                        "id": "commit-change",
                        "summary": "Commit the repository change",
                        "success_evidence": "Git commit status is clean",
                        "operation_hints": ["git commit"],
                        "resource_hints": ["repository branch"],
                        "mutation_expected": true
                    },
                    {
                        "id": "trigger-build",
                        "summary": "Trigger the Bamboo build",
                        "success_evidence": "Bamboo build succeeds",
                        "operation_hints": ["trigger build"],
                        "resource_hints": ["bamboo plan"],
                        "mutation_expected": true
                    }
                ]},
                "capability_plan": {"connectors": [{
                    "connector_id": "bamboo",
                    "matched_domains": ["bamboo"],
                    "matched_intents": ["build"],
                    "matched_tools": ["bamboo_trigger_build"]
                }]}
            }),
            parent_digest(),
            &[
                "workspace_read".to_string(),
                "workspace_write".to_string(),
                "shell".to_string(),
                "git".to_string(),
                "external_tools:bamboo".to_string(),
            ],
        )
        .expect("built-in authority narrows");

        assert_eq!(
            contracts[0].granted_capabilities,
            vec!["workspace_read".to_string()]
        );
        assert_eq!(
            contracts[1].granted_capabilities,
            vec!["workspace_read".to_string(), "workspace_write".to_string()]
        );
        assert_eq!(
            contracts[2].granted_capabilities,
            vec!["shell".to_string(), "workspace_read".to_string()]
        );
        assert_eq!(
            contracts[3].granted_capabilities,
            vec!["git".to_string(), "workspace_read".to_string()]
        );
        assert_eq!(
            contracts[4].granted_capabilities,
            vec!["external_tools:bamboo".to_string()]
        );
    }

    #[test]
    fn read_only_objective_cannot_gain_write_or_shell_from_operation_words() {
        let contracts = prepare_subtask_contracts(
            &serde_json::json!({"semantic_plan": {"objectives": [{
                "id": "review-command",
                "summary": "Review a shell command that writes a file",
                "success_evidence": "The source file is reviewed",
                "operation_hints": ["write file", "run shell command"],
                "resource_hints": ["workspace source file"],
                "mutation_expected": false
            }]}}),
            parent_digest(),
            &[
                "workspace_read".to_string(),
                "workspace_write".to_string(),
                "shell".to_string(),
                "git".to_string(),
            ],
        )
        .expect("read-only objective narrows closed");

        assert_eq!(
            contracts[0].granted_capabilities,
            vec!["workspace_read".to_string()]
        );
    }

    #[test]
    fn retained_contract_without_semantic_plan_gains_no_subtask_authority() {
        assert!(prepare_subtask_contracts(
            &serde_json::json!({"objective": {"summary": "retained task"}}),
            parent_digest(),
            &["workspace_read".to_string()],
        )
        .expect("retained contract remains compatible")
        .is_empty());
    }

    #[test]
    fn rejects_capability_expansion_and_delegable_subtasks() {
        let parent = vec!["workspace_read".to_string()];
        let expanded = compile_subtask_contract(
            parent_digest(),
            &parent,
            "verified",
            SubtaskContractRequest {
                objective_id: "objective-1".to_string(),
                requested_capabilities: vec!["workspace_read".to_string(), "git".to_string()],
                budget: budget(),
                may_delegate: false,
            },
        );
        assert_eq!(
            expanded.expect_err("authority expansion rejected"),
            "Subtask requested authority that the parent does not possess."
        );
        let delegation = compile_subtask_contract(
            parent_digest(),
            &parent,
            "verified",
            SubtaskContractRequest {
                objective_id: "objective-1".to_string(),
                requested_capabilities: parent.clone(),
                budget: budget(),
                may_delegate: true,
            },
        );
        assert_eq!(
            delegation.expect_err("delegable subtask rejected"),
            "Subtask authority is non-delegable."
        );
    }

    #[test]
    fn persisted_contract_hashes_evidence_and_is_stable_across_capability_order() {
        let evidence = "credential=must-not-be-persisted";
        let first = compile_subtask_contract(
            parent_digest(),
            &["git".to_string(), "workspace_read".to_string()],
            evidence,
            SubtaskContractRequest {
                objective_id: "objective-1".to_string(),
                requested_capabilities: vec!["workspace_read".to_string(), "git".to_string()],
                budget: budget(),
                may_delegate: false,
            },
        )
        .expect("first contract");
        let second = compile_subtask_contract(
            parent_digest(),
            &["workspace_read".to_string(), "git".to_string()],
            evidence,
            SubtaskContractRequest {
                objective_id: "objective-1".to_string(),
                requested_capabilities: vec!["git".to_string(), "workspace_read".to_string()],
                budget: budget(),
                may_delegate: false,
            },
        )
        .expect("second contract");
        assert_eq!(first.contract_id, second.contract_id);
        assert!(!serde_json::to_string(&first).unwrap().contains(evidence));
    }
}
