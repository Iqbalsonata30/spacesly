//! Provider-neutral authority contracts for future isolated Agent subtasks.
//!
//! This module prepares durable contracts only. It intentionally does not dispatch another
//! worker; execution must remain disabled until the scheduler owns independent subtask attempts
//! and fences.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const SUBTASK_CONTRACT_SCHEMA_VERSION: u32 = 2;
const CONNECTOR_READ_BOUND_SUBTASK_CONTRACT_SCHEMA_VERSION: u32 = 5;
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

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PreparedSubtaskEvidenceVerifier {
    pub verifier_id: String,
    pub provider: String,
    pub verification_method: String,
    pub required_states_digest: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub authority_mode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub authority_capability_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_identity_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_argument: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtaskVerifierResourceIdentity {
    pub resource_kind: String,
    pub resource_name: String,
    pub scope: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtaskEvidenceVerifierCandidate {
    pub verifier_id: String,
    pub provider: String,
    pub verification_method: String,
    pub required_states: Vec<String>,
    pub required_capability: String,
    pub resource_identity: Option<SubtaskVerifierResourceIdentity>,
    pub read_tool: Option<String>,
    pub read_argument: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubtaskVerifierBindingSummary {
    pub contracts: Vec<PreparedSubtaskContract>,
    pub assigned_verifiers: u32,
    pub unassigned_verifiers: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreparedSubtaskContract {
    pub schema_version: u32,
    pub contract_id: String,
    pub parent_contract_digest: String,
    pub objective_id: String,
    pub granted_capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub allowed_connector_tools: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_verifiers: Vec<PreparedSubtaskEvidenceVerifier>,
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
            let (objective_capabilities, allowed_connector_tools) =
                narrow_objective_capabilities(contract, objective, &parent_capabilities)?;
            compile_subtask_contract_with_tools(
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
                allowed_connector_tools,
            )
        })
        .collect()
}

/// Prepares version-5 contracts and assigns each supported verifier to one unambiguous objective.
///
/// Git terminal-state, Kubernetes Deployment-availability, and immutable-result Bamboo
/// verification are the supported binding slices. Unknown providers, missing objective signals,
/// missing narrowed authority, and ties stay unassigned and therefore cannot attest a subtask.
pub fn prepare_subtask_contracts_with_verifiers(
    contract: &Value,
    parent_contract_digest: &str,
    parent_capabilities: &[String],
    candidates: &[SubtaskEvidenceVerifierCandidate],
) -> Result<SubtaskVerifierBindingSummary, String> {
    let base = prepare_subtask_contracts(contract, parent_contract_digest, parent_capabilities)?;
    if base.is_empty() {
        return Ok(SubtaskVerifierBindingSummary {
            contracts: Vec::new(),
            assigned_verifiers: 0,
            unassigned_verifiers: u32::try_from(candidates.len()).unwrap_or(u32::MAX),
        });
    }
    if candidates.len() > 64 {
        return Err("Subtask verifier candidates exceed their bounded limit.".to_string());
    }
    let objectives = contract
        .pointer("/semantic_plan/objectives")
        .and_then(Value::as_array)
        .ok_or_else(|| "Subtask verifier binding requires semantic objectives.".to_string())?;
    let mut bindings = BTreeMap::<String, Vec<PreparedSubtaskEvidenceVerifier>>::new();
    let mut assigned = 0_u32;
    let mut unassigned = 0_u32;
    let mut normalized_candidates = candidates.to_vec();
    normalized_candidates.sort_by(|left, right| left.verifier_id.cmp(&right.verifier_id));
    if normalized_candidates
        .windows(2)
        .any(|pair| pair[0].verifier_id == pair[1].verifier_id)
    {
        return Err("Subtask verifier candidate identities must be unique.".to_string());
    }
    for candidate in normalized_candidates {
        validate_verifier_candidate(&candidate)?;
        let matching_objectives = objectives
            .iter()
            .filter(|objective| verifier_matches_objective(&candidate, objective))
            .filter_map(|objective| objective.get("id").and_then(Value::as_str))
            .collect::<Vec<_>>();
        if matching_objectives.len() != 1 {
            unassigned = unassigned.saturating_add(1);
            continue;
        }
        let objective_id = matching_objectives[0];
        let Some(prepared) = base.iter().find(|item| item.objective_id == objective_id) else {
            return Err(
                "Verifier objective is absent from prepared subtask authority.".to_string(),
            );
        };
        if !prepared
            .granted_capabilities
            .contains(&candidate.required_capability)
            || candidate.read_tool.as_ref().is_some_and(|tool| {
                prepared
                    .allowed_connector_tools
                    .get(&candidate.required_capability)
                    .is_none_or(|tools| !tools.contains(tool))
            })
        {
            unassigned = unassigned.saturating_add(1);
            continue;
        }
        let required_states = candidate
            .required_states
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let encoded_states = serde_json::to_vec(&required_states)
            .map_err(|_| "Failed to encode subtask verifier states.".to_string())?;
        let authority_capability_digest =
            subtask_authority_capability_digest(&candidate.required_capability);
        let resource_identity_digest = candidate
            .resource_identity
            .as_ref()
            .map(|identity| {
                serde_json::to_vec(&serde_json::json!({
                    "provider": candidate.provider.as_str(),
                    "resource_kind": identity.resource_kind.as_str(),
                    "resource_name": identity.resource_name.as_str(),
                    "scope": identity.scope.as_str(),
                }))
                .map(|encoded| digest(&encoded))
                .map_err(|_| "Failed to encode subtask verifier resource identity.".to_string())
            })
            .transpose()?;
        bindings.entry(objective_id.to_string()).or_default().push(
            PreparedSubtaskEvidenceVerifier {
                verifier_id: candidate.verifier_id,
                provider: candidate.provider,
                verification_method: candidate.verification_method,
                required_states_digest: digest(&encoded_states),
                authority_mode: "read_only".to_string(),
                authority_capability_digest,
                resource_identity_digest,
                read_tool: candidate.read_tool,
                read_argument: candidate.read_argument,
            },
        );
        assigned = assigned.saturating_add(1);
    }
    let mut contracts = Vec::with_capacity(base.len());
    for prepared in base {
        let objective = objectives
            .iter()
            .find(|objective| {
                objective.get("id").and_then(Value::as_str) == Some(prepared.objective_id.as_str())
            })
            .ok_or_else(|| "Prepared subtask objective is absent.".to_string())?;
        let evidence = objective
            .get("success_evidence")
            .and_then(Value::as_str)
            .ok_or_else(|| "Subtask evidence requirement is absent.".to_string())?;
        let mut objective_bindings = bindings.remove(&prepared.objective_id).unwrap_or_default();
        objective_bindings.sort();
        contracts.push(compile_subtask_contract_with_tools_and_verifiers(
            CONNECTOR_READ_BOUND_SUBTASK_CONTRACT_SCHEMA_VERSION,
            parent_contract_digest,
            parent_capabilities,
            evidence,
            SubtaskContractRequest {
                objective_id: prepared.objective_id,
                requested_capabilities: prepared.granted_capabilities,
                budget: prepared.budget,
                may_delegate: false,
            },
            prepared.allowed_connector_tools,
            objective_bindings,
        )?);
    }
    Ok(SubtaskVerifierBindingSummary {
        contracts,
        assigned_verifiers: assigned,
        unassigned_verifiers: unassigned,
    })
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
) -> Result<(Vec<String>, BTreeMap<String, Vec<String>>), String> {
    let mut selected = narrow_builtin_capabilities(objective, parent_capabilities);
    let objective_signals = objective_signal_tokens(objective);
    let objective_all_signals = objective_all_signal_tokens(objective);
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
            let planned_tools = connector
                .get("matched_tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|tool| canonical_tool_name(tool))
                .take(64)
                .map(str::to_string)
                .collect::<BTreeSet<_>>();
            Some((capability, signals, planned_tools))
        })
        .collect::<Vec<_>>();
    let mut signal_owners = BTreeMap::<String, usize>::new();
    for (_, signals, _) in &connectors {
        for signal in signals {
            *signal_owners.entry(signal.clone()).or_default() += 1;
        }
    }
    let mut allowed_connector_tools = BTreeMap::new();
    for (capability, signals, planned_tools) in connectors {
        if signals.iter().any(|signal| {
            objective_signals.contains(signal) && signal_owners.get(signal) == Some(&1)
        }) {
            let connector_signals = signal_tokens(
                capability
                    .strip_prefix("external_tools:")
                    .unwrap_or_default(),
            );
            let allowed = planned_tools
                .into_iter()
                .filter(|tool| {
                    tool_matches_objective(tool, &objective_all_signals, &connector_signals)
                })
                .collect::<Vec<_>>();
            if !allowed.is_empty() {
                selected.insert(capability.clone());
                allowed_connector_tools.insert(capability, allowed);
            }
        }
    }
    Ok((selected.into_iter().collect(), allowed_connector_tools))
}

fn tool_matches_objective(
    tool: &str,
    objective_signals: &BTreeSet<String>,
    connector_signals: &BTreeSet<String>,
) -> bool {
    let tool_signals = all_signal_tokens(tool);
    let resource_overlap = tool_signals.iter().any(|signal| {
        !connector_signals.contains(signal)
            && operation_signal(signal).is_none()
            && objective_signals.contains(signal)
    });
    let objective_operations = objective_signals
        .iter()
        .filter_map(|signal| operation_signal(signal))
        .collect::<BTreeSet<_>>();
    let tool_operations = tool_signals
        .iter()
        .filter_map(|signal| operation_signal(signal))
        .collect::<BTreeSet<_>>();
    resource_overlap
        && !objective_operations.is_empty()
        && !objective_operations.is_disjoint(&tool_operations)
}

fn operation_signal(value: &str) -> Option<&'static str> {
    match value {
        "get" | "inspect" | "list" | "read" | "search" => Some("read"),
        "add" | "create" | "publish" => Some("create"),
        "edit" | "modify" | "patch" | "replace" | "update" | "write" => Some("update"),
        "delete" | "remove" => Some("delete"),
        "promote" => Some("promote"),
        "restart" => Some("restart"),
        "trigger" => Some("trigger"),
        _ => None,
    }
}

