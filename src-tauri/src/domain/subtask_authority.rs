//! Provider-neutral authority contracts for future isolated Agent subtasks.
//!
//! This module prepares durable contracts only. It intentionally does not dispatch another
//! worker; execution must remain disabled until the scheduler owns independent subtask attempts
//! and fences.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

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
            compile_subtask_contract(
                parent_contract_digest,
                &parent_capabilities,
                evidence,
                SubtaskContractRequest {
                    objective_id: objective_id.to_string(),
                    requested_capabilities: parent_capabilities.clone(),
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