fn canonical_tool_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value == value.trim()
        && !value.contains("..")
        && !value.starts_with('/')
        && !value.ends_with('/')
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
        })
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

#[cfg_attr(not(test), allow(dead_code))]
pub fn compile_subtask_contract(
    parent_contract_digest: &str,
    parent_capabilities: &[String],
    evidence_requirement: &str,
    request: SubtaskContractRequest,
) -> Result<PreparedSubtaskContract, String> {
    compile_subtask_contract_with_tools(
        parent_contract_digest,
        parent_capabilities,
        evidence_requirement,
        request,
        BTreeMap::new(),
    )
}

fn compile_subtask_contract_with_tools(
    parent_contract_digest: &str,
    parent_capabilities: &[String],
    evidence_requirement: &str,
    request: SubtaskContractRequest,
    allowed_connector_tools: BTreeMap<String, Vec<String>>,
) -> Result<PreparedSubtaskContract, String> {
    compile_subtask_contract_with_tools_and_verifiers(
        SUBTASK_CONTRACT_SCHEMA_VERSION,
        parent_contract_digest,
        parent_capabilities,
        evidence_requirement,
        request,
        allowed_connector_tools,
        Vec::new(),
    )
}

fn compile_subtask_contract_with_tools_and_verifiers(
    schema_version: u32,
    parent_contract_digest: &str,
    parent_capabilities: &[String],
    evidence_requirement: &str,
    request: SubtaskContractRequest,
    allowed_connector_tools: BTreeMap<String, Vec<String>>,
    evidence_verifiers: Vec<PreparedSubtaskEvidenceVerifier>,
) -> Result<PreparedSubtaskContract, String> {
    if !matches!(schema_version, 2 | 3 | 4 | 5)
        || (schema_version < 3 && !evidence_verifiers.is_empty())
        || evidence_verifiers.len() > 64
        || evidence_verifiers.windows(2).any(|pair| pair[0] >= pair[1])
        || evidence_verifiers.iter().any(|binding| {
            !canonical_name(&binding.verifier_id)
                || !canonical_name(&binding.provider)
                || !canonical_name(&binding.verification_method)
                || !valid_sha256_digest(&binding.required_states_digest)
                || (schema_version < 4
                    && (!binding.authority_mode.is_empty()
                        || !binding.authority_capability_digest.is_empty()
                        || binding.resource_identity_digest.is_some()
                        || binding.read_tool.is_some()
                        || binding.read_argument.is_some()))
                || (schema_version == 4
                    && (binding.read_tool.is_some() || binding.read_argument.is_some()))
                || (schema_version >= 4
                    && (binding.authority_mode != "read_only"
                        || !valid_sha256_digest(&binding.authority_capability_digest)
                        || binding
                            .resource_identity_digest
                            .as_deref()
                            .is_some_and(|value| !valid_sha256_digest(value))
                        || (binding.provider == "kubernetes"
                            && binding.resource_identity_digest.is_none())))
                || (schema_version == 5
                    && (binding
                        .read_tool
                        .as_deref()
                        .is_some_and(|value| !canonical_tool_name(value))
                        || binding
                            .read_argument
                            .as_deref()
                            .is_some_and(|value| !canonical_name(value))
                        || (binding.provider == "bamboo"
                            && (binding.resource_identity_digest.is_none()
                                || binding.read_tool.is_none()
                                || binding.read_argument.is_none()))))
        })
    {
        return Err("Subtask verifier authority is invalid.".to_string());
    }
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
    if schema_version >= 4
        && evidence_verifiers.iter().any(|binding| {
            !requested.iter().any(|capability| {
                subtask_authority_capability_digest(capability)
                    == binding.authority_capability_digest
            })
        })
    {
        return Err("Subtask verifier capability authority is not granted.".to_string());
    }
    if schema_version == 5
        && evidence_verifiers.iter().any(|binding| {
            binding.read_tool.as_ref().is_some_and(|tool| {
                !requested.iter().any(|capability| {
                    subtask_authority_capability_digest(capability)
                        == binding.authority_capability_digest
                        && allowed_connector_tools
                            .get(capability)
                            .is_some_and(|tools| tools.contains(tool))
                })
            })
        })
    {
        return Err("Subtask verifier read operation authority is not granted.".to_string());
    }
    if allowed_connector_tools.iter().any(|(capability, tools)| {
        !capability.starts_with("external_tools:")
            || !requested.contains(capability)
            || tools.is_empty()
            || tools.len() > 64
            || tools.windows(2).any(|pair| pair[0] >= pair[1])
            || tools.iter().any(|tool| !canonical_tool_name(tool))
    }) || requested.iter().any(|capability| {
        capability.starts_with("external_tools:")
            && !allowed_connector_tools.contains_key(capability)
    }) {
        return Err("Subtask connector operation authority is invalid.".to_string());
    }
    let evidence_requirement_digest = digest(evidence_requirement.as_bytes());
    let identity = serde_json::json!({
        "schema_version": schema_version,
        "parent_contract_digest": parent_contract_digest,
        "objective_id": request.objective_id,
        "granted_capabilities": requested,
        "allowed_connector_tools": allowed_connector_tools,
        "evidence_verifiers": evidence_verifiers,
        "budget": request.budget,
        "evidence_requirement_digest": evidence_requirement_digest,
        "delegation_depth": 1,
        "may_delegate": false,
        "execution_enabled": false,
    });
    let encoded = serde_json::to_vec(&identity)
        .map_err(|_| "Failed to encode the subtask authority identity.".to_string())?;
    Ok(PreparedSubtaskContract {
        schema_version,
        contract_id: digest(&encoded),
        parent_contract_digest: parent_contract_digest.to_string(),
        objective_id: request.objective_id,
        granted_capabilities: requested,
        allowed_connector_tools,
        evidence_verifiers,
        budget: request.budget,
        evidence_requirement_digest,
        evidence_source: "semantic_objective_success_evidence".to_string(),
        delegation_depth: 1,
        may_delegate: false,
        execution_enabled: false,
    })
}

fn validate_verifier_candidate(candidate: &SubtaskEvidenceVerifierCandidate) -> Result<(), String> {
    if !canonical_name(&candidate.verifier_id)
        || !canonical_name(&candidate.provider)
        || !canonical_name(&candidate.verification_method)
        || !canonical_capability(&candidate.required_capability)
        || candidate.required_states.is_empty()
        || candidate.required_states.len() > 16
        || candidate
            .required_states
            .iter()
            .any(|state| !canonical_name(state))
        || candidate
            .resource_identity
            .as_ref()
            .is_some_and(|identity| {
                !canonical_name(&identity.resource_kind)
                    || !canonical_resource_identity_component(&identity.resource_name)
                    || !canonical_resource_identity_component(&identity.scope)
            })
        || (candidate.provider == "git" && candidate.resource_identity.is_some())
        || candidate
            .read_tool
            .as_deref()
            .is_some_and(|value| !canonical_tool_name(value))
        || candidate
            .read_argument
            .as_deref()
            .is_some_and(|value| !canonical_name(value))
        || (candidate.read_tool.is_some() != candidate.read_argument.is_some())
        || (candidate.provider == "kubernetes"
            && (candidate.required_states != ["deployment_available"]
                || candidate
                    .resource_identity
                    .as_ref()
                    .is_none_or(|identity| identity.resource_kind != "deployment")))
        || (candidate.provider == "bamboo"
            && (candidate.required_states != ["successful_build"]
                || candidate
                    .resource_identity
                    .as_ref()
                    .is_none_or(|identity| identity.resource_kind != "build")
                || candidate.read_tool.is_none()
                || candidate.read_argument.is_none()))
    {
        return Err("Subtask verifier candidate is invalid.".to_string());
    }
    Ok(())
}

fn verifier_matches_objective(
    candidate: &SubtaskEvidenceVerifierCandidate,
    objective: &Value,
) -> bool {
    match candidate.provider.as_str() {
        "git" => git_verifier_matches_objective(candidate, objective),
        "kubernetes" => kubernetes_verifier_matches_objective(candidate, objective),
        "bamboo" => bamboo_verifier_matches_objective(candidate, objective),
        _ => false,
    }
}

fn git_verifier_matches_objective(
    candidate: &SubtaskEvidenceVerifierCandidate,
    objective: &Value,
) -> bool {
    let signals = objective_all_signal_tokens(objective);
    if signals.contains("git") {
        return true;
    }
    candidate
        .required_states
        .iter()
        .any(|state| match state.as_str() {
            "clean_worktree" => contains_signal(&signals, &["clean", "status", "worktree"]),
            "new_commit" => signals.contains("commit"),
            "pushed_upstream" => contains_signal(&signals, &["push", "pushed", "upstream"]),
            _ => false,
        })
}

fn kubernetes_verifier_matches_objective(
    candidate: &SubtaskEvidenceVerifierCandidate,
    objective: &Value,
) -> bool {
    let Some(identity) = candidate.resource_identity.as_ref() else {
        return false;
    };
    let signals = objective_all_signal_tokens(objective);
    let has_deployment_scope = contains_signal(
        &signals,
        &["deployment", "kubernetes", "openshift", "ocp", "workload"],
    );
    has_deployment_scope && objective_contains_resource_identity(objective, &identity.resource_name)
}

fn bamboo_verifier_matches_objective(
    candidate: &SubtaskEvidenceVerifierCandidate,
    objective: &Value,
) -> bool {
    let Some(identity) = candidate.resource_identity.as_ref() else {
        return false;
    };
    let signals = objective_all_signal_tokens(objective);
    let has_build_scope = contains_signal(&signals, &["bamboo", "build", "result"]);
    has_build_scope && objective_contains_resource_identity(objective, &identity.resource_name)
}

fn objective_contains_resource_identity(objective: &Value, resource_identity: &str) -> bool {
    let expected = normalized_identity_sequence(resource_identity);
    if expected.is_empty() {
        return false;
    }
    let needle = format!("-{expected}-");
    ["summary", "success_evidence"]
        .into_iter()
        .filter_map(|field| objective.get(field).and_then(Value::as_str))
        .chain(
            ["operation_hints", "resource_hints"]
                .into_iter()
                .flat_map(|field| {
                    objective
                        .get(field)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .take(16)
                }),
        )
        .any(|value| {
            let normalized = normalized_identity_sequence(value);
            format!("-{normalized}-").contains(&needle)
        })
}

fn normalized_identity_sequence(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
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

fn canonical_resource_identity_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value == value.trim()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_parent_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        !digest.is_empty()
            && digest.len() <= 128
            && digest.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn digest(value: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(value))
}

pub(crate) fn subtask_authority_capability_digest(capability: &str) -> String {
    digest(capability.as_bytes())
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
    fn binds_git_terminal_state_verifier_to_one_exact_objective() {
        let contract = serde_json::json!({"semantic_plan": {"objectives": [
            {
                "id": "inspect-template",
                "summary": "Inspect the Helm template",
                "success_evidence": "Template content is understood",
                "operation_hints": ["read file"],
                "resource_hints": ["helm template"],
                "mutation_expected": false
            },
            {
                "id": "commit-change",
                "summary": "Commit and push the Git repository change",
                "success_evidence": "Git worktree is clean and the commit is pushed upstream",
                "operation_hints": ["git commit", "git push"],
                "resource_hints": ["repository branch"],
                "mutation_expected": true
            }
        ]}});
        let summary = prepare_subtask_contracts_with_verifiers(
            &contract,
            parent_digest(),
            &["workspace_read".to_string(), "git".to_string()],
            &[SubtaskEvidenceVerifierCandidate {
                verifier_id: "git-release-state".to_string(),
                provider: "git".to_string(),
                verification_method: "git_terminal_state_v1".to_string(),
                required_states: vec![
                    "clean_worktree".to_string(),
                    "new_commit".to_string(),
                    "pushed_upstream".to_string(),
                ],
                required_capability: "git".to_string(),
                resource_identity: None,
                read_tool: None,
                read_argument: None,
            }],
        )
        .expect("Git verifier binds");

        assert_eq!(summary.assigned_verifiers, 1);
        assert_eq!(summary.unassigned_verifiers, 0);
        assert!(summary
            .contracts
            .iter()
            .all(|item| item.schema_version == 5));
        assert!(summary.contracts[0].evidence_verifiers.is_empty());
        assert_eq!(summary.contracts[1].evidence_verifiers.len(), 1);
        assert_eq!(
            summary.contracts[1].evidence_verifiers[0].verification_method,
            "git_terminal_state_v1"
        );
        assert_ne!(
            summary.contracts[1].contract_id,
            prepare_subtask_contracts(
                &contract,
                parent_digest(),
                &["workspace_read".to_string(), "git".to_string()],
            )
            .expect("legacy staged contracts prepare")[1]
                .contract_id
        );
        let reversed = prepare_subtask_contracts_with_verifiers(
            &contract,
            parent_digest(),
            &["git".to_string(), "workspace_read".to_string()],
            &[SubtaskEvidenceVerifierCandidate {
                verifier_id: "git-release-state".to_string(),
                provider: "git".to_string(),
                verification_method: "git_terminal_state_v1".to_string(),
                required_states: vec![
                    "pushed_upstream".to_string(),
                    "new_commit".to_string(),
                    "clean_worktree".to_string(),
                ],
                required_capability: "git".to_string(),
                resource_identity: None,
                read_tool: None,
                read_argument: None,
            }],
        )
        .expect("state and capability order is canonical");
        assert_eq!(summary.contracts, reversed.contracts);

        let changed_requirement = prepare_subtask_contracts_with_verifiers(
            &contract,
            parent_digest(),
            &["workspace_read".to_string(), "git".to_string()],
            &[SubtaskEvidenceVerifierCandidate {
                verifier_id: "git-release-state".to_string(),
                provider: "git".to_string(),
                verification_method: "git_terminal_state_v1".to_string(),
                required_states: vec!["new_commit".to_string()],
                required_capability: "git".to_string(),
                resource_identity: None,
                read_tool: None,
                read_argument: None,
            }],
        )
        .expect("changed verifier requirement prepares");
        assert_ne!(
            summary.contracts[1].contract_id, changed_requirement.contracts[1].contract_id,
            "required terminal states must be bound into contract identity"
        );
    }

    #[test]
    fn binds_kubernetes_verifier_to_exact_resource_objective_and_read_authority() {
        let contract = serde_json::json!({
            "capability_plan": {"connectors": [{
                "connector_id": "trusted-ocp",
                "matched_domains": ["kubernetes"],
                "matched_intents": ["deployment health"],
                "matched_tools": ["ocp_get_deployment"]
            }]},
            "semantic_plan": {"objectives": [
                {
                    "id": "edit-chart",
                    "summary": "Edit the Helm chart",
                    "success_evidence": "Chart values are updated",
                    "operation_hints": ["edit file"],
                    "resource_hints": ["helm values"],
                    "mutation_expected": true
                },
                {
                    "id": "verify-payroll",
                    "summary": "Read Kubernetes payroll-api Deployment availability",
                    "success_evidence": "The payroll-api Deployment is available",
                    "operation_hints": ["get deployment"],
                    "resource_hints": ["payroll-api deployment"],
                    "mutation_expected": false
                }
            ]}
        });
        let candidate = SubtaskEvidenceVerifierCandidate {
            verifier_id: "deployment-health".to_string(),
            provider: "kubernetes".to_string(),
            verification_method: "kubernetes_deployment_available_v1".to_string(),
            required_states: vec!["deployment_available".to_string()],
            required_capability: "external_tools:trusted-ocp".to_string(),
            resource_identity: Some(SubtaskVerifierResourceIdentity {
                resource_kind: "deployment".to_string(),
                resource_name: "payroll-api".to_string(),
                scope: "prerelease".to_string(),
            }),
            read_tool: None,
            read_argument: None,
        };
        let summary = prepare_subtask_contracts_with_verifiers(
            &contract,
            parent_digest(),
            &[
                "workspace_read".to_string(),
                "workspace_write".to_string(),
                "external_tools:trusted-ocp".to_string(),
            ],
            std::slice::from_ref(&candidate),
        )
        .expect("Kubernetes verifier binds");

        assert_eq!(summary.assigned_verifiers, 1);
        assert_eq!(summary.unassigned_verifiers, 0);
        assert!(summary.contracts[0].evidence_verifiers.is_empty());
        let binding = &summary.contracts[1].evidence_verifiers[0];
        assert_eq!(summary.contracts[1].schema_version, 5);
        assert_eq!(binding.authority_mode, "read_only");
        assert!(valid_sha256_digest(&binding.authority_capability_digest));
        assert!(binding
            .resource_identity_digest
            .as_deref()
            .is_some_and(valid_sha256_digest));
        let encoded = serde_json::to_string(&summary.contracts).expect("contracts encode");
        assert!(!encoded.contains("payroll-api"));
        assert!(!encoded.contains("prerelease"));

        let mut changed_identity = candidate.clone();
        changed_identity.resource_identity = Some(SubtaskVerifierResourceIdentity {
            resource_kind: "deployment".to_string(),
            resource_name: "payroll-api".to_string(),
            scope: "disaster-recovery".to_string(),
        });
        let changed = prepare_subtask_contracts_with_verifiers(
            &contract,
            parent_digest(),
            &["external_tools:trusted-ocp".to_string()],
            &[changed_identity],
        )
        .expect("changed resource identity prepares");
        assert_ne!(
            summary.contracts[1].contract_id, changed.contracts[1].contract_id,
            "resolved resource identity must be bound into contract identity"
        );
    }

    #[test]
    fn ambiguous_kubernetes_objectives_and_missing_connector_authority_stay_closed() {
        let contract = serde_json::json!({
            "capability_plan": {"connectors": [{
                "connector_id": "trusted-ocp",
                "matched_domains": ["kubernetes"],
                "matched_intents": ["deployment health"],
                "matched_tools": ["ocp_get_deployment"]
            }]},
            "semantic_plan": {"objectives": [
                {
                    "id": "verify-one",
                    "summary": "Read the Kubernetes payroll-api Deployment",
                    "success_evidence": "payroll-api Deployment is available",
                    "operation_hints": ["get deployment"],
                    "resource_hints": ["payroll-api deployment"],
                    "mutation_expected": false
                },
                {
                    "id": "verify-two",
                    "summary": "Inspect payroll-api Deployment health in Kubernetes",
                    "success_evidence": "payroll-api Deployment remains ready",
                    "operation_hints": ["inspect deployment"],
                    "resource_hints": ["payroll-api deployment"],
                    "mutation_expected": false
                }
            ]}
        });
        let candidate = SubtaskEvidenceVerifierCandidate {
            verifier_id: "deployment-health".to_string(),
            provider: "kubernetes".to_string(),
            verification_method: "kubernetes_deployment_available_v1".to_string(),
            required_states: vec!["deployment_available".to_string()],
            required_capability: "external_tools:trusted-ocp".to_string(),
            resource_identity: Some(SubtaskVerifierResourceIdentity {
                resource_kind: "deployment".to_string(),
                resource_name: "payroll-api".to_string(),
                scope: "prerelease".to_string(),
            }),
            read_tool: None,
            read_argument: None,
        };
        let ambiguous = prepare_subtask_contracts_with_verifiers(
            &contract,
            parent_digest(),
            &["external_tools:trusted-ocp".to_string()],
            std::slice::from_ref(&candidate),
        )
        .expect("ambiguous Kubernetes binding narrows closed");
        assert_eq!(ambiguous.assigned_verifiers, 0);
        assert_eq!(ambiguous.unassigned_verifiers, 1);

        let one_objective = serde_json::json!({
            "semantic_plan": {"objectives": [contract["semantic_plan"]["objectives"][0].clone()]}
        });
        let missing_authority = prepare_subtask_contracts_with_verifiers(
            &one_objective,
            parent_digest(),
            &[],
            &[candidate],
        )
        .expect("missing Kubernetes connector authority narrows closed");
        assert_eq!(missing_authority.assigned_verifiers, 0);
        assert_eq!(missing_authority.unassigned_verifiers, 1);
    }

    #[test]
    fn binds_bamboo_verifier_to_exact_build_and_connector_read_operation() {
        let contract = serde_json::json!({
            "capability_plan": {"connectors": [{
                "connector_id": "corporate-bamboo",
                "matched_domains": ["bamboo"],
                "matched_intents": ["read build result"],
                "matched_tools": ["bamboo_get_build"]
            }]},
            "semantic_plan": {"objectives": [
                {
                    "id": "inspect-chart",
                    "summary": "Inspect the Helm chart",
                    "success_evidence": "Chart content is understood",
                    "operation_hints": ["read file"],
                    "resource_hints": ["helm chart"],
                    "mutation_expected": false
                },
                {
                    "id": "verify-build",
                    "summary": "Read Bamboo build result PAYROLL-DEPLOY-42",
                    "success_evidence": "Bamboo PAYROLL-DEPLOY-42 build is successful",
                    "operation_hints": ["get build"],
                    "resource_hints": ["PAYROLL-DEPLOY-42 build result"],
                    "mutation_expected": false
                }
            ]}
        });
        let candidate = SubtaskEvidenceVerifierCandidate {
            verifier_id: "bamboo-build-state".to_string(),
            provider: "bamboo".to_string(),
            verification_method: "bamboo_build_result_v1".to_string(),
            required_states: vec!["successful_build".to_string()],
            required_capability: "external_tools:corporate-bamboo".to_string(),
            resource_identity: Some(SubtaskVerifierResourceIdentity {
                resource_kind: "build".to_string(),
                resource_name: "PAYROLL-DEPLOY-42".to_string(),
                scope: "corporate-bamboo".to_string(),
            }),
            read_tool: Some("bamboo_get_build".to_string()),
            read_argument: Some("result_key".to_string()),
        };
        let summary = prepare_subtask_contracts_with_verifiers(
            &contract,
            parent_digest(),
            &["external_tools:corporate-bamboo".to_string()],
            std::slice::from_ref(&candidate),
        )
        .expect("Bamboo verifier binds");

        assert_eq!(summary.assigned_verifiers, 1);
        assert_eq!(summary.unassigned_verifiers, 0);
        assert!(summary.contracts[0].evidence_verifiers.is_empty());
        let binding = &summary.contracts[1].evidence_verifiers[0];
        assert_eq!(summary.contracts[1].schema_version, 5);
        assert_eq!(binding.read_tool.as_deref(), Some("bamboo_get_build"));
        assert_eq!(binding.read_argument.as_deref(), Some("result_key"));
        assert!(binding
            .resource_identity_digest
            .as_deref()
            .is_some_and(valid_sha256_digest));
        let encoded = serde_json::to_string(&summary.contracts).expect("contracts encode");
        assert!(!encoded.contains("PAYROLL-DEPLOY-42"));

        let mut changed_identity = candidate.clone();
        changed_identity.resource_identity = Some(SubtaskVerifierResourceIdentity {
            resource_kind: "build".to_string(),
            resource_name: "PAYROLL-DEPLOY-43".to_string(),
            scope: "corporate-bamboo".to_string(),
        });
        let changed = prepare_subtask_contracts_with_verifiers(
            &contract,
            parent_digest(),
            &["external_tools:corporate-bamboo".to_string()],
            &[changed_identity],
        )
        .expect("different build identity stays closed");
        assert_eq!(changed.assigned_verifiers, 0);
        assert_eq!(changed.unassigned_verifiers, 1);
    }

    #[test]
    fn bamboo_verifier_without_exact_read_tool_authority_stays_closed() {
        let contract = serde_json::json!({
            "capability_plan": {"connectors": [{
                "connector_id": "corporate-bamboo",
                "matched_domains": ["bamboo"],
                "matched_intents": ["trigger build"],
                "matched_tools": ["bamboo_trigger_build"]
            }]},
            "semantic_plan": {"objectives": [{
                "id": "verify-build",
                "summary": "Read Bamboo build result PAYROLL-DEPLOY-42",
                "success_evidence": "Bamboo PAYROLL-DEPLOY-42 build is successful",
                "operation_hints": ["get build"],
                "resource_hints": ["PAYROLL-DEPLOY-42 build result"],
                "mutation_expected": false
            }]}
        });
        let summary = prepare_subtask_contracts_with_verifiers(
            &contract,
            parent_digest(),
            &["external_tools:corporate-bamboo".to_string()],
            &[SubtaskEvidenceVerifierCandidate {
                verifier_id: "bamboo-build-state".to_string(),
                provider: "bamboo".to_string(),
                verification_method: "bamboo_build_result_v1".to_string(),
                required_states: vec!["successful_build".to_string()],
                required_capability: "external_tools:corporate-bamboo".to_string(),
                resource_identity: Some(SubtaskVerifierResourceIdentity {
                    resource_kind: "build".to_string(),
                    resource_name: "PAYROLL-DEPLOY-42".to_string(),
                    scope: "corporate-bamboo".to_string(),
                }),
                read_tool: Some("bamboo_get_build".to_string()),
                read_argument: Some("result_key".to_string()),
            }],
        )
        .expect("missing Bamboo read authority narrows closed");
        assert_eq!(summary.assigned_verifiers, 0);
        assert_eq!(summary.unassigned_verifiers, 1);
        assert!(summary.contracts[0].evidence_verifiers.is_empty());
    }

    #[test]
    fn ambiguous_or_unsupported_verifier_binding_stays_closed() {
        let contract = serde_json::json!({"semantic_plan": {"objectives": [
            {
                "id": "commit-one",
                "summary": "Create a Git commit",
                "success_evidence": "New commit exists",
                "operation_hints": ["git commit"],
                "resource_hints": ["repository"],
                "mutation_expected": true
            },
            {
                "id": "commit-two",
                "summary": "Verify the Git commit",
                "success_evidence": "Git commit is observed",
                "operation_hints": ["git status"],
                "resource_hints": ["repository"],
                "mutation_expected": false
            }
        ]}});
        let summary = prepare_subtask_contracts_with_verifiers(
            &contract,
            parent_digest(),
            &["workspace_read".to_string(), "git".to_string()],
            &[
                SubtaskEvidenceVerifierCandidate {
                    verifier_id: "git-state".to_string(),
                    provider: "git".to_string(),
                    verification_method: "git_terminal_state_v1".to_string(),
                    required_states: vec!["new_commit".to_string()],
                    required_capability: "git".to_string(),
                    resource_identity: None,
                    read_tool: None,
                    read_argument: None,
                },
                SubtaskEvidenceVerifierCandidate {
                    verifier_id: "future-state".to_string(),
                    provider: "future".to_string(),
                    verification_method: "future_read_v1".to_string(),
                    required_states: vec!["ready".to_string()],
                    required_capability: "external_tools:future".to_string(),
                    resource_identity: None,
                    read_tool: None,
                    read_argument: None,
                },
            ],
        )
        .expect("ambiguous bindings narrow closed");

        assert_eq!(summary.assigned_verifiers, 0);
        assert_eq!(summary.unassigned_verifiers, 2);
        assert!(summary
            .contracts
            .iter()
            .all(|item| item.schema_version == 5 && item.evidence_verifiers.is_empty()));

        let one_git_objective = serde_json::json!({"semantic_plan": {"objectives": [{
            "id": "commit-one",
            "summary": "Create a Git commit",
            "success_evidence": "New commit exists",
            "operation_hints": ["git commit"],
            "resource_hints": ["repository"],
            "mutation_expected": true
        }]}});
        let missing_authority = prepare_subtask_contracts_with_verifiers(
            &one_git_objective,
            parent_digest(),
            &["workspace_read".to_string()],
            &[SubtaskEvidenceVerifierCandidate {
                verifier_id: "git-state".to_string(),
                provider: "git".to_string(),
                verification_method: "git_terminal_state_v1".to_string(),
                required_states: vec!["new_commit".to_string()],
                required_capability: "git".to_string(),
                resource_identity: None,
                read_tool: None,
                read_argument: None,
            }],
        )
        .expect("missing Git authority narrows closed");
        assert_eq!(missing_authority.assigned_verifiers, 0);
        assert_eq!(missing_authority.unassigned_verifiers, 1);
        assert!(missing_authority.contracts[0].evidence_verifiers.is_empty());
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
        assert_eq!(contracts[0].schema_version, 2);
        assert_eq!(
            contracts[0]
                .allowed_connector_tools
                .get("external_tools:confluence"),
            Some(&vec!["confluence_get_page".to_string()])
        );
        assert_eq!(
            contracts[1].granted_capabilities,
            vec!["external_tools:bamboo".to_string()]
        );
        assert_eq!(
            contracts[1]
                .allowed_connector_tools
                .get("external_tools:bamboo"),
            Some(&vec!["bamboo_trigger_build".to_string()])
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
        assert!(first[0].allowed_connector_tools.is_empty());
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
        assert_eq!(
            contracts[0]
                .allowed_connector_tools
                .get("external_tools:future-system"),
            Some(&vec!["promoteRelease".to_string()])
        );
    }

    #[test]
    fn connector_operation_class_must_match_the_objective() {
        let contracts = prepare_subtask_contracts(
            &serde_json::json!({
                "semantic_plan": {"objectives": [{
                    "id": "inspect-issue",
                    "summary": "Inspect the Jira issue",
                    "success_evidence": "Issue state is observed",
                    "operation_hints": ["read issue"],
                    "resource_hints": ["jira issue"],
                    "mutation_expected": false
                }]},
                "capability_plan": {"connectors": [{
                    "connector_id": "jira",
                    "matched_domains": ["jira"],
                    "matched_intents": ["issue"],
                    "matched_tools": ["jira_update_issue"]
                }]}
            }),
            parent_digest(),
            &["external_tools:jira".to_string()],
        )
        .expect("mismatched planned operation narrows closed");

        assert!(contracts[0].granted_capabilities.is_empty());
        assert!(contracts[0].allowed_connector_tools.is_empty());
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
