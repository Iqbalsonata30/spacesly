use super::execution_engine::{
    TaskEventReporter, TaskExecutionContext, TaskExecutionError, TaskExecutor,
};
use crate::domain::execution_manifest::{ExecutionManifestDraft, ExecutionModelConfiguration};
use crate::domain::governance::{
    ConnectorRuleFact, DeploymentTargetRuleFact, GovernanceResolutionRecord, RepositoryRuleFact,
    RuleFactsRecord,
};
use crate::domain::task_examination::{
    examine_task, ConnectorConfigurationPreflightRecord, ConnectorDiscoveryStatus,
    DeploymentTargetResolutionRecord, EvidenceVerifierBindingRecord, RepositoryResolutionRecord,
    RuleContradictionRecord, VerificationPolicyBindingRecord,
};
use crate::domain::task_recovery::{
    decide_capability_repair, decide_runtime_recovery, RuntimeFailureClass, RuntimeRecoveryAction,
    RuntimeRecoveryContext,
};
use crate::domain::task_session::{
    AgentTaskCompletionStatus, AgentTaskObjectiveResult, AgentTaskObjectiveToolReceipt,
    AgentTaskResult, TaskExecutionOutput, TaskMcpConnectorContext, TaskProgress,
    TaskSessionEnvelope, TaskSessionEnvelopeV1, TaskSessionEventKind, TaskSessionKind,
};
use crate::infrastructure::ai_worker::{
    execute_ai_worker_task, AiWorkerCompletionStatus, AiWorkerConfig, AiWorkerEventCallback,
    AiWorkerMcpServer, AiWorkerStreamEvent, AiWorkerTask, AiWorkerTaskResult,
};
use crate::infrastructure::git::repository_root_at;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_AUTOMATIC_RUNTIME_RETRIES: u8 = 1;
const MAX_REPOSITORY_DISCOVERY_DEPTH: usize = 4;
const MAX_REPOSITORY_DISCOVERY_DIRECTORIES: usize = 4_096;
const MAX_EVIDENCE_POLL_INTERVAL_SECONDS: u64 = 30;
const MAX_EVIDENCE_TIMEOUT_SECONDS: u64 = 600;

struct RepositoryPreflight {
    record: Option<RepositoryResolutionRecord>,
    repository_root: Option<PathBuf>,
    blocker: Option<String>,
}

struct DeploymentTargetPreflight {
    record: Option<DeploymentTargetResolutionRecord>,
    target: Option<DeploymentTargetRuleFact>,
    blocker: Option<String>,
}

fn rule_source_reference(source: &str, source_line: u32) -> String {
    format!(
        "{}:{}",
        if source.trim().is_empty() {
            "global.agent_rules"
        } else {
            source
        },
        source_line
    )
}

fn resolve_rule_contradictions(
    contract: &Value,
    requested_capabilities: &[String],
    connector_ids: &[String],
    rule_facts: &RuleFactsRecord,
) -> Vec<RuleContradictionRecord> {
    let contract_text = serde_json::to_string(contract)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let labels = contract
        .pointer("/ticket/labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let mut contradictions = Vec::new();

    let distinct_repository_ids = rule_facts
        .repositories
        .iter()
        .map(|repository| repository.id.as_str())
        .collect::<HashSet<_>>();
    let repository_ids = rule_facts
        .repositories
        .iter()
        .filter(|repository| {
            distinct_repository_ids.len() == 1
                || contract_text.contains(&repository.id.to_ascii_lowercase())
                || contract_text.contains(&repository.remote_url.to_ascii_lowercase())
        })
        .map(|repository| repository.id.as_str())
        .collect::<HashSet<_>>();
    for id in repository_ids {
        let definitions = rule_facts
            .repositories
            .iter()
            .filter(|repository| repository.id == id)
            .collect::<Vec<_>>();
        if definitions.len() > 1 {
            contradictions.push(RuleContradictionRecord {
                schema_version: 1,
                domain: "repository".to_string(),
                key: id.to_string(),
                source_references: definitions
                    .iter()
                    .map(|fact| rule_source_reference(&fact.source, fact.source_line))
                    .collect(),
                reason: "Multiple authoritative repository definitions use the same repository ID."
                    .to_string(),
            });
        }
    }

    let selected_labels = rule_facts
        .deployment_targets
        .iter()
        .filter(|target| {
            labels
                .iter()
                .any(|label| label.eq_ignore_ascii_case(&target.label))
        })
        .map(|target| target.label.as_str())
        .collect::<HashSet<_>>();
    for label in selected_labels {
        let definitions = rule_facts
            .deployment_targets
            .iter()
            .filter(|target| target.label.eq_ignore_ascii_case(label))
            .collect::<Vec<_>>();
        if definitions.len() > 1 {
            contradictions.push(RuleContradictionRecord {
                schema_version: 1,
                domain: "deployment_target".to_string(),
                key: label.to_string(),
                source_references: definitions
                    .iter()
                    .map(|fact| rule_source_reference(&fact.source, fact.source_line))
                    .collect(),
                reason:
                    "One ticket label maps to multiple deployment targets, branches, or namespaces."
                        .to_string(),
            });
        }
    }
    if let Some(selector) = contract
        .pointer("/deployment/target")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let definitions = rule_facts
            .deployment_targets
            .iter()
            .filter(|target| target.target.eq_ignore_ascii_case(selector))
            .collect::<Vec<_>>();
        if definitions.len() > 1 {
            contradictions.push(RuleContradictionRecord {
                schema_version: 1,
                domain: "deployment_target".to_string(),
                key: format!("target:{selector}"),
                source_references: definitions
                    .iter()
                    .map(|fact| rule_source_reference(&fact.source, fact.source_line))
                    .collect(),
                reason: "One explicit target name maps to multiple deployment Rules rows."
                    .to_string(),
            });
        }
    }

    for connector_id in connector_ids {
        let definitions = rule_facts
            .connectors
            .iter()
            .filter(|connector| connector.id == *connector_id)
            .collect::<Vec<_>>();
        if definitions.len() > 1 {
            contradictions.push(RuleContradictionRecord {
                schema_version: 1,
                domain: "connector".to_string(),
                key: connector_id.clone(),
                source_references: definitions
                    .iter()
                    .map(|fact| rule_source_reference(&fact.source, fact.source_line))
                    .collect(),
                reason:
                    "Multiple authoritative Connector Rules use the same configured connector ID."
                        .to_string(),
            });
        }
    }

    let applicable_policies = rule_facts
        .verification_policies
        .iter()
        .filter(|policy| connector_ids.iter().any(|id| id == &policy.connector_id))
        .filter(|policy| {
            policy.applies_to_labels.is_empty()
                || labels.iter().any(|label| {
                    policy
                        .applies_to_labels
                        .iter()
                        .any(|required| label.eq_ignore_ascii_case(required))
                })
        })
        .collect::<Vec<_>>();
    let policy_ids = applicable_policies
        .iter()
        .map(|policy| policy.id.as_str())
        .collect::<HashSet<_>>();
    for policy_id in policy_ids {
        let definitions = applicable_policies
            .iter()
            .filter(|policy| policy.id == policy_id)
            .collect::<Vec<_>>();
        if definitions.len() > 1 {
            contradictions.push(RuleContradictionRecord {
                schema_version: 1,
                domain: "verification".to_string(),
                key: policy_id.to_string(),
                source_references: definitions
                    .iter()
                    .map(|fact| rule_source_reference(&fact.source, fact.source_line))
                    .collect(),
                reason: "Multiple applicable Verification Rules use the same policy ID."
                    .to_string(),
            });
        }
    }
    let applicable_verifiers = rule_facts
        .evidence_verifiers
        .iter()
        .filter(|verifier| {
            (verifier.provider == "git"
                && requested_capabilities
                    .iter()
                    .any(|capability| capability == "git"))
                || connector_ids.iter().any(|connector_id| {
                    rule_facts.connectors.iter().any(|connector| {
                        connector.id == *connector_id
                            && connector.connector_type == verifier.provider
                    })
                })
        })
        .filter(|verifier| {
            verifier.applies_to_labels.is_empty()
                || labels.iter().any(|label| {
                    verifier
                        .applies_to_labels
                        .iter()
                        .any(|required| label.eq_ignore_ascii_case(required))
                })
        })
        .collect::<Vec<_>>();
    let verifier_ids = applicable_verifiers
        .iter()
        .map(|verifier| verifier.id.as_str())
        .collect::<HashSet<_>>();
    for verifier_id in verifier_ids {
        let definitions = applicable_verifiers
            .iter()
            .filter(|verifier| verifier.id == verifier_id)
            .collect::<Vec<_>>();
        if definitions.len() > 1 {
            contradictions.push(RuleContradictionRecord {
                schema_version: 1,
                domain: "evidence_verifier".to_string(),
                key: verifier_id.to_string(),
                source_references: definitions
                    .iter()
                    .map(|fact| rule_source_reference(&fact.source, fact.source_line))
                    .collect(),
                reason: "Multiple applicable Evidence Verifier Rules use the same ID.".to_string(),
            });
        }
    }
    contradictions
        .sort_by(|left, right| (&left.domain, &left.key).cmp(&(&right.domain, &right.key)));
    contradictions
}

fn resolve_evidence_verifier_bindings(
    contract: &Value,
    requested_capabilities: &[String],
    connector_ids: &[String],
    rule_facts: &RuleFactsRecord,
    repository_root: Option<&Path>,
    deployment_target: Option<&DeploymentTargetRuleFact>,
    trusted_kubernetes_connector: bool,
) -> (Vec<EvidenceVerifierBindingRecord>, Vec<String>) {
    let labels = contract
        .pointer("/ticket/labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let mut records = Vec::new();
    let mut blockers = Vec::new();
    for verifier in &rule_facts.evidence_verifiers {
        let provider_requested = if verifier.provider == "git" {
            requested_capabilities
                .iter()
                .any(|capability| capability == "git")
        } else {
            connector_ids.iter().any(|connector_id| {
                rule_facts.connectors.iter().any(|connector| {
                    connector.id == *connector_id
                        && (connector.connector_type == verifier.provider
                            || (verifier.provider == "kubernetes"
                                && matches!(
                                    connector.connector_type.as_str(),
                                    "ocp" | "openshift"
                                )))
                })
            })
        };
        if !provider_requested {
            continue;
        }
        let matched_labels = labels
            .iter()
            .filter(|label| {
                verifier
                    .applies_to_labels
                    .iter()
                    .any(|required| label.eq_ignore_ascii_case(required))
            })
            .map(|label| (*label).to_string())
            .collect::<Vec<_>>();
        if !verifier.applies_to_labels.is_empty() && matched_labels.is_empty() {
            continue;
        }
        let valid_states = match verifier.provider.as_str() {
            "git" => verifier.required_states.iter().all(|state| {
                matches!(
                    state.as_str(),
                    "clean_worktree" | "new_commit" | "pushed_upstream"
                )
            }),
            "kubernetes" => verifier.required_states == ["deployment_available"],
            _ => false,
        };
        let workload = contract
            .pointer("/deployment/workload")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| canonical_connector_token(value));
        let polling_configured =
            verifier.poll_interval_seconds.is_some() || verifier.timeout_seconds.is_some();
        let valid_polling = verifier.polling_configuration_error.is_none()
            && verifier.poll_interval_seconds.is_some() == verifier.timeout_seconds.is_some()
            && verifier.poll_interval_seconds.is_none_or(|interval| {
                (1..=MAX_EVIDENCE_POLL_INTERVAL_SECONDS).contains(&interval)
                    && verifier.timeout_seconds.is_some_and(|timeout| {
                        (interval..=MAX_EVIDENCE_TIMEOUT_SECONDS).contains(&timeout)
                    })
            })
            && (verifier.provider == "kubernetes" || !polling_configured);
        let (status, reason) = if !canonical_connector_token(&verifier.id)
            || !canonical_connector_token(&verifier.provider)
            || verifier.required_states.is_empty()
            || verifier
                .required_states
                .iter()
                .any(|state| !canonical_connector_token(state))
        {
            (
                "invalid_rule",
                "Evidence Verifier fields must be canonical non-empty identifiers.",
            )
        } else if !valid_polling {
            (
                "invalid_rule",
                "Evidence polling requires Kubernetes plus both bounded interval and timeout seconds.",
            )
        } else if !matches!(verifier.provider.as_str(), "git" | "kubernetes") {
            (
                "unsupported_provider",
                "This evidence verifier provider has no deterministic adapter yet.",
            )
        } else if !valid_states {
            (
                "invalid_rule",
                "Evidence Verifier required states are not supported by this provider adapter.",
            )
        } else if verifier.provider == "kubernetes" && !trusted_kubernetes_connector {
            (
                "unsupported_provider",
                "Kubernetes evidence verification requires the trusted embedded connector adapter.",
            )
        } else if verifier.provider == "git" && repository_root.is_none() {
            (
                "missing_repository",
                "Git evidence verification requires one resolved trusted repository.",
            )
        } else if verifier.provider == "git"
            && verifier
                .required_states
                .iter()
                .any(|state| state == "new_commit")
            && contract
                .pointer("/repository/head_commit")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_none_or(str::is_empty)
        {
            ("missing_repository", "The immutable contract must include repository.head_commit for new_commit verification.")
        } else if verifier.provider == "kubernetes"
            && (deployment_target.is_none() || workload.is_none())
        {
            ("missing_resource", "Kubernetes deployment availability requires a resolved deployment target and canonical deployment.workload.")
        } else {
            let reason = if verifier.provider == "kubernetes" {
                "The Deployment verifier is bound to the trusted connector and resolved resource identity."
            } else {
                "The Git terminal-state verifier is bound to the resolved trusted repository."
            };
            ("ready", reason)
        };
        if status != "ready" {
            blockers.push(format!("{}: {reason}", verifier.id));
        }
        records.push(EvidenceVerifierBindingRecord {
            schema_version: 1,
            verifier_id: verifier.id.clone(),
            provider: verifier.provider.clone(),
            status: status.to_string(),
            matched_labels,
            required_states: verifier.required_states.clone(),
            resource_kind: (verifier.provider == "kubernetes").then(|| "deployment".to_string()),
            resource_name: (verifier.provider == "kubernetes")
                .then(|| workload.map(str::to_string))
                .flatten(),
            namespace: (verifier.provider == "kubernetes")
                .then(|| deployment_target.map(|target| target.namespace.clone()))
                .flatten(),
            poll_interval_seconds: (verifier.provider == "kubernetes")
                .then_some(verifier.poll_interval_seconds)
                .flatten(),
            timeout_seconds: (verifier.provider == "kubernetes")
                .then_some(verifier.timeout_seconds)
                .flatten(),
            source: verifier.source.clone(),
            source_line: verifier.source_line,
            reason: reason.to_string(),
        });
    }
    records.sort_by(|left, right| left.verifier_id.cmp(&right.verifier_id));
    blockers.sort();
    (records, blockers)
}

fn run_git_evidence_command(repository_root: &Path, arguments: &[&str]) -> Result<String, String> {
    let git = crate::infrastructure::git::git_executable()?;
    let output = std::process::Command::new(git)
        .args(arguments)
        .current_dir(repository_root)
        .output()
        .map_err(|error| format!("Could not run Git evidence verifier: {error}"))?;
    if !output.status.success() {
        return Err("Git evidence command did not succeed.".to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn verify_git_evidence_states(
    bindings: &[EvidenceVerifierBindingRecord],
    repository_root: &Path,
    contract: &Value,
) -> (Vec<Value>, Vec<String>) {
    let required = bindings
        .iter()
        .filter(|binding| binding.provider == "git" && binding.status == "ready")
        .flat_map(|binding| binding.required_states.iter().cloned())
        .collect::<HashSet<_>>();
    let mut evidence = Vec::new();
    let mut failures = Vec::new();
    for state in ["clean_worktree", "new_commit", "pushed_upstream"] {
        if !required.contains(state) {
            continue;
        }
        let result = match state {
            "clean_worktree" => {
                run_git_evidence_command(repository_root, &["status", "--porcelain"])
                    .map(|status| status.is_empty())
            }
            "new_commit" => {
                run_git_evidence_command(repository_root, &["rev-parse", "HEAD"]).map(|head| {
                    contract
                        .pointer("/repository/head_commit")
                        .and_then(Value::as_str)
                        .is_some_and(|baseline| !baseline.trim().eq_ignore_ascii_case(&head))
                })
            }
            "pushed_upstream" => {
                run_git_evidence_command(repository_root, &["rev-parse", "--verify", "@{u}"])
                    .and_then(|_| {
                        run_git_evidence_command(
                            repository_root,
                            &["merge-base", "--is-ancestor", "HEAD", "@{u}"],
                        )
                    })
                    .map(|_| true)
            }
            _ => unreachable!(),
        };
        match result {
            Ok(true) => evidence.push(json!({"state": state, "status": "satisfied"})),
            Ok(false) => {
                evidence.push(json!({"state": state, "status": "unsatisfied"}));
                failures.push(state.to_string());
            }
            Err(_) => {
                evidence.push(json!({"state": state, "status": "unavailable"}));
                failures.push(state.to_string());
            }
        }
    }
    (evidence, failures)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeploymentAvailabilityPollStatus {
    Satisfied,
    Unsatisfied,
    TimedOut,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DeploymentAvailabilityPollResult {
    status: DeploymentAvailabilityPollStatus,
    evidence: Option<crate::infrastructure::ocp::DeploymentAvailabilityEvidence>,
    attempts: u32,
}

fn poll_deployment_availability<Read, Ensure, Wait, Elapsed, ControlError>(
    interval: Option<Duration>,
    timeout: Option<Duration>,
    mut read: Read,
    mut ensure_current: Ensure,
    mut wait: Wait,
    mut elapsed: Elapsed,
) -> Result<DeploymentAvailabilityPollResult, ControlError>
where
    Read: FnMut(
        Duration,
    ) -> Result<crate::infrastructure::ocp::DeploymentAvailabilityEvidence, String>,
    Ensure: FnMut() -> Result<(), ControlError>,
    Wait: FnMut(Duration) -> Result<(), ControlError>,
    Elapsed: FnMut() -> Duration,
{
    let polling = interval.zip(timeout);
    let mut attempts = 0_u32;
    let mut last_evidence = None;
    loop {
        ensure_current()?;
        let before_read = elapsed();
        if attempts > 0 && polling.is_some_and(|(_, deadline)| before_read >= deadline) {
            return Ok(DeploymentAvailabilityPollResult {
                status: DeploymentAvailabilityPollStatus::TimedOut,
                evidence: last_evidence,
                attempts,
            });
        }
        let request_budget = polling
            .map(|(_, deadline)| deadline.saturating_sub(before_read))
            .unwrap_or(Duration::from_secs(30))
            .max(Duration::from_millis(1));
        attempts = attempts.saturating_add(1);
        let evidence = match read(request_budget) {
            Ok(evidence) => evidence,
            Err(_) => {
                return Ok(DeploymentAvailabilityPollResult {
                    status: DeploymentAvailabilityPollStatus::Unavailable,
                    evidence: None,
                    attempts,
                });
            }
        };
        let after_read = elapsed();
        if evidence.available && polling.is_none_or(|(_, deadline)| after_read <= deadline) {
            return Ok(DeploymentAvailabilityPollResult {
                status: DeploymentAvailabilityPollStatus::Satisfied,
                evidence: Some(evidence),
                attempts,
            });
        }
        last_evidence = Some(evidence);
        let Some((interval, deadline)) = polling else {
            return Ok(DeploymentAvailabilityPollResult {
                status: DeploymentAvailabilityPollStatus::Unsatisfied,
                evidence: last_evidence,
                attempts,
            });
        };
        if after_read >= deadline {
            return Ok(DeploymentAvailabilityPollResult {
                status: DeploymentAvailabilityPollStatus::TimedOut,
                evidence: last_evidence,
                attempts,
            });
        }
        wait(interval.min(deadline.saturating_sub(after_read)))?;
    }
}

fn deployment_availability_evidence_json(result: &DeploymentAvailabilityPollResult) -> Value {
    let status = match result.status {
        DeploymentAvailabilityPollStatus::Satisfied => "satisfied",
        DeploymentAvailabilityPollStatus::Unsatisfied => "unsatisfied",
        DeploymentAvailabilityPollStatus::TimedOut => "timed_out",
        DeploymentAvailabilityPollStatus::Unavailable => "unavailable",
    };
    let mut value = json!({
        "state": "deployment_available",
        "status": status,
        "attempts": result.attempts,
    });
    if let Some(evidence) = &result.evidence {
        value["desired_replicas"] = json!(evidence.desired_replicas);
        value["updated_replicas"] = json!(evidence.updated_replicas);
        value["ready_replicas"] = json!(evidence.ready_replicas);
        value["available_replicas"] = json!(evidence.available_replicas);
        value["generation_observed"] = json!(evidence.generation_observed);
    }
    value
}

fn matching_connector_tool(
    operation: &str,
    connector_id: &str,
    connector_type: &str,
    tools: &[crate::domain::task_examination::DiscoveredToolCapability],
) -> Result<String, &'static str> {
    let names = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let exact = names
        .iter()
        .copied()
        .filter(|name| *name == operation)
        .collect::<Vec<_>>();
    let typed_name = format!("{connector_type}_{operation}");
    let typed = names
        .iter()
        .copied()
        .filter(|name| *name == typed_name)
        .collect::<Vec<_>>();
    let connector_name = format!("{connector_id}_{operation}");
    let connector_scoped = names
        .iter()
        .copied()
        .filter(|name| *name == connector_name)
        .collect::<Vec<_>>();
    let suffix = format!("_{operation}");
    let fallback = names
        .iter()
        .copied()
        .filter(|name| name.ends_with(&suffix))
        .collect::<Vec<_>>();
    let candidates = [exact, typed, connector_scoped, fallback]
        .into_iter()
        .find(|candidates| !candidates.is_empty())
        .unwrap_or_default();
    match candidates.as_slice() {
        [tool] => Ok((*tool).to_string()),
        [] => Err("missing_operations"),
        _ => Err("ambiguous_operation"),
    }
}

fn resolve_verification_policy_bindings(
    contract: &Value,
    connector_ids: &[String],
    rule_facts: &RuleFactsRecord,
    capabilities: &[crate::domain::task_examination::ConnectorCapabilitySnapshot],
) -> (Vec<VerificationPolicyBindingRecord>, Vec<String>) {
    let labels = contract
        .pointer("/ticket/labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .collect::<Vec<_>>();
    let mut records = Vec::new();
    let mut blockers = Vec::new();
    for policy in &rule_facts.verification_policies {
        if !connector_ids.iter().any(|id| id == &policy.connector_id) {
            continue;
        }
        let matched_labels = labels
            .iter()
            .filter(|label| {
                policy
                    .applies_to_labels
                    .iter()
                    .any(|required| label.eq_ignore_ascii_case(required))
            })
            .map(|label| (*label).to_string())
            .collect::<Vec<_>>();
        if !policy.applies_to_labels.is_empty() && matched_labels.is_empty() {
            continue;
        }
        let connector_rule = rule_facts
            .connectors
            .iter()
            .find(|rule| rule.id == policy.connector_id);
        let capability = capabilities.iter().find(|capability| {
            capability.connector_id == policy.connector_id
                && capability.status == ConnectorDiscoveryStatus::Available
        });
        let invalid = !canonical_connector_token(&policy.id)
            || !canonical_connector_token(&policy.connector_id)
            || policy.required_operations.is_empty()
            || policy
                .required_operations
                .iter()
                .any(|operation| !canonical_connector_operation(operation));
        let (status, verified_tools, reason) = if invalid || connector_rule.is_none() {
            ("invalid_rule", Vec::new(), "Verification policy must reference one valid Connector Rule and declare canonical required operations.")
        } else if let (Some(rule), Some(capability)) = (connector_rule, capability) {
            let resolved = policy
                .required_operations
                .iter()
                .map(|operation| {
                    matching_connector_tool(
                        operation,
                        &policy.connector_id,
                        &rule.connector_type,
                        &capability.tools,
                    )
                })
                .collect::<Result<Vec<_>, _>>();
            match resolved {
                Ok(tools) => ("ready", tools, "Required successful connector operations were bound to the live tool inventory."),
                Err(_) => ("missing_operations", Vec::new(), "One or more verification operations are absent or ambiguous in the live tool inventory."),
            }
        } else {
            (
                "missing_operations",
                Vec::new(),
                "Verification policy connector has no usable live tool inventory.",
            )
        };
        if status != "ready" {
            blockers.push(format!("{}: {reason}", policy.id));
        }
        records.push(VerificationPolicyBindingRecord {
            schema_version: 1,
            policy_id: policy.id.clone(),
            connector_id: policy.connector_id.clone(),
            status: status.to_string(),
            matched_labels,
            required_operations: policy.required_operations.clone(),
            verified_tools,
            source: policy.source.clone(),
            source_line: policy.source_line,
            reason: reason.to_string(),
        });
    }
    records.sort_by(|left, right| left.policy_id.cmp(&right.policy_id));
    blockers.sort();
    (records, blockers)
}

fn resolve_connector_configuration_preflights(
    connector_ids: &[String],
    rule_facts: &RuleFactsRecord,
    servers: &[AiWorkerMcpServer],
    capabilities: &[crate::domain::task_examination::ConnectorCapabilitySnapshot],
) -> (Vec<ConnectorConfigurationPreflightRecord>, Vec<String>) {
    if rule_facts.connectors.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut records = Vec::new();
    let mut blockers = Vec::new();
    for connector_id in connector_ids {
        let matching_rules = rule_facts
            .connectors
            .iter()
            .filter(|rule| rule.id == *connector_id)
            .collect::<Vec<_>>();
        let (record, blocker) = match matching_rules.as_slice() {
            [] => connector_preflight_failure(
                connector_id,
                None,
                "missing_rule",
                "Requested connector has no matching user-defined Connector Rule.",
            ),
            [rule] => resolve_connector_rule(rule, servers, capabilities),
            _ => connector_preflight_failure(
                connector_id,
                None,
                "invalid_rule",
                "Multiple Connector Rules use the same connector ID.",
            ),
        };
        if let Some(blocker) = blocker {
            blockers.push(format!("{connector_id}: {blocker}"));
        }
        records.push(record);
    }
    records.sort_by(|left, right| left.connector_id.cmp(&right.connector_id));
    blockers.sort();
    blockers.dedup();
    (records, blockers)
}

fn resolve_connector_rule(
    rule: &ConnectorRuleFact,
    servers: &[AiWorkerMcpServer],
    capabilities: &[crate::domain::task_examination::ConnectorCapabilitySnapshot],
) -> (ConnectorConfigurationPreflightRecord, Option<String>) {
    if !canonical_connector_token(&rule.id)
        || !canonical_connector_token(&rule.connector_type)
        || rule.required_operations.is_empty()
        || rule
            .required_operations
            .iter()
            .any(|operation| !canonical_connector_operation(operation))
    {
        return connector_preflight_failure(
            &rule.id,
            Some(rule),
            "invalid_rule",
            "Connector Rule type and required operations must be non-empty canonical identifiers.",
        );
    }
    let Some(rule_url) = normalized_connector_base_url(&rule.base_url) else {
        return connector_preflight_failure(
            &rule.id,
            Some(rule),
            "invalid_rule",
            "Connector Rule Base URL must be an absolute HTTP(S) URL without credentials, query, or fragment.",
        );
    };
    let Some(server) = servers.iter().find(|server| server.secret_id == rule.id) else {
        return connector_preflight_failure(
            &rule.id,
            Some(rule),
            "missing_configuration",
            "Connector Rule has no configured MCP connector with the same ID.",
        );
    };
    let configured_urls = server
        .environment
        .iter()
        .filter(|(key, _)| {
            let key = key.to_ascii_uppercase();
            key.contains("URL") || key.contains("SERVER") || key.contains("BASE")
        })
        .filter_map(|(_, value)| normalized_connector_base_url(value))
        .collect::<Vec<_>>();
    if configured_urls.is_empty() {
        return connector_preflight_failure(
            &rule.id,
            Some(rule),
            "missing_configuration",
            "Configured connector does not expose a valid secret-free base URL setting.",
        );
    }
    if !configured_urls.iter().any(|url| url == &rule_url) {
        return connector_preflight_failure(
            &rule.id,
            Some(rule),
            "url_mismatch",
            "Configured connector Base URL does not match its authoritative Connector Rule.",
        );
    }
    let Some(capability) = capabilities
        .iter()
        .find(|capability| capability.connector_id == rule.id)
    else {
        return connector_preflight_failure(
            &rule.id,
            Some(rule),
            "connector_unavailable",
            "Connector has no live capability snapshot.",
        );
    };
    if capability.status != ConnectorDiscoveryStatus::Available {
        return connector_preflight_failure(
            &rule.id,
            Some(rule),
            "connector_unavailable",
            "Connector did not expose a usable live tool inventory.",
        );
    }
    let mut verified_tools = Vec::new();
    for operation in &rule.required_operations {
        match matching_connector_tool(operation, &rule.id, &rule.connector_type, &capability.tools)
        {
            Ok(tool) => verified_tools.push(tool),
            Err("missing_operations") => {
                return connector_preflight_failure(
                    &rule.id,
                    Some(rule),
                    "missing_operations",
                    "Connector live inventory is missing one or more required operations.",
                );
            }
            Err(_) => {
                return connector_preflight_failure(
                    &rule.id,
                    Some(rule),
                    "ambiguous_operation",
                    "A required operation matches multiple live tools; use the exact tool name in Rules.",
                );
            }
        }
    }
    verified_tools.sort();
    verified_tools.dedup();
    (
        ConnectorConfigurationPreflightRecord {
            schema_version: 1,
            connector_id: rule.id.clone(),
            connector_type: Some(rule.connector_type.clone()),
            status: "ready".to_string(),
            base_url: Some(rule_url),
            required_operations: rule.required_operations.clone(),
            verified_tools,
            source: rule.source.clone(),
            source_line: rule.source_line,
            reason:
                "Connector configuration and required live operations match the authoritative Rule."
                    .to_string(),
        },
        None,
    )
}

fn connector_preflight_failure(
    connector_id: &str,
    rule: Option<&ConnectorRuleFact>,
    status: &str,
    reason: &str,
) -> (ConnectorConfigurationPreflightRecord, Option<String>) {
    (
        ConnectorConfigurationPreflightRecord {
            schema_version: 1,
            connector_id: connector_id.to_string(),
            connector_type: rule
                .map(|rule| rule.connector_type.clone())
                .filter(|value| !value.is_empty()),
            status: status.to_string(),
            base_url: rule.and_then(|rule| normalized_connector_base_url(&rule.base_url)),
            required_operations: rule
                .map(|rule| rule.required_operations.clone())
                .unwrap_or_default(),
            verified_tools: Vec::new(),
            source: rule
                .map(|rule| rule.source.clone())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "global.agent_rules".to_string()),
            source_line: rule.map(|rule| rule.source_line).unwrap_or(0),
            reason: reason.to_string(),
        },
        Some(reason.to_string()),
    )
}

fn canonical_connector_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn canonical_connector_operation(value: &str) -> bool {
    canonical_connector_token(value)
        || (!value.is_empty()
            && value.len() <= 128
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/' | b':')
            }))
}

fn normalized_connector_base_url(value: &str) -> Option<String> {
    if value != value.trim() {
        return None;
    }
    let mut url = url::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.has_host()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let normalized_path = url.path().trim_end_matches('/').to_string();
    url.set_path(if normalized_path.is_empty() {
        "/"
    } else {
        &normalized_path
    });
    Some(url.to_string())
}

fn resolve_deployment_target_preflight(
    contract: &Value,
    rule_facts: &RuleFactsRecord,
) -> DeploymentTargetPreflight {
    let labels = contract
        .pointer("/ticket/labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .collect::<Vec<_>>();
    let mut label_matches = rule_facts
        .deployment_targets
        .iter()
        .filter(|target| {
            labels
                .iter()
                .any(|label| label.eq_ignore_ascii_case(&target.label))
        })
        .cloned()
        .collect::<Vec<_>>();
    sort_and_dedup_deployment_targets(&mut label_matches);
    let explicit_selector = match contract.pointer("/deployment/target") {
        None | Some(Value::Null) => None,
        Some(Value::String(value))
            if canonical_connector_token(value.trim()) && value == value.trim() =>
        {
            Some(value.as_str())
        }
        Some(_) => {
            let reason =
                "Execution Contract deployment.target must be a non-empty canonical identifier.";
            return DeploymentTargetPreflight {
                record: Some(deployment_target_resolution_record(
                    "invalid",
                    "explicit_target",
                    None,
                    None,
                    reason,
                )),
                target: None,
                blocker: Some(reason.to_string()),
            };
        }
    };
    let mut explicit_matches = explicit_selector
        .map(|selector| {
            rule_facts
                .deployment_targets
                .iter()
                .filter(|target| target.target.eq_ignore_ascii_case(selector))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    sort_and_dedup_deployment_targets(&mut explicit_matches);

    if label_matches.len() > 1 {
        let reason = "Ticket labels select multiple conflicting deployment targets; keep exactly one environment label or correct the Rules table.";
        return DeploymentTargetPreflight {
            record: Some(deployment_target_resolution_record(
                "ambiguous",
                "ticket_label",
                None,
                None,
                reason,
            )),
            target: None,
            blocker: Some(reason.to_string()),
        };
    }
    if explicit_matches.len() > 1 {
        let reason =
            "The explicit deployment target matches multiple Rules rows; make target names unique.";
        return DeploymentTargetPreflight {
            record: Some(deployment_target_resolution_record(
                "ambiguous",
                "explicit_target",
                explicit_selector,
                None,
                reason,
            )),
            target: None,
            blocker: Some(reason.to_string()),
        };
    }
    if explicit_selector.is_some() && explicit_matches.is_empty() {
        let reason =
            "The explicit deployment target has no exact match in the authoritative Rules table.";
        return DeploymentTargetPreflight {
            record: Some(deployment_target_resolution_record(
                "unresolved",
                "explicit_target",
                explicit_selector,
                None,
                reason,
            )),
            target: None,
            blocker: Some(reason.to_string()),
        };
    }
    let label_target = label_matches.first();
    let explicit_target = explicit_matches.first();
    if let (Some(label_target), Some(explicit_target)) = (label_target, explicit_target) {
        if label_target.target != explicit_target.target
            || label_target.branch != explicit_target.branch
            || label_target.namespace != explicit_target.namespace
        {
            let reason =
                "The ticket label and explicit deployment target select different Rules rows.";
            return DeploymentTargetPreflight {
                record: Some(deployment_target_resolution_record(
                    "conflict",
                    "combined",
                    explicit_selector,
                    None,
                    reason,
                )),
                target: None,
                blocker: Some(reason.to_string()),
            };
        }
    }
    let (target, selector_kind, selector_value, reason) = match (label_target, explicit_target) {
        (Some(target), Some(_)) => (
            Some(target),
            "combined",
            explicit_selector,
            "The ticket label and explicit target selected the same user-defined deployment target.",
        ),
        (Some(target), None) => (
            Some(target),
            "ticket_label",
            Some(target.label.as_str()),
            "An exact ticket label selected one user-defined deployment target.",
        ),
        (None, Some(target)) => (
            Some(target),
            "explicit_target",
            explicit_selector,
            "The explicit Execution Contract target selected one user-defined deployment target.",
        ),
        (None, None) => (None, "", None, ""),
    };
    let Some(target) = target else {
        return DeploymentTargetPreflight {
            record: None,
            target: None,
            blocker: None,
        };
    };
    if !valid_rule_branch(&target.branch) || !valid_kubernetes_namespace(&target.namespace) {
        let reason = "The matched deployment target contains an invalid Git branch or Kubernetes namespace; correct the Rules table.";
        return DeploymentTargetPreflight {
            record: Some(deployment_target_resolution_record(
                "invalid",
                selector_kind,
                selector_value,
                Some(target),
                reason,
            )),
            target: None,
            blocker: Some(reason.to_string()),
        };
    }
    DeploymentTargetPreflight {
        record: Some(deployment_target_resolution_record(
            "resolved",
            selector_kind,
            selector_value,
            Some(target),
            reason,
        )),
        target: Some(target.clone()),
        blocker: None,
    }
}

fn sort_and_dedup_deployment_targets(targets: &mut Vec<DeploymentTargetRuleFact>) {
    targets.sort_by(|left, right| {
        (&left.label, &left.target, &left.branch, &left.namespace).cmp(&(
            &right.label,
            &right.target,
            &right.branch,
            &right.namespace,
        ))
    });
    targets.dedup_by(|left, right| {
        left.label.eq_ignore_ascii_case(&right.label)
            && left.target == right.target
            && left.branch == right.branch
            && left.namespace == right.namespace
    });
}

fn valid_rule_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && !value.ends_with(['.', '/'])
        && !value.ends_with(".lock")
        && !value.contains("..")
        && !value.contains("//")
        && !value.contains("@{")
        && !value.bytes().any(|byte| {
            byte <= b' ' || matches!(byte, b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
}

fn valid_kubernetes_namespace(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn deployment_target_resolution_record(
    status: &str,
    selector_kind: &str,
    selector_value: Option<&str>,
    target: Option<&DeploymentTargetRuleFact>,
    reason: &str,
) -> DeploymentTargetResolutionRecord {
    DeploymentTargetResolutionRecord {
        schema_version: 2,
        status: status.to_string(),
        selector_kind: (!selector_kind.is_empty()).then(|| selector_kind.to_string()),
        selector_value: selector_value.map(str::to_string),
        matched_label: target.map(|value| value.label.clone()),
        target: target.map(|value| value.target.clone()),
        branch: target.map(|value| value.branch.clone()),
        namespace: target.map(|value| value.namespace.clone()),
        source: target
            .map(|value| value.source.clone())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "global.agent_rules".to_string()),
        source_line: target.map(|value| value.source_line).unwrap_or(0),
        reason: reason.to_string(),
    }
}

fn resolve_repository_preflight(
    contract: &Value,
    rule_facts: &RuleFactsRecord,
    workspace_root: &Path,
) -> RepositoryPreflight {
    if rule_facts.repositories.is_empty() {
        return RepositoryPreflight {
            record: None,
            repository_root: None,
            blocker: None,
        };
    }
    let contract_text = serde_json::to_string(contract)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let matching = rule_facts
        .repositories
        .iter()
        .filter(|repository| {
            contract_text.contains(&repository.id.to_ascii_lowercase())
                || contract_text.contains(&repository.remote_url.to_ascii_lowercase())
        })
        .collect::<Vec<_>>();
    let selected = if matching.len() == 1 {
        Some(matching[0])
    } else if matching.is_empty() && rule_facts.repositories.len() == 1 {
        rule_facts.repositories.first()
    } else {
        None
    };
    let Some(repository) = selected else {
        let reason = if matching.len() > 1 {
            "The task matches multiple repository Rules; declare one repository root in the execution contract."
        } else {
            "Multiple repository Rules are available, but the task does not identify which repository to use."
        };
        return repository_preflight_failure("ambiguous", None, reason);
    };
    let contract_path = contract
        .pointer("/repository/root_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let (Some(contract_path), Some(rule_path)) =
        (contract_path, repository.local_path.as_deref())
    {
        let contract_candidate = absolute_repository_candidate(workspace_root, contract_path);
        let rule_candidate = absolute_repository_candidate(workspace_root, rule_path);
        let resolved_contract = contract_candidate
            .canonicalize()
            .unwrap_or(contract_candidate);
        let resolved_rule = rule_candidate.canonicalize().unwrap_or(rule_candidate);
        if resolved_contract != resolved_rule {
            return repository_preflight_failure(
                "conflict",
                Some(repository),
                "The execution contract repository root conflicts with the authoritative Rules checkout path.",
            );
        }
    }
    let candidate = contract_path
        .or(repository.local_path.as_deref())
        .map(|path| absolute_repository_candidate(workspace_root, path));
    let candidate = match candidate {
        Some(candidate) => candidate,
        None => match discover_named_repositories(workspace_root, &repository.id) {
            Ok((matches, false)) if matches.len() == 1 => matches[0].clone(),
            Ok((matches, _)) if matches.len() > 1 => {
                return repository_preflight_failure(
                    "ambiguous",
                    Some(repository),
                    "More than one contained Git checkout matches the repository Rule; declare its exact local checkout path.",
                );
            }
            Ok((_, true)) => {
                return repository_preflight_failure(
                    "missing_checkout",
                    Some(repository),
                    "Bounded repository discovery was exhausted; declare the exact local checkout path in Rules.",
                );
            }
            Ok(_) => {
                return repository_preflight_failure(
                    "missing_checkout",
                    Some(repository),
                    "The repository Rule has no local checkout path and no matching contained Git checkout was discovered. Add `Local checkout: /absolute/path` to Rules.",
                );
            }
            Err(error) => {
                return repository_preflight_failure(
                    "invalid_checkout",
                    Some(repository),
                    &format!("Repository discovery failed: {error}"),
                );
            }
        },
    };
    let canonical_workspace = match workspace_root.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            return repository_preflight_failure(
                "invalid_checkout",
                Some(repository),
                "The trusted workspace root does not exist or cannot be resolved.",
            );
        }
    };
    let canonical = match candidate.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            return repository_preflight_failure(
                "missing_checkout",
                Some(repository),
                "The configured repository checkout does not exist or cannot be resolved.",
            );
        }
    };
    if !canonical.starts_with(&canonical_workspace) {
        return repository_preflight_failure(
            "outside_workspace",
            Some(repository),
            "The configured repository checkout escapes the trusted workspace root.",
        );
    }
    match repository_root_at(&canonical) {
        Ok(Some(root)) if root == canonical => {}
        Ok(Some(_)) => {
            return repository_preflight_failure(
                "invalid_checkout",
                Some(repository),
                "The configured checkout path is inside a repository but is not its root.",
            );
        }
        Ok(None) => {
            return repository_preflight_failure(
                "invalid_checkout",
                Some(repository),
                "The configured checkout path is not a Git repository root.",
            );
        }
        Err(error) => {
            return repository_preflight_failure(
                "invalid_checkout",
                Some(repository),
                &format!("Git could not validate the configured checkout: {error}"),
            );
        }
    }
    RepositoryPreflight {
        record: Some(repository_resolution_record(
            "resolved",
            Some(repository),
            Some(&canonical),
            "A unique contained Git checkout was resolved from the immutable contract and Rules.",
        )),
        repository_root: Some(canonical),
        blocker: None,
    }
}

fn repository_preflight_failure(
    status: &str,
    repository: Option<&RepositoryRuleFact>,
    reason: &str,
) -> RepositoryPreflight {
    RepositoryPreflight {
        record: Some(repository_resolution_record(
            status, repository, None, reason,
        )),
        repository_root: None,
        blocker: Some(reason.to_string()),
    }
}

fn repository_resolution_record(
    status: &str,
    repository: Option<&RepositoryRuleFact>,
    local_path: Option<&Path>,
    reason: &str,
) -> RepositoryResolutionRecord {
    RepositoryResolutionRecord {
        schema_version: 1,
        status: status.to_string(),
        repository_id: repository.map(|value| value.id.clone()),
        remote_url: repository.map(|value| sanitize_repository_url(&value.remote_url)),
        local_path: local_path.map(|path| path.to_string_lossy().to_string()),
        backend_path: repository.and_then(|value| value.backend_path.clone()),
        frontend_path: repository.and_then(|value| value.frontend_path.clone()),
        source: repository
            .map(|value| value.source.clone())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "global.agent_rules".to_string()),
        source_line: repository.map(|value| value.source_line).unwrap_or(0),
        reason: reason.to_string(),
    }
}

fn sanitize_repository_url(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value) else {
        return "invalid-repository-url".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn absolute_repository_candidate(workspace_root: &Path, value: &str) -> PathBuf {
    let candidate = PathBuf::from(value);
    if candidate.is_absolute() {
        candidate
    } else {
        workspace_root.join(candidate)
    }
}

fn discover_named_repositories(
    workspace_root: &Path,
    repository_id: &str,
) -> Result<(Vec<PathBuf>, bool), String> {
    let mut pending = vec![(workspace_root.to_path_buf(), 0usize)];
    let mut matches = Vec::new();
    let mut inspected = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        inspected = inspected.saturating_add(1);
        if inspected > MAX_REPOSITORY_DISCOVERY_DIRECTORIES {
            return Ok((matches, true));
        }
        if directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(repository_id))
            && repository_root_at(&directory)?.as_deref() == Some(directory.as_path())
        {
            matches.push(directory);
            continue;
        }
        if depth >= MAX_REPOSITORY_DISCOVERY_DEPTH {
            continue;
        }
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if depth == 0 => {
                return Err(format!("Could not read '{}': {error}", directory.display()));
            }
            Err(_) => continue,
        };
        let mut children = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let file_type = entry.file_type().ok()?;
                let name = entry.file_name();
                let name = name.to_str()?;
                (file_type.is_dir()
                    && !file_type.is_symlink()
                    && !matches!(name, ".git" | "node_modules" | "target" | ".cache"))
                .then(|| entry.path())
            })
            .collect::<Vec<_>>();
        children.sort();
        pending.extend(children.into_iter().rev().map(|child| (child, depth + 1)));
    }
    matches.sort();
    matches.dedup();
    Ok((matches, false))
}

#[derive(Clone, Debug, Default)]
struct RuntimeAttemptObservation {
    successful_tool_calls: u32,
    successful_mutation_observed: bool,
    in_flight_tools: HashMap<String, AgentTaskObjectiveToolReceipt>,
    uncheckpointed_tool_receipts: Vec<AgentTaskObjectiveToolReceipt>,
    opencode_session_id: Option<String>,
    failed_tool: Option<String>,
    failed_tool_risk: Option<String>,
}

struct RuntimeCallbackGate {
    open: Arc<Mutex<bool>>,
}

impl Drop for RuntimeCallbackGate {
    fn drop(&mut self) {
        if let Ok(mut open) = self.open.lock() {
            *open = false;
        }
    }
}

fn runtime_callback(
    reporter: TaskEventReporter,
    callback_gate: Arc<Mutex<bool>>,
    observation: Arc<Mutex<RuntimeAttemptObservation>>,
    objective_mutations: Arc<HashMap<String, bool>>,
    protected_mutations: Arc<Mutex<HashMap<(String, String), String>>>,
    successful_receipts: Arc<Mutex<Vec<AgentTaskObjectiveToolReceipt>>>,
) -> AiWorkerEventCallback {
    Box::new(move |event| {
        let open = callback_gate.lock().map_err(|error| error.to_string())?;
        if !*open {
            return Err("Agent runtime callback is closed.".to_string());
        }
        if let AiWorkerStreamEvent::OpenCodeSession { session_id, .. } = &event {
            reporter
                .bind_opencode_session(session_id)
                .map_err(|error| error.to_string())?;
            observation
                .lock()
                .map_err(|error| error.to_string())?
                .opencode_session_id = Some(session_id.clone());
            return Ok(());
        }
        if let AiWorkerStreamEvent::ObjectiveCheckpoint {
            objective_id,
            evidence,
        } = &event
        {
            let mutation_expected = objective_mutations.get(objective_id).ok_or_else(|| {
                format!("Agent checkpoint referenced unknown objective '{objective_id}'.")
            })?;
            let tool_receipts = {
                let mut observation = observation.lock().map_err(|error| error.to_string())?;
                std::mem::take(&mut observation.uncheckpointed_tool_receipts)
            };
            if *mutation_expected
                && !tool_receipts
                    .iter()
                    .any(|receipt| !matches!(receipt.risk.as_str(), "read"))
            {
                return Err(format!(
                    "Mutation objective '{objective_id}' cannot checkpoint before a successful mutation tool event."
                ));
            }
            let checkpoint_event = reporter
                .record_objective_checkpoint(objective_id, evidence, &tool_receipts)
                .map_err(|error| error.to_string())?;
            if checkpoint_event.payload["new_checkpoint"] == true {
                let mut protected = protected_mutations
                    .lock()
                    .map_err(|error| error.to_string())?;
                for receipt in tool_receipts
                    .iter()
                    .filter(|receipt| receipt.risk != "read")
                {
                    protected.insert(
                        (receipt.tool_name.clone(), receipt.arguments_digest.clone()),
                        objective_id.clone(),
                    );
                }
            }
            return Ok(());
        }
        if let AiWorkerStreamEvent::ToolStarted {
            tool_call_id,
            tool_name,
            risk,
            arguments_digest,
            ..
        } = &event
        {
            let normalized_tool_name = tool_name.trim().to_ascii_lowercase();
            if let Some(objective_id) = protected_mutations
                .lock()
                .map_err(|error| error.to_string())?
                .get(&(normalized_tool_name.clone(), arguments_digest.clone()))
                .cloned()
            {
                return Err(format!(
                    "Tool call '{tool_name}' replays completed objective '{objective_id}' with identical arguments."
                ));
            }
            observation
                .lock()
                .map_err(|error| error.to_string())?
                .in_flight_tools
                .insert(
                    tool_call_id.clone(),
                    AgentTaskObjectiveToolReceipt {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: normalized_tool_name,
                        risk: match risk.trim().to_ascii_lowercase().as_str() {
                            "low" => "read".to_string(),
                            normalized => normalized.to_string(),
                        },
                        arguments_digest: arguments_digest.clone(),
                        resource_operation_key: None,
                    },
                );
        }
        if let AiWorkerStreamEvent::ToolCompleted {
            tool_call_id,
            tool_name,
            success,
            error,
            risk,
            arguments_digest,
            arguments_observed,
            resource_operation_key,
            ..
        } = &event
        {
            if *success {
                let mut observation = observation.lock().map_err(|error| error.to_string())?;
                let normalized_tool_name = tool_name.trim().to_ascii_lowercase();
                let mut receipt = match observation.in_flight_tools.remove(tool_call_id) {
                    Some(receipt)
                        if receipt.tool_name != normalized_tool_name
                            || (*arguments_observed
                                && receipt.arguments_digest != *arguments_digest) =>
                    {
                        return Err(format!(
                            "Tool completion '{tool_call_id}' did not match its started tool identity."
                        ));
                    }
                    Some(receipt) => receipt,
                    None => AgentTaskObjectiveToolReceipt {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: normalized_tool_name,
                        risk: match risk.trim().to_ascii_lowercase().as_str() {
                            "low" => "read".to_string(),
                            normalized => normalized.to_string(),
                        },
                        arguments_digest: arguments_digest.clone(),
                        resource_operation_key: None,
                    },
                };
                receipt.resource_operation_key = resource_operation_key.clone();
                observation.successful_tool_calls =
                    observation.successful_tool_calls.saturating_add(1);
                if !matches!(risk.trim().to_ascii_lowercase().as_str(), "read" | "low") {
                    observation.successful_mutation_observed = true;
                }
                successful_receipts
                    .lock()
                    .map_err(|error| error.to_string())?
                    .push(receipt.clone());
                observation.uncheckpointed_tool_receipts.push(receipt);
            } else {
                let mut attempt = observation.lock().map_err(|error| error.to_string())?;
                attempt.in_flight_tools.remove(tool_call_id);
                attempt.failed_tool = Some(tool_name.clone());
                attempt.failed_tool_risk = Some(risk.clone());
                drop(attempt);
                if error
                    .as_deref()
                    .is_some_and(|error| error.contains("[approval_required]"))
                {
                    reporter
                        .emit_event(
                            TaskSessionEventKind::Runtime,
                            json!({
                                "type": "approval_requested",
                                "operation": tool_name,
                                "arguments_digest": arguments_digest,
                            }),
                        )
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        emit_runtime_event(&reporter, event)
    })
}

fn missing_verification_tools(
    policies: &[VerificationPolicyBindingRecord],
    checkpoints: &[crate::domain::task_session::AgentTaskObjectiveCheckpoint],
    current_receipts: &[AgentTaskObjectiveToolReceipt],
) -> Vec<String> {
    let observed = checkpoints
        .iter()
        .flat_map(|checkpoint| checkpoint.tool_receipts.iter())
        .chain(current_receipts.iter())
        .map(|receipt| receipt.tool_name.trim().to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut missing = policies
        .iter()
        .filter(|policy| policy.status == "ready")
        .flat_map(|policy| {
            let observed = &observed;
            policy
                .verified_tools
                .iter()
                .filter(move |tool| !observed.contains(&tool.trim().to_ascii_lowercase()))
                .map(|tool| format!("{}:{tool}", policy.policy_id))
        })
        .collect::<Vec<_>>();
    missing.sort();
    missing.dedup();
    missing
}

/// Trusted configuration and contract reconstructed from one durable Task Session envelope.
pub struct ResolvedAgentTask {
    pub runtime_profile_id: String,
    pub config: AiWorkerConfig,
    pub task: AiWorkerTask,
    pub governance: GovernanceResolutionRecord,
    pub connector_capabilities: Vec<crate::domain::task_examination::ConnectorCapabilitySnapshot>,
}

/// Backend authority that resolves profiles, contracts, secrets, and trusted workspace paths.
pub trait AgentRuntimeResolver: Send + Sync + 'static {
    fn resolve(
        &self,
        task_session_id: u64,
        envelope: &TaskSessionEnvelopeV1,
        runtime_attempt_id: &str,
        retained_governance: Option<&GovernanceResolutionRecord>,
    ) -> Result<ResolvedAgentTask, String>;
}

/// Provider/OpenCode invocation boundary used to test orchestration without external processes.
pub trait AgentRuntimeRunner: Send + Sync + 'static {
    fn execute(
        &self,
        config: AiWorkerConfig,
        task: AiWorkerTask,
        cancellation: Arc<AtomicBool>,
        on_event: AiWorkerEventCallback,
    ) -> Result<AiWorkerTaskResult, String>;
}

/// Production runner backed by the existing provider/OpenCode implementation.
pub struct AiWorkerRuntimeRunner;

impl AgentRuntimeRunner for AiWorkerRuntimeRunner {
    fn execute(
        &self,
        config: AiWorkerConfig,
        task: AiWorkerTask,
        cancellation: Arc<AtomicBool>,
        on_event: AiWorkerEventCallback,
    ) -> Result<AiWorkerTaskResult, String> {
        execute_ai_worker_task(config, task, cancellation, Some(on_event))
    }
}

/// Scheduler executor that applies assignment fencing before invoking a real Agent runtime.
pub struct AgentTaskExecutor {
    resolver: Arc<dyn AgentRuntimeResolver>,
    runner: Arc<dyn AgentRuntimeRunner>,
}

impl AgentTaskExecutor {
    /// Creates an Agent executor with a trusted resolver and runtime invocation boundary.
    ///
    /// The executor is shared by scheduler Workers, while each call to `execute` receives a distinct
    /// `TaskExecutionContext` with its own cancellation token, fencing token, MCP authority, and
    /// activity timeline.
    pub fn new(
        resolver: Arc<dyn AgentRuntimeResolver>,
        runner: Arc<dyn AgentRuntimeRunner>,
    ) -> Self {
        Self { resolver, runner }
    }
}

impl TaskExecutor for AgentTaskExecutor {
    fn execute(
        &self,
        context: &TaskExecutionContext,
    ) -> Result<TaskExecutionOutput, TaskExecutionError> {
        context.ensure_current()?;
        let envelope = context
            .request()
            .envelope()
            .map_err(TaskExecutionError::new)?
            .ok_or_else(|| TaskExecutionError::new("Agent task envelope is required."))?;
        let envelope = match envelope {
            TaskSessionEnvelope::V1(envelope) => envelope,
            TaskSessionEnvelope::V2(_) => {
                return Err(TaskExecutionError::new(
                    "AgentTaskExecutor requires a V1 Agent envelope.",
                ));
            }
        };
        if envelope.kind != TaskSessionKind::Agent {
            return Err(TaskExecutionError::new(
                "AgentTaskExecutor only accepts Agent Task Sessions.",
            ));
        }

        context.report_progress(
            TaskProgress {
                phase: "resolving_runtime".to_string(),
                completed: 0,
                total: None,
            },
            json!({ "runtime_profile_id": envelope.runtime_profile_id }),
        )?;
        let runtime_attempt_id = context.runtime_attempt_id();
        let runtime_preparation =
            crate::infrastructure::performance::span("runtime_preparation_ms", "agent_runtime")
                .with_context("task_session_id", context.session_id().0.to_string())
                .with_context("execution_attempt", context.attempt_id().to_string())
                .with_context("worker_id", context.worker_id().to_string())
                .with_context("runtime_id", runtime_attempt_id.clone());
        let retained_governance = context.governance_resolution()?;
        let mut resolved = self
            .resolver
            .resolve(
                context.session_id().0,
                &envelope,
                &runtime_attempt_id,
                retained_governance.as_ref(),
            )
            .map_err(TaskExecutionError::new)?;
        validate_resolved_task(&envelope, &resolved).map_err(TaskExecutionError::new)?;
        if retained_governance.is_none() {
            context.bind_governance_resolution(&resolved.governance)?;
        }
        context.emit_event(
            TaskSessionEventKind::Runtime,
            json!({
                "type": "governance_resolved",
                "task_session_id": resolved.governance.task_session_id,
                "status": resolved.governance.status,
                "rules_revision": envelope.rules_revision,
                "skill_catalog_revision": resolved.governance.skills.catalog_revision,
                "selected_skill_ids": resolved.governance.skills.selected_skill_ids,
                "selected_skill_count": resolved.governance.skills.selected_skill_ids.len(),
                "resolved_rule_count": resolved.governance.rules.entries.len(),
                "rules_prompt_bytes": resolved.governance.rules.snapshot.len(),
                "skills_prompt_bytes": resolved.governance.skills.snapshot.len(),
                "reused": retained_governance.is_some(),
            }),
        )?;
        let opencode_resolution = crate::infrastructure::performance::span(
            "opencode_session_resolution_ms",
            "agent_runtime",
        );
        let opencode_session_id = context.opencode_session_id()?;
        let opencode_resolution = if let Some(session_id) = opencode_session_id.as_deref() {
            opencode_resolution.with_context("opencode_session_id", session_id)
        } else {
            opencode_resolution
        };
        resolved.task.opencode_session_id = opencode_session_id.clone();
        opencode_resolution.finish();
        if resolved.config.runtime != "opencode" {
            return Err(TaskExecutionError::new(
                "Scheduler Agent execution requires the isolated fenced OpenCode runtime.",
            ));
        }
        resolved.task.session_key = Some(format!("task-session:{}", context.session_id().0));
        resolved.config.opencode_auto_approve = false;
        resolved.config.restrict_tools = false;
        resolved.config.fenced_tools_only = true;
        resolved.config.isolated_opencode_process = true;
        let granted_capabilities = context
            .capability_grants()
            .iter()
            .map(|grant| grant.capability.as_str())
            .collect::<HashSet<_>>();
        let connectors = envelope
            .connector_ids
            .iter()
            .map(|connector_id| {
                let capability = format!("external_tools:{connector_id}");
                TaskMcpConnectorContext {
                    connector_id: connector_id.clone(),
                    requested: true,
                    granted: granted_capabilities.contains(capability.as_str()),
                    capability,
                }
            })
            .collect::<Vec<_>>();
        let mut examination = examine_task(
            resolved
                .task
                .execution_contract
                .as_ref()
                .expect("validated execution contract"),
            &envelope.context_digest,
            &envelope,
            &granted_capabilities,
            &resolved.governance.rules.facts,
            &resolved.connector_capabilities,
        );
        let (connector_configuration_preflights, connector_configuration_blockers) =
            resolve_connector_configuration_preflights(
                &envelope.connector_ids,
                &resolved.governance.rules.facts,
                &resolved.config.mcp_servers,
                &resolved.connector_capabilities,
            );
        examination.connector_configuration_preflights = connector_configuration_preflights;
        let (verification_policy_bindings, verification_policy_blockers) =
            resolve_verification_policy_bindings(
                resolved
                    .task
                    .execution_contract
                    .as_ref()
                    .expect("validated execution contract"),
                &envelope.connector_ids,
                &resolved.governance.rules.facts,
                &resolved.connector_capabilities,
            );
        examination.verification_policy_bindings = verification_policy_bindings;
        let bound_verification_policies = examination.verification_policy_bindings.clone();
        examination.rule_contradictions = resolve_rule_contradictions(
            resolved
                .task
                .execution_contract
                .as_ref()
                .expect("validated execution contract"),
            &envelope.requested_capabilities,
            &envelope.connector_ids,
            &resolved.governance.rules.facts,
        );
        let rule_contradiction_blockers = examination
            .rule_contradictions
            .iter()
            .map(|record| format!("{} '{}': {}", record.domain, record.key, record.reason))
            .collect::<Vec<_>>();
        let objective_checkpoints = context.objective_checkpoints()?;
        let protected_mutations = Arc::new(Mutex::new(
            objective_checkpoints
                .iter()
                .flat_map(|checkpoint| {
                    checkpoint
                        .tool_receipts
                        .iter()
                        .filter(|receipt| receipt.risk != "read")
                        .map(|receipt| {
                            (
                                (receipt.tool_name.clone(), receipt.arguments_digest.clone()),
                                checkpoint.objective_id.clone(),
                            )
                        })
                })
                .collect::<HashMap<_, _>>(),
        ));
        let semantic_objective_mutations = resolved
            .task
            .execution_contract
            .as_ref()
            .and_then(|contract| contract.get("semantic_plan"))
            .and_then(|plan| plan.get("objectives"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|objective| {
                Some((
                    objective.get("id")?.as_str()?.to_string(),
                    objective
                        .get("mutation_expected")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                ))
            })
            .collect::<HashMap<_, _>>();
        if objective_checkpoints
            .iter()
            .any(|checkpoint| !semantic_objective_mutations.contains_key(&checkpoint.objective_id))
        {
            return Err(TaskExecutionError::new(
                "Retained objective checkpoint does not belong to the immutable semantic plan.",
            ));
        }
        examination.objective_checkpoints = objective_checkpoints.clone();
        let deployment_target_preflight = resolve_deployment_target_preflight(
            resolved
                .task
                .execution_contract
                .as_ref()
                .expect("validated execution contract"),
            &resolved.governance.rules.facts,
        );
        examination.deployment_target_resolution = deployment_target_preflight.record.clone();
        if let Some(target) = deployment_target_preflight.target.as_ref() {
            for policy in &resolved.governance.rules.facts.protected_branches {
                if policy.approval_required
                    && policy
                        .branches
                        .iter()
                        .any(|branch| branch == &target.branch)
                {
                    examination
                        .approval_boundaries
                        .push(format!("protected_branch:{}", target.branch));
                }
            }
            examination.approval_boundaries.sort();
            examination.approval_boundaries.dedup();
        }
        let (workspace_git_preflight, default_repository_root, repository_preflight_blocker) =
            if envelope
                .requested_capabilities
                .iter()
                .any(|capability| capability == "git")
            {
                resolved.config.opencode_workdir.as_deref().map_or(
                (None, None, None),
                |workdir| {
                let configured = std::path::PathBuf::from(workdir);
                let canonical = configured.canonicalize().ok();
                let repository_preflight = resolve_repository_preflight(
                    resolved
                        .task
                        .execution_contract
                        .as_ref()
                        .expect("validated execution contract"),
                    &resolved.governance.rules.facts,
                    &configured,
                );
                examination.repository_resolution = repository_preflight.record.clone();
                if let Some(record) = repository_preflight.record.as_ref() {
                    return (
                        Some(json!({
                            "type": "workspace_git_preflight",
                            "schema_version": 2,
                            "status": record.status,
                            "workspace_root": canonical,
                            "repository_root": repository_preflight.repository_root,
                            "repository_resolution": record,
                            "guidance": repository_preflight.blocker,
                        })),
                        repository_preflight.repository_root,
                        repository_preflight.blocker,
                    );
                }
                let (status, repository_root, guidance) = match canonical.as_deref() {
                    None => (
                        "invalid_workspace",
                        None,
                        "Configure an existing trusted workspace directory before using Git.",
                    ),
                    Some(root) => match repository_root_at(root) {
                        Ok(Some(repository_root)) if repository_root == root => {
                            ("workspace_repository", Some(repository_root), "")
                        }
                        Ok(Some(repository_root)) => (
                            "repository_outside_scope",
                            Some(repository_root),
                            "Configure the repository root or one of its parent directories as the trusted workspace.",
                        ),
                        Ok(None) => (
                            "nested_repository_required",
                            None,
                            "The workspace root is not a repository. Pass the repository directory using the Git tool 'workdir' argument.",
                        ),
                        Err(_) => (
                            "git_unavailable",
                            None,
                            "Install Git or expose it through PATH or a standard package-manager profile.",
                        ),
                    },
                };
                if !guidance.is_empty()
                    && examination.warnings.len() < 64
                    && !examination.warnings.iter().any(|warning| warning == guidance)
                {
                    examination.warnings.push(guidance.to_string());
                }
                (
                    Some(json!({
                        "type": "workspace_git_preflight",
                        "schema_version": 1,
                        "status": status,
                        "workspace_root": canonical,
                        "repository_root": repository_root.clone(),
                        "guidance": guidance,
                    })),
                    repository_root,
                    None,
                )
            })
            } else {
                (None, None, None)
            };
        let evidence_contract = resolved
            .task
            .execution_contract
            .as_ref()
            .expect("validated execution contract")
            .clone();
        let expected_ocp_command = crate::infrastructure::ocp::ocp_worker_command().ok();
        let trusted_kubernetes_connector_ids = expected_ocp_command
            .as_ref()
            .into_iter()
            .flat_map(|expected| {
                resolved
                    .config
                    .mcp_servers
                    .iter()
                    .filter(move |server| &server.command == expected)
            })
            .filter(|server| envelope.connector_ids.contains(&server.secret_id))
            .filter(|server| {
                resolved
                    .governance
                    .rules
                    .facts
                    .connectors
                    .iter()
                    .any(|rule| {
                        rule.id == server.secret_id
                            && matches!(
                                rule.connector_type.as_str(),
                                "kubernetes" | "ocp" | "openshift"
                            )
                    })
            })
            .map(|server| server.secret_id.clone())
            .collect::<Vec<_>>();
        let trusted_kubernetes_connector_id = (trusted_kubernetes_connector_ids.len() == 1)
            .then(|| trusted_kubernetes_connector_ids[0].clone());
        let trusted_kubernetes_connector = trusted_kubernetes_connector_id.is_some();
        let (evidence_verifier_bindings, evidence_verifier_blockers) =
            resolve_evidence_verifier_bindings(
                &evidence_contract,
                &envelope.requested_capabilities,
                &envelope.connector_ids,
                &resolved.governance.rules.facts,
                default_repository_root.as_deref(),
                deployment_target_preflight.target.as_ref(),
                trusted_kubernetes_connector,
            );
        examination.evidence_verifier_bindings = evidence_verifier_bindings;
        let bound_evidence_verifiers = examination.evidence_verifier_bindings.clone();
        let evidence_repository_root = default_repository_root.clone();
        let semantic_objective_mutations = Arc::new(semantic_objective_mutations);
        examination
            .validate(&envelope.context_digest)
            .map_err(TaskExecutionError::new)?;
        context.emit_event(
            TaskSessionEventKind::Runtime,
            json!({
                "type": "task_examined",
                "status": examination.status,
                "objective_count": examination.objectives.len(),
                "resource_count": examination.resources.len(),
                "required_capability_count": examination.required_capabilities.len(),
                "unresolved_requirement_count": examination.unresolved_requirements.len(),
                "approval_boundary_count": examination.approval_boundaries.len(),
                "objective_checkpoint_count": examination.objective_checkpoints.len(),
                "live_connector_count": examination.connector_capabilities.iter()
                    .filter(|connector| connector.status == crate::domain::task_examination::ConnectorDiscoveryStatus::Available)
                    .count(),
                "discovered_tool_count": examination.connector_capabilities.iter()
                    .map(|connector| connector.tools.len())
                    .sum::<usize>(),
            }),
        )?;
        if let Some(preflight) = workspace_git_preflight {
            context.emit_event(TaskSessionEventKind::Runtime, preflight)?;
        }
        if let Some(record) = deployment_target_preflight.record.as_ref() {
            context.emit_event(
                TaskSessionEventKind::Runtime,
                json!({
                    "type": "deployment_target_preflight",
                    "schema_version": 1,
                    "status": record.status,
                    "resolution": record,
                    "guidance": deployment_target_preflight.blocker,
                }),
            )?;
        }
        for record in &examination.connector_configuration_preflights {
            context.emit_event(
                TaskSessionEventKind::Runtime,
                json!({
                    "type": "connector_configuration_preflight",
                    "schema_version": 1,
                    "connector_id": record.connector_id,
                    "status": record.status,
                    "connector_type": record.connector_type,
                    "base_url": record.base_url,
                    "required_operations": record.required_operations,
                    "verified_tools": record.verified_tools,
                    "source": record.source,
                    "source_line": record.source_line,
                    "reason": record.reason,
                }),
            )?;
        }
        for record in &examination.verification_policy_bindings {
            context.emit_event(
                TaskSessionEventKind::Runtime,
                json!({
                    "type": "verification_policy_preflight",
                    "schema_version": 1,
                    "policy_id": record.policy_id,
                    "connector_id": record.connector_id,
                    "status": record.status,
                    "matched_labels": record.matched_labels,
                    "required_operations": record.required_operations,
                    "verified_tools": record.verified_tools,
                    "source": record.source,
                    "source_line": record.source_line,
                    "reason": record.reason,
                }),
            )?;
        }
        for record in &examination.rule_contradictions {
            context.emit_event(
                TaskSessionEventKind::Runtime,
                json!({
                    "type": "rule_contradiction",
                    "schema_version": 1,
                    "domain": record.domain,
                    "key": record.key,
                    "source_references": record.source_references,
                    "reason": record.reason,
                }),
            )?;
        }
        for record in &examination.evidence_verifier_bindings {
            context.emit_event(
                TaskSessionEventKind::Runtime,
                json!({
                    "type": "evidence_verifier_preflight",
                    "schema_version": 1,
                    "verifier_id": record.verifier_id,
                    "provider": record.provider,
                    "status": record.status,
                    "matched_labels": record.matched_labels,
                    "required_states": record.required_states,
                    "resource_kind": record.resource_kind,
                    "resource_name": record.resource_name,
                    "namespace": record.namespace,
                    "poll_interval_seconds": record.poll_interval_seconds,
                    "timeout_seconds": record.timeout_seconds,
                    "source": record.source,
                    "source_line": record.source_line,
                    "reason": record.reason,
                }),
            )?;
        }
        let mut connector_preflight_blockers = examination
            .connector_capabilities
            .iter()
            .filter(|connector| {
                connector.status
                    == crate::domain::task_examination::ConnectorDiscoveryStatus::Unavailable
            })
            .map(|connector| {
                format!(
                    "{}: {}",
                    connector.connector_id,
                    connector
                        .error
                        .as_deref()
                        .unwrap_or("MCP tools are unavailable.")
                )
            })
            .collect::<Vec<_>>();
        connector_preflight_blockers.extend(
            examination
                .unresolved_requirements
                .iter()
                .filter(|requirement| !requirement.starts_with("Capability '"))
                .cloned(),
        );
        connector_preflight_blockers.sort();
        connector_preflight_blockers.dedup();
        resolved.task.task_examination = Some(examination.clone());
        context.bind_execution_manifest(&ExecutionManifestDraft {
            kind: envelope.kind,
            workspace_id: envelope.workspace_id.clone(),
            subject_id: envelope.subject_id.clone(),
            conversation_id: envelope.conversation_id.clone(),
            execution_run_id: envelope.execution_run_id.clone(),
            context_digest: envelope.context_digest.clone(),
            context_revision: envelope.context_revision.clone(),
            runtime: resolved.config.runtime.clone(),
            runtime_profile_id: resolved.runtime_profile_id.clone(),
            runtime_id: runtime_attempt_id.clone(),
            model: resolved.config.model.clone(),
            model_configuration: ExecutionModelConfiguration {
                provider_id: resolved.config.provider_id.clone(),
                api_style: resolved.config.api_style.clone(),
                temperature: resolved.config.temperature.to_string(),
            },
            prompt_template_version: envelope.prompt_template_version.clone(),
            rules_revision: envelope.rules_revision.clone(),
            skills_revision: envelope.skills_revision.clone(),
            rules: resolved.governance.rules.entries.clone(),
            rules_digest: resolved.governance.rules.final_digest.clone(),
            rule_facts: resolved.governance.rules.facts.clone(),
            task_examination: examination,
            skills_catalog_revision: resolved.governance.skills.catalog_revision.clone(),
            skills: resolved.governance.skills.entries.clone(),
            connectors,
            tool_permission_mode: "fenced_tools_only".to_string(),
            unknown_fields: vec![
                "git_revision".to_string(),
                "environment_fingerprint".to_string(),
                "mcp_implementation_versions".to_string(),
                "mcp_connection_ids".to_string(),
            ],
        })?;
        if !rule_contradiction_blockers.is_empty() {
            let runtime_preparation_duration = runtime_preparation.finish();
            emit_execution_trace_stage(
                context,
                "runtime_preparation",
                runtime_preparation_duration,
                "blocked",
                &runtime_attempt_id,
                opencode_session_id.as_deref(),
            );
            return Err(TaskExecutionError::blocked(format!(
                "Authoritative Rules contradict each other. {}",
                rule_contradiction_blockers.join(" ")
            )));
        }
        if !evidence_verifier_blockers.is_empty() {
            let runtime_preparation_duration = runtime_preparation.finish();
            emit_execution_trace_stage(
                context,
                "runtime_preparation",
                runtime_preparation_duration,
                "blocked",
                &runtime_attempt_id,
                opencode_session_id.as_deref(),
            );
            return Err(TaskExecutionError::blocked(format!(
                "Evidence verifier preflight blocked execution. {}",
                evidence_verifier_blockers.join(" ")
            )));
        }
        if let Some(blocker) = deployment_target_preflight.blocker.as_ref() {
            let runtime_preparation_duration = runtime_preparation.finish();
            emit_execution_trace_stage(
                context,
                "runtime_preparation",
                runtime_preparation_duration,
                "blocked",
                &runtime_attempt_id,
                opencode_session_id.as_deref(),
            );
            return Err(TaskExecutionError::blocked(format!(
                "Deployment target preflight blocked execution. {blocker}"
            )));
        }
        if !connector_configuration_blockers.is_empty() {
            let runtime_preparation_duration = runtime_preparation.finish();
            emit_execution_trace_stage(
                context,
                "runtime_preparation",
                runtime_preparation_duration,
                "blocked",
                &runtime_attempt_id,
                opencode_session_id.as_deref(),
            );
            return Err(TaskExecutionError::blocked(format!(
                "Connector configuration preflight blocked execution. {}",
                connector_configuration_blockers.join(" ")
            )));
        }
        if !verification_policy_blockers.is_empty() {
            let runtime_preparation_duration = runtime_preparation.finish();
            emit_execution_trace_stage(
                context,
                "runtime_preparation",
                runtime_preparation_duration,
                "blocked",
                &runtime_attempt_id,
                opencode_session_id.as_deref(),
            );
            return Err(TaskExecutionError::blocked(format!(
                "Verification policy preflight blocked execution. {}",
                verification_policy_blockers.join(" ")
            )));
        }
        if let Some(blocker) = repository_preflight_blocker {
            let runtime_preparation_duration = runtime_preparation.finish();
            emit_execution_trace_stage(
                context,
                "runtime_preparation",
                runtime_preparation_duration,
                "blocked",
                &runtime_attempt_id,
                opencode_session_id.as_deref(),
            );
            return Err(TaskExecutionError::blocked(format!(
                "Repository preflight blocked execution. {blocker}"
            )));
        }
        if !connector_preflight_blockers.is_empty() {
            let runtime_preparation_duration = runtime_preparation.finish();
            emit_execution_trace_stage(
                context,
                "runtime_preparation",
                runtime_preparation_duration,
                "blocked",
                &runtime_attempt_id,
                opencode_session_id.as_deref(),
            );
            return Err(TaskExecutionError::blocked(format!(
                "MCP capability preflight blocked execution. {}",
                connector_preflight_blockers.join(" ")
            )));
        }
        let workspace_root =
            resolved.config.opencode_workdir.as_deref().ok_or_else(|| {
                TaskExecutionError::new("Trusted Agent workspace root is required.")
            })?;
        let mut task_tool_authority = context.task_tool_authority(
            &envelope.workspace_id,
            std::path::PathBuf::from(workspace_root),
            &envelope.requested_capabilities,
        )?;
        if let Some(authority) = task_tool_authority.as_mut() {
            authority.default_repository_root = default_repository_root;
            authority.bound_branch = deployment_target_preflight
                .target
                .as_ref()
                .map(|target| target.branch.clone());
        }
        resolved.config.task_tool_authority = task_tool_authority;
        let mut evidence_ocp_environment = None;
        for server in &mut resolved.config.mcp_servers {
            server.name = format!("spacesly-{}", server.secret_id);
            if expected_ocp_command.as_ref() == Some(&server.command)
                && trusted_kubernetes_connector_id.as_deref() == Some(server.secret_id.as_str())
            {
                if let Some(target) = deployment_target_preflight.target.as_ref() {
                    server.environment.insert(
                        crate::infrastructure::ocp::ENV_DEFAULT_NAMESPACE.to_string(),
                        target.namespace.clone(),
                    );
                    server.environment.insert(
                        crate::infrastructure::ocp::TASK_BOUND_NAMESPACE_ENV.to_string(),
                        target.namespace.clone(),
                    );
                }
                evidence_ocp_environment = Some(server.environment.clone());
            }
            server.proxy_authority = Some(context.external_authority(
                &server.secret_id,
                &server.command,
                &server.environment,
            )?);
        }
        context.ensure_current()?;
        context.report_progress(
            TaskProgress {
                phase: "executing_runtime".to_string(),
                completed: 0,
                total: None,
            },
            json!({ "runtime_attempt_id": runtime_attempt_id }),
        )?;
        let runtime_preparation_duration = runtime_preparation.finish();
        emit_execution_trace_stage(
            context,
            "runtime_preparation",
            runtime_preparation_duration,
            "succeeded",
            &runtime_attempt_id,
            opencode_session_id.as_deref(),
        );

        let provider_request =
            crate::infrastructure::performance::span("provider_or_runtime_request_ms", "provider")
                .with_context("task_session_id", context.session_id().0.to_string())
                .with_context("runtime_id", runtime_attempt_id.clone());
        let mut retries_attempted = 0_u8;
        let mut capability_repairs_attempted = 0_u8;
        let mut retry_task = resolved.task;
        let mut retry_config = resolved.config;
        let mut terminal_recovery_action = None;
        let successful_receipts = Arc::new(Mutex::new(Vec::new()));
        let result = loop {
            context.ensure_current()?;
            let observation = Arc::new(Mutex::new(RuntimeAttemptObservation::default()));
            let callback_open = Arc::new(Mutex::new(true));
            let callback_guard = RuntimeCallbackGate {
                open: callback_open.clone(),
            };
            let callback = runtime_callback(
                context.event_reporter(),
                callback_open,
                observation.clone(),
                semantic_objective_mutations.clone(),
                protected_mutations.clone(),
                successful_receipts.clone(),
            );
            let attempt_result = self.runner.execute(
                retry_config.clone(),
                retry_task.clone(),
                context.cancellation().shared_flag(),
                callback,
            );
            drop(callback_guard);

            let recovery_input = match &attempt_result {
                Err(error) => Some(error.as_str()),
                Ok(result) if result.completion_status == AiWorkerCompletionStatus::Blocked => {
                    Some(result.blocked_reason.as_deref().unwrap_or(&result.summary))
                }
                Ok(_) => None,
            };
            let Some(recovery_input) = recovery_input else {
                break attempt_result;
            };
            let observation = observation
                .lock()
                .map_err(|error| TaskExecutionError::new(error.to_string()))?
                .clone();
            let decision = decide_runtime_recovery(
                recovery_input,
                RuntimeRecoveryContext {
                    retries_attempted,
                    max_automatic_retries: MAX_AUTOMATIC_RUNTIME_RETRIES,
                    successful_mutation_observed: observation.successful_mutation_observed,
                    cancellation_requested: context.cancellation().is_cancelled(),
                },
            );
            if decision.failure_class == RuntimeFailureClass::MissingCapability {
                let repair = retry_task.task_examination.as_ref().map(|examination| {
                    decide_capability_repair(
                        examination,
                        recovery_input,
                        observation.failed_tool.as_deref().unwrap_or_default(),
                        observation.failed_tool_risk.as_deref().unwrap_or("unknown"),
                        capability_repairs_attempted,
                        observation.successful_mutation_observed,
                    )
                });
                if let Some(repair) = repair {
                    context.emit_event(
                        TaskSessionEventKind::Runtime,
                        json!({
                            "type": "capability_repair_decision",
                            "schema_version": repair.schema_version,
                            "repairable": repair.repairable,
                            "reason": repair.reason,
                            "failed_tool": observation.failed_tool.as_deref(),
                            "connector_id": repair.guidance.as_ref().map(|guidance| guidance.connector_id.as_str()),
                            "allowed_alternatives": repair.guidance.as_ref().map(|guidance| guidance.allowed_alternatives.as_slice()).unwrap_or_default(),
                            "repairs_attempted": capability_repairs_attempted,
                        }),
                    )?;
                    if repair.repairable {
                        capability_repairs_attempted =
                            capability_repairs_attempted.saturating_add(1);
                        if let Some(examination) = retry_task.task_examination.as_mut() {
                            examination.runtime_repair = repair.guidance;
                            examination
                                .validate(&envelope.context_digest)
                                .map_err(TaskExecutionError::new)?;
                            if let Some(guidance) = examination.runtime_repair.as_ref() {
                                for server in &mut retry_config.mcp_servers {
                                    if server.secret_id == guidance.connector_id {
                                        if let Some(authority) = server.proxy_authority.as_mut() {
                                            authority.allowed_tools =
                                                guidance.allowed_alternatives.clone();
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(session_id) = observation.opencode_session_id {
                            retry_task.opencode_session_id = Some(session_id);
                        }
                        context.report_progress(
                            TaskProgress {
                                phase: "repairing_capability_plan".to_string(),
                                completed: u64::from(capability_repairs_attempted),
                                total: Some(1),
                            },
                            json!({
                                "runtime_attempt_id": runtime_attempt_id,
                                "repair_number": capability_repairs_attempted,
                            }),
                        )?;
                        continue;
                    }
                }
            }
            context.emit_event(
                TaskSessionEventKind::Runtime,
                json!({
                    "type": "runtime_recovery_decision",
                    "schema_version": decision.schema_version,
                    "failure_class": decision.failure_class,
                    "action": decision.action,
                    "retryable": decision.retryable,
                    "reason": decision.reason,
                    "retries_attempted": retries_attempted,
                    "max_automatic_retries": MAX_AUTOMATIC_RUNTIME_RETRIES,
                    "successful_tool_calls": observation.successful_tool_calls,
                    "successful_mutation_observed": observation.successful_mutation_observed,
                }),
            )?;
            if !decision.should_retry() {
                terminal_recovery_action = Some(decision.action);
                break attempt_result;
            }

            retries_attempted = retries_attempted.saturating_add(1);
            if let Some(session_id) = observation.opencode_session_id {
                retry_task.opencode_session_id = Some(session_id);
            }
            context.report_progress(
                TaskProgress {
                    phase: "recovering_runtime".to_string(),
                    completed: u64::from(retries_attempted),
                    total: Some(u64::from(MAX_AUTOMATIC_RUNTIME_RETRIES)),
                },
                json!({
                    "runtime_attempt_id": runtime_attempt_id,
                    "retry_number": retries_attempted,
                    "reason": decision.reason,
                }),
            )?;
        };
        let provider_duration = provider_request.finish();
        let provider_outcome = match &result {
            Ok(result) if result.completion_status == AiWorkerCompletionStatus::Blocked => {
                "blocked"
            }
            Ok(_) => "succeeded",
            Err(error) if error.contains("[approval_required]") => "blocked",
            Err(_) if context.cancellation().is_cancelled() => "cancelled",
            Err(_) => "failed",
        };
        let bound_opencode_session_id = context.opencode_session_id().ok().flatten();
        emit_execution_trace_stage(
            context,
            "agent_runtime_request",
            provider_duration,
            provider_outcome,
            &runtime_attempt_id,
            bound_opencode_session_id.as_deref(),
        );
        let result = result.map_err(|error| {
            if error.contains("[approval_required]")
                || matches!(
                    terminal_recovery_action,
                    Some(
                        RuntimeRecoveryAction::AwaitOperator
                            | RuntimeRecoveryAction::ReviewUncertainOutcome
                    )
                )
            {
                TaskExecutionError::blocked(error)
            } else {
                TaskExecutionError::new(error)
            }
        })?;
        context.ensure_current()?;
        context.emit_event(
            TaskSessionEventKind::Runtime,
            json!({
                "type": "agent_result_candidate",
                "authoritative": false,
                "result": result,
            }),
        )?;
        if result.completion_status == AiWorkerCompletionStatus::Completed {
            let current_receipts = successful_receipts
                .lock()
                .map_err(|error| TaskExecutionError::new(error.to_string()))?;
            let missing = missing_verification_tools(
                &bound_verification_policies,
                &objective_checkpoints,
                &current_receipts,
            );
            if !missing.is_empty() {
                context.emit_event(
                    TaskSessionEventKind::Runtime,
                    json!({
                        "type": "verification_policy_result",
                        "schema_version": 1,
                        "status": "blocked",
                        "missing_successful_tools": missing,
                    }),
                )?;
                return Err(TaskExecutionError::blocked(format!(
                    "Verification policy blocked completion because successful tool receipts are missing: {}.",
                    missing.join(", ")
                )));
            }
            if !bound_verification_policies.is_empty() {
                context.emit_event(
                    TaskSessionEventKind::Runtime,
                    json!({
                        "type": "verification_policy_result",
                        "schema_version": 1,
                        "status": "satisfied",
                        "policy_count": bound_verification_policies.len(),
                    }),
                )?;
            }
            let git_verifiers = bound_evidence_verifiers
                .iter()
                .filter(|binding| binding.provider == "git")
                .cloned()
                .collect::<Vec<_>>();
            if !git_verifiers.is_empty() {
                let repository_root = evidence_repository_root.as_deref().ok_or_else(|| {
                    TaskExecutionError::blocked(
                        "Evidence verifier lost its resolved repository authority.",
                    )
                })?;
                let (evidence, failures) =
                    verify_git_evidence_states(&git_verifiers, repository_root, &evidence_contract);
                context.emit_event(
                    TaskSessionEventKind::Runtime,
                    json!({
                        "type": "evidence_verifier_result",
                        "schema_version": 1,
                        "provider": "git",
                        "status": if failures.is_empty() { "satisfied" } else { "blocked" },
                        "evidence": evidence,
                        "failed_states": failures,
                    }),
                )?;
                if !failures.is_empty() {
                    return Err(TaskExecutionError::blocked(format!(
                        "Git evidence verification blocked completion; unsatisfied states: {}.",
                        failures.join(", ")
                    )));
                }
            }
            for binding in bound_evidence_verifiers
                .iter()
                .filter(|binding| binding.provider == "kubernetes")
            {
                let environment = evidence_ocp_environment.as_ref().ok_or_else(|| {
                    TaskExecutionError::blocked(
                        "Kubernetes evidence verifier lost its trusted connector configuration.",
                    )
                })?;
                let namespace = binding.namespace.as_deref().ok_or_else(|| {
                    TaskExecutionError::blocked("Kubernetes evidence namespace is missing.")
                })?;
                let name = binding.resource_name.as_deref().ok_or_else(|| {
                    TaskExecutionError::blocked("Kubernetes evidence resource is missing.")
                })?;
                let interval = binding.poll_interval_seconds.map(Duration::from_secs);
                let timeout = binding.timeout_seconds.map(Duration::from_secs);
                let poll_started = Instant::now();
                let result = poll_deployment_availability(
                    interval,
                    timeout,
                    |request_timeout| {
                        crate::infrastructure::ocp::verify_deployment_available(
                            environment,
                            namespace,
                            name,
                            request_timeout,
                        )
                    },
                    || context.ensure_current(),
                    |duration| {
                        let wait_started = Instant::now();
                        while wait_started.elapsed() < duration {
                            context.ensure_current()?;
                            let remaining = duration.saturating_sub(wait_started.elapsed());
                            std::thread::sleep(remaining.min(Duration::from_millis(100)));
                        }
                        context.ensure_current()
                    },
                    || poll_started.elapsed(),
                )?;
                let satisfied = result.status == DeploymentAvailabilityPollStatus::Satisfied;
                let evidence = deployment_availability_evidence_json(&result);
                context.emit_event(
                    TaskSessionEventKind::Runtime,
                    json!({
                        "type": "evidence_verifier_result",
                        "schema_version": 1,
                        "provider": "kubernetes",
                        "status": if satisfied { "satisfied" } else { "blocked" },
                        "resource_kind": "deployment",
                        "poll_interval_seconds": binding.poll_interval_seconds,
                        "timeout_seconds": binding.timeout_seconds,
                        "evidence": [evidence],
                    }),
                )?;
                if !satisfied {
                    return Err(TaskExecutionError::blocked(
                        "Kubernetes Deployment availability verification blocked completion.",
                    ));
                }
            }
        }
        let completion_status = match result.completion_status {
            AiWorkerCompletionStatus::Completed => AgentTaskCompletionStatus::Completed,
            AiWorkerCompletionStatus::Blocked => AgentTaskCompletionStatus::Blocked,
        };
        Ok(TaskExecutionOutput::Agent(AgentTaskResult {
            summary: result.summary,
            evidence: result.evidence,
            details: result.details,
            next: result.next,
            completion_status,
            blocked_reason: result.blocked_reason,
            objective_results: result
                .objective_results
                .into_iter()
                .map(|objective| AgentTaskObjectiveResult {
                    objective_id: objective.objective_id,
                    completion_status: match objective.completion_status {
                        AiWorkerCompletionStatus::Completed => AgentTaskCompletionStatus::Completed,
                        AiWorkerCompletionStatus::Blocked => AgentTaskCompletionStatus::Blocked,
                    },
                    evidence: objective.evidence,
                    blocked_reason: objective.blocked_reason,
                })
                .collect(),
        }))
    }
}

fn emit_execution_trace_stage(
    context: &TaskExecutionContext,
    stage: &'static str,
    duration: std::time::Duration,
    outcome: &'static str,
    runtime_id: &str,
    opencode_session_id: Option<&str>,
) {
    let result = context.emit_event(
        TaskSessionEventKind::Runtime,
        json!({
            "type": "execution_trace_stage",
            "schema_version": 1,
            "trace_id": format!("task-session:{}", context.session_id().0),
            "span_id": format!("attempt:{}:{stage}", context.attempt_id()),
            "stage": stage,
            "duration_us": duration.as_micros().min(u64::MAX as u128) as u64,
            "outcome": outcome,
            "worker_id": context.worker_id(),
            "runtime_id": runtime_id,
            "opencode_session_id": opencode_session_id,
        }),
    );
    if result.is_err() {
        crate::infrastructure::performance::increment(
            "execution_trace_events_dropped_total",
            "observability",
            1,
        );
    }
}

fn validate_resolved_task(
    envelope: &TaskSessionEnvelopeV1,
    resolved: &ResolvedAgentTask,
) -> Result<(), String> {
    if resolved.runtime_profile_id != envelope.runtime_profile_id {
        return Err("Resolved Agent runtime profile did not match the envelope.".to_string());
    }
    if resolved.config.workspace_id != envelope.workspace_id {
        return Err("Resolved Agent workspace did not match the envelope.".to_string());
    }
    let resolved_model = if resolved.config.runtime == "opencode" {
        resolved.config.opencode_model.as_str()
    } else {
        resolved.config.model.as_str()
    };
    if resolved_model != envelope.model {
        return Err("Resolved Agent model did not match the envelope.".to_string());
    }
    let resolved_connectors = resolved
        .config
        .mcp_servers
        .iter()
        .map(|server| server.secret_id.as_str())
        .collect::<HashSet<_>>();
    let resolved_names = resolved
        .config
        .mcp_servers
        .iter()
        .map(|server| server.name.trim())
        .collect::<HashSet<_>>();
    let requested_connectors = envelope
        .connector_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if resolved_connectors.len() != resolved.config.mcp_servers.len()
        || resolved_names.len() != resolved.config.mcp_servers.len()
        || resolved.config.mcp_servers.iter().any(|server| {
            server.name.trim().is_empty()
                || !server
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                || server.secret_id.trim().is_empty()
                || server.secret_id != server.secret_id.trim()
                || server.command.is_empty()
                || server.command[0].trim().is_empty()
        })
        || resolved_connectors != requested_connectors
    {
        return Err("Resolved Agent connectors did not match the envelope.".to_string());
    }
    let contract = resolved
        .task
        .execution_contract
        .as_ref()
        .ok_or_else(|| "Resolved Agent execution contract is required.".to_string())?;
    if execution_contract_digest(contract)? != envelope.context_digest {
        return Err(
            "Resolved Agent execution contract digest did not match the envelope.".to_string(),
        );
    }
    if resolved.config.agent_rules != resolved.governance.rules.snapshot
        || resolved.config.agent_skills != resolved.governance.skills.snapshot
    {
        return Err(
            "Runtime Rules or Skills do not match the authoritative governance snapshot."
                .to_string(),
        );
    }
    Ok(())
}

pub fn execution_contract_digest(contract: &serde_json::Value) -> Result<String, String> {
    let encoded = serde_json::to_vec(contract)
        .map_err(|error| format!("Failed to encode Agent execution contract: {error}"))?;
    Ok(format!(
        "sha256:{}",
        Sha256::digest(encoded)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

pub(crate) fn emit_runtime_event(
    reporter: &TaskEventReporter,
    event: AiWorkerStreamEvent,
) -> Result<(), String> {
    let (kind, payload, failure) = match event {
        AiWorkerStreamEvent::OpenCodeSession { session_id, action } => (
            TaskSessionEventKind::Runtime,
            json!({
                "type": "opencode_session",
                "opencode_session_id": session_id,
                "action": action,
            }),
            None,
        ),
        AiWorkerStreamEvent::TextDelta(text) => (
            TaskSessionEventKind::Runtime,
            json!({ "type": "text_delta", "text": text }),
            None,
        ),
        AiWorkerStreamEvent::ObjectiveCheckpoint {
            objective_id,
            evidence,
        } => (
            TaskSessionEventKind::Runtime,
            json!({
                "type": "objective_checkpoint_candidate",
                "objective_id": objective_id,
                "evidence": evidence,
            }),
            None,
        ),
        AiWorkerStreamEvent::ToolStarted {
            tool_call_id,
            tool_name,
            risk,
            arguments_digest,
            display_context,
        } => (
            TaskSessionEventKind::Tool,
            json!({
                "type": "tool_started",
                "tool_call_id": tool_call_id,
                "tool_name": tool_name,
                "risk": risk,
                "arguments_digest": arguments_digest,
                "display_context": display_context,
            }),
            None,
        ),
        AiWorkerStreamEvent::ToolCompleted {
            tool_call_id,
            tool_name,
            success,
            error,
            risk,
            arguments_digest,
            arguments_observed: _,
            display_context,
            resource_operation_key,
        } => {
            let failure = (!success).then(|| {
                format!(
                    "External tool '{tool_name}' failed. Cause: {}",
                    error
                        .as_deref()
                        .unwrap_or("The runtime did not provide an error detail.")
                )
            });
            (
                TaskSessionEventKind::Tool,
                json!({
                    "type": "tool_completed",
                    "tool_call_id": tool_call_id,
                    "tool_name": tool_name,
                    "success": success,
                    "error": error,
                    "risk": risk,
                    "arguments_digest": arguments_digest,
                    "display_context": display_context,
                    "resource_operation_key": resource_operation_key,
                }),
                failure,
            )
        }
        AiWorkerStreamEvent::UsageUpdated {
            input_tokens,
            output_tokens,
        } => (
            TaskSessionEventKind::Runtime,
            json!({
                "type": "usage_updated",
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
            }),
            None,
        ),
    };
    reporter
        .emit_event(kind, payload)
        .map_err(|error| error.to_string())?;
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::execution_engine::ExecutionEngine;
    use crate::domain::execution::{ExecutionRun, StepRun};
    use crate::domain::task_session::{TaskSessionEnvelope, TaskSessionState, TaskToolState};
    use crate::infrastructure::ai_worker::AiWorkerMcpServer;
    use crate::infrastructure::ai_worker::AiWorkerObjectiveResult;
    use crate::infrastructure::tool_broker::{argument_digest, ToolDisplayContext};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Barrier, Mutex};
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    struct FakeResolver {
        attempts: Arc<Mutex<Vec<String>>>,
        runtime_profile_id: String,
    }

    struct UnavailableConnectorResolver;

    struct CapabilityRepairResolver;

    struct CheckpointResolver {
        contract: serde_json::Value,
    }

    impl AgentRuntimeResolver for UnavailableConnectorResolver {
        fn resolve(
            &self,
            task_session_id: u64,
            envelope: &TaskSessionEnvelopeV1,
            runtime_attempt_id: &str,
            retained_governance: Option<&GovernanceResolutionRecord>,
        ) -> Result<ResolvedAgentTask, String> {
            let mut resolved = FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: envelope.runtime_profile_id.clone(),
            }
            .resolve(
                task_session_id,
                envelope,
                runtime_attempt_id,
                retained_governance,
            )?;
            resolved.connector_capabilities = vec![
                crate::domain::task_examination::ConnectorCapabilitySnapshot {
                    connector_id: "jira".to_string(),
                    status: crate::domain::task_examination::ConnectorDiscoveryStatus::Unavailable,
                    tools: Vec::new(),
                    error: Some("MCP tools/list discovery failed.".to_string()),
                    warnings: Vec::new(),
                },
            ];
            Ok(resolved)
        }
    }

    impl AgentRuntimeResolver for FakeResolver {
        fn resolve(
            &self,
            task_session_id: u64,
            _envelope: &TaskSessionEnvelopeV1,
            runtime_attempt_id: &str,
            retained_governance: Option<&GovernanceResolutionRecord>,
        ) -> Result<ResolvedAgentTask, String> {
            self.attempts
                .lock()
                .expect("attempt lock")
                .push(runtime_attempt_id.to_string());
            Ok(ResolvedAgentTask {
                runtime_profile_id: self.runtime_profile_id.clone(),
                config: test_config(),
                task: AiWorkerTask {
                    execution_contract: Some(json!({ "contract": "test" })),
                    task_examination: None,
                    session_key: None,
                    opencode_session_id: None,
                },
                governance: retained_governance
                    .cloned()
                    .unwrap_or_else(|| test_governance(task_session_id)),
                connector_capabilities: Vec::new(),
            })
        }
    }

    impl AgentRuntimeResolver for CapabilityRepairResolver {
        fn resolve(
            &self,
            task_session_id: u64,
            envelope: &TaskSessionEnvelopeV1,
            runtime_attempt_id: &str,
            retained_governance: Option<&GovernanceResolutionRecord>,
        ) -> Result<ResolvedAgentTask, String> {
            let mut resolved = FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: envelope.runtime_profile_id.clone(),
            }
            .resolve(
                task_session_id,
                envelope,
                runtime_attempt_id,
                retained_governance,
            )?;
            resolved.connector_capabilities = vec![
                crate::domain::task_examination::ConnectorCapabilitySnapshot {
                    connector_id: "jira".to_string(),
                    status: crate::domain::task_examination::ConnectorDiscoveryStatus::Available,
                    tools: vec![crate::domain::task_examination::DiscoveredToolCapability {
                        name: "jira_read_issue".to_string(),
                        risk: "read".to_string(),
                        argument_names: vec!["issue_key".to_string()],
                    }],
                    error: None,
                    warnings: Vec::new(),
                },
            ];
            Ok(resolved)
        }
    }

    impl AgentRuntimeResolver for CheckpointResolver {
        fn resolve(
            &self,
            task_session_id: u64,
            envelope: &TaskSessionEnvelopeV1,
            _runtime_attempt_id: &str,
            retained_governance: Option<&GovernanceResolutionRecord>,
        ) -> Result<ResolvedAgentTask, String> {
            Ok(ResolvedAgentTask {
                runtime_profile_id: envelope.runtime_profile_id.clone(),
                config: test_config(),
                task: AiWorkerTask {
                    execution_contract: Some(self.contract.clone()),
                    task_examination: None,
                    session_key: None,
                    opencode_session_id: None,
                },
                governance: retained_governance
                    .cloned()
                    .unwrap_or_else(|| test_governance(task_session_id)),
                connector_capabilities: Vec::new(),
            })
        }
    }

    struct FakeRunner {
        executions: Arc<AtomicUsize>,
    }

    type IsolationObservation = Arc<Mutex<Vec<(String, u64, u64, u64)>>>;

    struct IsolationRunner {
        barrier: Arc<Barrier>,
        seen: IsolationObservation,
        active: Arc<AtomicUsize>,
        maximum: Arc<AtomicUsize>,
        origin: Instant,
        intervals: Arc<Mutex<Vec<(u64, u128, u128)>>>,
    }

    struct BlockedRunner;

    struct ApprovalThenCompleteRunner {
        executions: AtomicUsize,
        seen_session_ids: Mutex<Vec<Option<String>>>,
    }

    struct BlockedThenCompleteRunner {
        executions: AtomicUsize,
        seen_session_ids: Mutex<Vec<Option<String>>>,
    }

    struct ToolFailureRunner;

    struct MismatchedToolCompletionRunner;

    struct TransientThenCompleteRunner {
        executions: Arc<AtomicUsize>,
        resumed_session_ids: Arc<Mutex<Vec<Option<String>>>>,
    }

    struct MutationThenTransientFailureRunner {
        executions: Arc<AtomicUsize>,
    }

    struct CapabilityDriftThenCompleteRunner {
        executions: Arc<AtomicUsize>,
        observed_alternatives: Arc<Mutex<Vec<Vec<String>>>>,
    }

    struct CheckpointThenCompleteRunner {
        executions: Arc<AtomicUsize>,
        observed_checkpoints: Arc<Mutex<Vec<Vec<String>>>>,
    }

    struct MutationCheckpointReplayRunner {
        executions: Arc<AtomicUsize>,
    }

    impl AgentRuntimeRunner for ToolFailureRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            _task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            on_event(AiWorkerStreamEvent::ToolCompleted {
                tool_call_id: "call-1".to_string(),
                tool_name: "jira_search".to_string(),
                success: false,
                error: Some("Connection refused while reading stdout.".to_string()),
                risk: "read".to_string(),
                arguments_digest: "digest".to_string(),
                arguments_observed: false,
                display_context: ToolDisplayContext {
                    label: "Reading from external tool".to_string(),
                    category: "external".to_string(),
                    target: None,
                },
                resource_operation_key: None,
            })?;
            unreachable!("failed tool callback must terminate execution")
        }
    }

    impl AgentRuntimeRunner for MismatchedToolCompletionRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            _task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            on_event(AiWorkerStreamEvent::ToolStarted {
                tool_call_id: "call-mismatch".to_string(),
                tool_name: "bamboo_trigger_build".to_string(),
                risk: "mutation".to_string(),
                arguments_digest: "a".repeat(64),
                display_context: ToolDisplayContext {
                    label: "Trigger Bamboo build".to_string(),
                    category: "bamboo".to_string(),
                    target: Some("QCASH-BUILD".to_string()),
                },
            })?;
            on_event(AiWorkerStreamEvent::ToolCompleted {
                tool_call_id: "call-mismatch".to_string(),
                tool_name: "bamboo_trigger_build".to_string(),
                success: true,
                error: None,
                risk: "mutation".to_string(),
                arguments_digest: "b".repeat(64),
                arguments_observed: true,
                display_context: ToolDisplayContext {
                    label: "Trigger Bamboo build".to_string(),
                    category: "bamboo".to_string(),
                    target: Some("QCASH-BUILD".to_string()),
                },
                resource_operation_key: None,
            })?;
            unreachable!("mismatched completion callback must terminate execution")
        }
    }

    impl AgentRuntimeRunner for TransientThenCompleteRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let execution = self.executions.fetch_add(1, Ordering::SeqCst);
            self.resumed_session_ids
                .lock()
                .expect("resumed sessions lock")
                .push(task.opencode_session_id.clone());
            if execution == 0 {
                on_event(AiWorkerStreamEvent::OpenCodeSession {
                    session_id: "opencode-recovery-session".to_string(),
                    action: "created".to_string(),
                })?;
                on_event(AiWorkerStreamEvent::ToolCompleted {
                    tool_call_id: "call-transient".to_string(),
                    tool_name: "confluence_get_page".to_string(),
                    success: false,
                    error: Some("HTTP 503 service unavailable".to_string()),
                    risk: "read".to_string(),
                    arguments_digest: "digest-read".to_string(),
                    arguments_observed: false,
                    display_context: ToolDisplayContext {
                        label: "Read Confluence page".to_string(),
                        category: "external".to_string(),
                        target: None,
                    },
                    resource_operation_key: None,
                })?;
                unreachable!("failed tool callback must terminate the first execution")
            }
            Ok(AiWorkerTaskResult {
                summary: "Recovered and completed".to_string(),
                evidence: vec!["Confluence page was read on retry".to_string()],
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
                objective_results: Vec::new(),
            })
        }
    }

    impl AgentRuntimeRunner for MutationThenTransientFailureRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            _task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            on_event(AiWorkerStreamEvent::ToolCompleted {
                tool_call_id: "call-mutation".to_string(),
                tool_name: "bamboo_trigger_build".to_string(),
                success: true,
                error: None,
                risk: "mutation".to_string(),
                arguments_digest: "digest-mutation".to_string(),
                arguments_observed: false,
                display_context: ToolDisplayContext {
                    label: "Trigger Bamboo build".to_string(),
                    category: "external".to_string(),
                    target: None,
                },
                resource_operation_key: None,
            })?;
            on_event(AiWorkerStreamEvent::ToolCompleted {
                tool_call_id: "call-follow-up".to_string(),
                tool_name: "bamboo_get_build".to_string(),
                success: false,
                error: Some("gateway timeout".to_string()),
                risk: "read".to_string(),
                arguments_digest: "digest-follow-up".to_string(),
                arguments_observed: false,
                display_context: ToolDisplayContext {
                    label: "Inspect Bamboo build".to_string(),
                    category: "external".to_string(),
                    target: None,
                },
                resource_operation_key: None,
            })?;
            unreachable!("failed follow-up callback must terminate execution")
        }
    }

    impl AgentRuntimeRunner for CapabilityDriftThenCompleteRunner {
        fn execute(
            &self,
            config: AiWorkerConfig,
            task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let execution = self.executions.fetch_add(1, Ordering::SeqCst);
            self.observed_alternatives
                .lock()
                .expect("alternatives lock")
                .push(
                    task.task_examination
                        .as_ref()
                        .and_then(|examination| examination.runtime_repair.as_ref())
                        .map(|repair| repair.allowed_alternatives.clone())
                        .unwrap_or_default(),
                );
            if execution == 0 {
                on_event(AiWorkerStreamEvent::ToolCompleted {
                    tool_call_id: "call-stale-tool".to_string(),
                    tool_name: "jira_get_issue".to_string(),
                    success: false,
                    error: Some("unknown tool jira_get_issue".to_string()),
                    risk: "read".to_string(),
                    arguments_digest: "digest-stale".to_string(),
                    arguments_observed: false,
                    display_context: ToolDisplayContext {
                        label: "Read Jira issue".to_string(),
                        category: "jira".to_string(),
                        target: None,
                    },
                    resource_operation_key: None,
                })?;
                unreachable!("missing tool callback must terminate the first execution")
            }
            assert_eq!(
                config.mcp_servers[0]
                    .proxy_authority
                    .as_ref()
                    .expect("repair authority")
                    .allowed_tools,
                vec!["jira_read_issue"]
            );
            Ok(AiWorkerTaskResult {
                summary: "Capability plan repaired".to_string(),
                evidence: vec!["jira_read_issue returned the issue".to_string()],
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
                objective_results: Vec::new(),
            })
        }
    }

    impl AgentRuntimeRunner for CheckpointThenCompleteRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let execution = self.executions.fetch_add(1, Ordering::SeqCst);
            self.observed_checkpoints
                .lock()
                .expect("checkpoint observations lock")
                .push(
                    task.task_examination
                        .as_ref()
                        .map(|examination| {
                            examination
                                .objective_checkpoints
                                .iter()
                                .map(|checkpoint| checkpoint.objective_id.clone())
                                .collect()
                        })
                        .unwrap_or_default(),
                );
            if execution == 0 {
                on_event(AiWorkerStreamEvent::OpenCodeSession {
                    session_id: "opencode-checkpoint-session".to_string(),
                    action: "created".to_string(),
                })?;
                on_event(AiWorkerStreamEvent::ObjectiveCheckpoint {
                    objective_id: "objective-1".to_string(),
                    evidence: vec!["Bamboo build 42 succeeded".to_string()],
                })?;
                return Ok(AiWorkerTaskResult {
                    summary: "Rollout still needs operator input".to_string(),
                    evidence: vec!["Bamboo build 42 succeeded".to_string()],
                    details: Vec::new(),
                    next: vec!["Continue rollout".to_string()],
                    completion_status: AiWorkerCompletionStatus::Blocked,
                    blocked_reason: Some("operator input required".to_string()),
                    objective_results: vec![
                        AiWorkerObjectiveResult {
                            objective_id: "objective-1".to_string(),
                            completion_status: AiWorkerCompletionStatus::Completed,
                            evidence: vec!["Bamboo build 42 succeeded".to_string()],
                            blocked_reason: None,
                        },
                        AiWorkerObjectiveResult {
                            objective_id: "objective-2".to_string(),
                            completion_status: AiWorkerCompletionStatus::Blocked,
                            evidence: Vec::new(),
                            blocked_reason: Some("operator input required".to_string()),
                        },
                    ],
                });
            }
            Ok(AiWorkerTaskResult {
                summary: "Rollout completed without rebuilding".to_string(),
                evidence: vec!["Existing build 42 deployed successfully".to_string()],
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
                objective_results: vec![
                    AiWorkerObjectiveResult {
                        objective_id: "objective-1".to_string(),
                        completion_status: AiWorkerCompletionStatus::Completed,
                        evidence: vec!["Bamboo build 42 succeeded".to_string()],
                        blocked_reason: None,
                    },
                    AiWorkerObjectiveResult {
                        objective_id: "objective-2".to_string(),
                        completion_status: AiWorkerCompletionStatus::Completed,
                        evidence: vec!["Rollout healthy".to_string()],
                        blocked_reason: None,
                    },
                ],
            })
        }
    }

    impl AgentRuntimeRunner for MutationCheckpointReplayRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            _task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let execution = self.executions.fetch_add(1, Ordering::SeqCst);
            let digest = argument_digest(&json!({ "plan_key": "QCASH-BUILD" }))?;
            on_event(AiWorkerStreamEvent::OpenCodeSession {
                session_id: "opencode-mutation-checkpoint-session".to_string(),
                action: if execution == 0 { "created" } else { "resumed" }.to_string(),
            })?;
            on_event(AiWorkerStreamEvent::ToolStarted {
                tool_call_id: format!("bamboo-call-{execution}"),
                tool_name: "bamboo_trigger_build".to_string(),
                risk: "mutation".to_string(),
                arguments_digest: digest.clone(),
                display_context: ToolDisplayContext {
                    label: "Triggering QCASH-BUILD".to_string(),
                    category: "bamboo".to_string(),
                    target: Some("QCASH-BUILD".to_string()),
                },
            })?;
            on_event(AiWorkerStreamEvent::ToolCompleted {
                tool_call_id: format!("bamboo-call-{execution}"),
                tool_name: "bamboo_trigger_build".to_string(),
                success: true,
                error: None,
                risk: "mutation".to_string(),
                arguments_digest: argument_digest(&json!({}))?,
                arguments_observed: false,
                display_context: ToolDisplayContext {
                    label: "Triggering QCASH-BUILD".to_string(),
                    category: "bamboo".to_string(),
                    target: Some("QCASH-BUILD".to_string()),
                },
                resource_operation_key: None,
            })?;
            on_event(AiWorkerStreamEvent::ObjectiveCheckpoint {
                objective_id: "objective-1".to_string(),
                evidence: vec!["Bamboo build 842 succeeded".to_string()],
            })?;
            Ok(AiWorkerTaskResult {
                summary: "Deployment still needs operator input".to_string(),
                evidence: vec!["Bamboo build 842 succeeded".to_string()],
                details: Vec::new(),
                next: vec!["Continue deployment".to_string()],
                completion_status: AiWorkerCompletionStatus::Blocked,
                blocked_reason: Some("operator input required".to_string()),
                objective_results: vec![
                    AiWorkerObjectiveResult {
                        objective_id: "objective-1".to_string(),
                        completion_status: AiWorkerCompletionStatus::Completed,
                        evidence: vec!["Bamboo build 842 succeeded".to_string()],
                        blocked_reason: None,
                    },
                    AiWorkerObjectiveResult {
                        objective_id: "objective-2".to_string(),
                        completion_status: AiWorkerCompletionStatus::Blocked,
                        evidence: Vec::new(),
                        blocked_reason: Some("operator input required".to_string()),
                    },
                ],
            })
        }
    }

    impl AgentRuntimeRunner for BlockedRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            _task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            _on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            Ok(AiWorkerTaskResult {
                summary: "approval required".to_string(),
                evidence: Vec::new(),
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Blocked,
                blocked_reason: Some("operator approval required".to_string()),
                objective_results: Vec::new(),
            })
        }
    }

    impl AgentRuntimeRunner for ApprovalThenCompleteRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let execution = self.executions.fetch_add(1, Ordering::SeqCst);
            self.seen_session_ids
                .lock()
                .expect("seen sessions lock")
                .push(task.opencode_session_id.clone());
            on_event(AiWorkerStreamEvent::OpenCodeSession {
                session_id: "opencode-session-resume".to_string(),
                action: if execution == 0 { "created" } else { "resumed" }.to_string(),
            })?;
            if execution == 0 {
                let approval_error =
                    "[approval_required] OpenShift restart requires explicit operator approval";
                on_event(AiWorkerStreamEvent::ToolCompleted {
                    tool_call_id: "call-approval".to_string(),
                    tool_name: "ocp_restart_deployment".to_string(),
                    success: false,
                    error: Some(approval_error.to_string()),
                    risk: "write".to_string(),
                    arguments_digest: "approval-digest".to_string(),
                    arguments_observed: false,
                    display_context: ToolDisplayContext {
                        label: "Restart deployment".to_string(),
                        category: "external".to_string(),
                        target: Some("deployment/clbo".to_string()),
                    },
                    resource_operation_key: None,
                })?;
                return Err(approval_error.to_string());
            }
            Ok(AiWorkerTaskResult {
                summary: "continued after approval".to_string(),
                evidence: vec!["same OpenCode session".to_string()],
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
                objective_results: Vec::new(),
            })
        }
    }

    impl AgentRuntimeRunner for BlockedThenCompleteRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let execution = self.executions.fetch_add(1, Ordering::SeqCst);
            self.seen_session_ids
                .lock()
                .expect("seen sessions lock")
                .push(task.opencode_session_id.clone());
            on_event(AiWorkerStreamEvent::OpenCodeSession {
                session_id: "opencode-session-continued".to_string(),
                action: if execution == 0 { "created" } else { "resumed" }.to_string(),
            })?;
            Ok(AiWorkerTaskResult {
                summary: if execution == 0 {
                    "operator input required"
                } else {
                    "continued successfully"
                }
                .to_string(),
                evidence: Vec::new(),
                details: Vec::new(),
                next: Vec::new(),
                completion_status: if execution == 0 {
                    AiWorkerCompletionStatus::Blocked
                } else {
                    AiWorkerCompletionStatus::Completed
                },
                blocked_reason: (execution == 0).then(|| "operator input required".to_string()),
                objective_results: Vec::new(),
            })
        }
    }

    struct DetachedCallbackRunner {
        callback: Arc<Mutex<Option<AiWorkerEventCallback>>>,
        panic_after_detach: bool,
    }

    impl AgentRuntimeRunner for DetachedCallbackRunner {
        fn execute(
            &self,
            _config: AiWorkerConfig,
            _task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            *self.callback.lock().expect("callback lock") = Some(on_event);
            assert!(!self.panic_after_detach, "expected detached runner panic");
            Ok(AiWorkerTaskResult {
                summary: "complete".to_string(),
                evidence: Vec::new(),
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
                objective_results: Vec::new(),
            })
        }
    }

    impl AgentRuntimeRunner for FakeRunner {
        fn execute(
            &self,
            config: AiWorkerConfig,
            task: AiWorkerTask,
            cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            assert!(!config.opencode_auto_approve);
            assert!(config.fenced_tools_only);
            assert!(config.isolated_opencode_process);
            assert!(!cancellation.load(Ordering::Acquire));
            assert!(task
                .session_key
                .as_deref()
                .is_some_and(|value| value.starts_with("task-session:")));
            assert!(task.opencode_session_id.is_none());
            let authority = config.mcp_servers[0]
                .proxy_authority
                .as_ref()
                .expect("fenced proxy authority");
            assert_eq!(authority.connector_id, "jira");
            assert_eq!(authority.capability, "external_tools:jira");
            on_event(AiWorkerStreamEvent::OpenCodeSession {
                session_id: "opencode-session-fake".to_string(),
                action: "created".to_string(),
            })?;
            on_event(AiWorkerStreamEvent::TextDelta("working".to_string()))?;
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok(AiWorkerTaskResult {
                summary: "complete".to_string(),
                evidence: Vec::new(),
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
                objective_results: Vec::new(),
            })
        }
    }

    impl AgentRuntimeRunner for IsolationRunner {
        fn execute(
            &self,
            config: AiWorkerConfig,
            task: AiWorkerTask,
            _cancellation: Arc<AtomicBool>,
            mut on_event: AiWorkerEventCallback,
        ) -> Result<AiWorkerTaskResult, String> {
            let started_at = self.origin.elapsed().as_millis();
            let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum.fetch_max(current, Ordering::SeqCst);
            let authority = config.mcp_servers[0]
                .proxy_authority
                .as_ref()
                .expect("fenced proxy authority");
            self.seen.lock().expect("seen lock").push((
                task.session_key.expect("session key"),
                authority.session_id.0,
                authority.attempt_id,
                authority.fencing_token,
            ));
            self.barrier.wait();
            on_event(AiWorkerStreamEvent::ToolStarted {
                tool_call_id: format!("tool-{}", authority.session_id.0),
                tool_name: "jira_search".to_string(),
                risk: "low".to_string(),
                arguments_digest: "abc".to_string(),
                display_context: ToolDisplayContext {
                    label: "jira_search".to_string(),
                    category: "external".to_string(),
                    target: Some(authority.session_id.0.to_string()),
                },
            })?;
            on_event(AiWorkerStreamEvent::ToolCompleted {
                tool_call_id: format!("tool-{}", authority.session_id.0),
                tool_name: "jira_search".to_string(),
                success: true,
                error: None,
                risk: "low".to_string(),
                arguments_digest: "abc".to_string(),
                arguments_observed: true,
                display_context: ToolDisplayContext {
                    label: "jira_search".to_string(),
                    category: "external".to_string(),
                    target: Some(authority.session_id.0.to_string()),
                },
                resource_operation_key: None,
            })?;
            on_event(AiWorkerStreamEvent::TextDelta(format!(
                "session:{}",
                authority.session_id.0
            )))?;
            std::thread::sleep(Duration::from_millis(20));
            let completed_at = self.origin.elapsed().as_millis();
            self.intervals.lock().expect("interval lock").push((
                authority.session_id.0,
                started_at,
                completed_at,
            ));
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(AiWorkerTaskResult {
                summary: "complete".to_string(),
                evidence: Vec::new(),
                details: Vec::new(),
                next: Vec::new(),
                completion_status: AiWorkerCompletionStatus::Completed,
                blocked_reason: None,
                objective_results: Vec::new(),
            })
        }
    }

    #[test]
    fn scheduler_agent_executor_fences_connectors_and_journals_runtime_events() {
        let directory = tempdir().expect("temp directory");
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let executions = Arc::new(AtomicUsize::new(0));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: attempts.clone(),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(FakeRunner {
                executions: executions.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let envelope = test_envelope();
        let session = engine
            .submit_envelope_with_grants(
                "real-agent-boundary",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task completes");
        assert_eq!(completed.state, TaskSessionState::Succeeded);
        assert_eq!(
            completed.opencode_session_id.as_deref(),
            Some("opencode-session-fake")
        );
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert_eq!(attempts.lock().expect("attempt lock").len(), 1);
        assert!(engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .iter()
            .any(|event| {
                event.kind == TaskSessionEventKind::Runtime && event.payload["type"] == "text_delta"
            }));
        assert!(engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .iter()
            .any(|event| {
                event.kind == TaskSessionEventKind::Runtime
                    && event.payload["type"] == "agent_result_candidate"
                    && event.payload["authoritative"] == false
                    && event.payload["result"]["completion_status"] == "completed"
            }));
        let trace = engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .into_iter()
            .filter(|event| event.payload["type"] == "execution_trace_stage")
            .collect::<Vec<_>>();
        assert_eq!(trace.len(), 2);
        assert_eq!(trace[0].payload["stage"], "runtime_preparation");
        assert_eq!(trace[1].payload["stage"], "agent_runtime_request");
        for event in &trace {
            assert_eq!(event.payload["schema_version"], 1);
            assert_eq!(
                event.payload["trace_id"],
                format!("task-session:{}", session.id.0)
            );
            assert_eq!(event.payload["worker_id"], 1);
            assert!(event.payload["duration_us"].as_u64().is_some());
            assert!(event.payload["runtime_id"]
                .as_str()
                .is_some_and(|runtime_id| runtime_id.contains("attempt-")));
        }
        assert!(trace[0].payload["opencode_session_id"].is_null());
        assert_eq!(
            trace[1].payload["opencode_session_id"],
            "opencode-session-fake"
        );
    }

    #[test]
    fn five_agent_sessions_execute_simultaneously_with_isolated_runtime_and_tool_authority() {
        let directory = tempdir().expect("temp directory");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let intervals = Arc::new(Mutex::new(Vec::new()));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(IsolationRunner {
                barrier: Arc::new(Barrier::new(5)),
                seen: seen.clone(),
                active: active.clone(),
                maximum: maximum.clone(),
                origin: Instant::now(),
                intervals: intervals.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let sessions = (1..=5)
            .map(|index| {
                let mut envelope = test_envelope();
                let TaskSessionEnvelope::V1(session) = &mut envelope else {
                    unreachable!();
                };
                session.subject_id = Some(format!("card-{index}"));
                session.conversation_id = Some(format!("conversation-{index}"));
                session.execution_run_id = Some(format!("run-{index}"));
                engine
                    .submit_envelope_with_grants(
                        format!("isolated-agent-{index}"),
                        &envelope,
                        vec!["external_tools:jira".to_string()],
                        "test-approval",
                    )
                    .expect("task submitted")
            })
            .collect::<Vec<_>>();

        for session in &sessions {
            engine
                .wait_for_terminal(session.id, Duration::from_secs(5))
                .expect("task completes");
        }
        let seen = seen.lock().expect("seen lock").clone();
        assert_eq!(seen.len(), 5);
        assert_eq!(
            seen.iter()
                .map(|entry| entry.0.as_str())
                .collect::<HashSet<_>>()
                .len(),
            5
        );
        assert_eq!(
            seen.iter()
                .map(|entry| entry.1)
                .collect::<HashSet<_>>()
                .len(),
            5
        );
        assert_eq!(maximum.load(Ordering::SeqCst), 5);
        let intervals = intervals.lock().expect("interval lock").clone();
        assert_eq!(intervals.len(), 5);
        let latest_start = intervals.iter().map(|entry| entry.1).max().unwrap();
        let earliest_finish = intervals.iter().map(|entry| entry.2).min().unwrap();
        assert!(
            latest_start < earliest_finish,
            "all Agent runtime intervals must overlap"
        );
        assert_eq!(
            sessions
                .iter()
                .map(|session| {
                    engine
                        .session(session.id)
                        .expect("session read")
                        .expect("session exists")
                        .worker_id
                        .expect("worker assigned")
                })
                .collect::<HashSet<_>>()
                .len(),
            5
        );
        for session in sessions {
            let events = engine.events_after(session.id, 0).expect("events");
            assert!(events.iter().all(|event| event.session_id == session.id));
            let tools = TaskToolState::from_events(session.id, &events);
            assert_eq!(tools.calls.len(), 1);
            assert!(events.iter().any(|event| {
                event.kind == TaskSessionEventKind::Runtime && event.payload["type"] == "text_delta"
            }));
        }
    }

    #[test]
    fn scheduler_agent_executor_fails_before_runner_without_connector_grant() {
        let directory = tempdir().expect("temp directory");
        let executions = Arc::new(AtomicUsize::new(0));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(FakeRunner {
                executions: executions.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope("missing-grant", &test_envelope())
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task terminates");
        assert_eq!(completed.state, TaskSessionState::Failed);
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(completed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("lacks capability")));
    }

    #[test]
    fn scheduler_agent_executor_blocks_before_runner_when_live_tools_are_unavailable() {
        let directory = tempdir().expect("temp directory");
        let executions = Arc::new(AtomicUsize::new(0));
        let executor = AgentTaskExecutor::new(
            Arc::new(UnavailableConnectorResolver),
            Arc::new(FakeRunner {
                executions: executions.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "unavailable-live-tools",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task blocks");

        assert_eq!(completed.state, TaskSessionState::Blocked);
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(completed
            .error
            .as_deref()
            .is_some_and(|error| { error.contains("MCP capability preflight blocked execution") }));
        assert!(engine
            .events_after(session.id, 0)
            .expect("events")
            .iter()
            .any(|event| {
                event.payload["type"] == "task_examined"
                    && event.payload["live_connector_count"] == 0
            }));
    }

    #[test]
    fn scheduler_agent_executor_preserves_blocked_terminal_outcome() {
        let directory = tempdir().expect("temp directory");
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(BlockedRunner),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "blocked-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task blocks");
        assert_eq!(completed.state, TaskSessionState::Blocked);
        assert_eq!(
            completed.error.as_deref(),
            Some("operator approval required")
        );
        assert!(engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .iter()
            .any(|event| {
                event.payload["type"] == "agent_result_candidate"
                    && event.payload["result"]["completion_status"] == "blocked"
                    && event.payload["result"]["blocked_reason"] == "operator approval required"
            }));
    }

    #[test]
    fn approval_continuation_resumes_the_same_opencode_session_end_to_end() {
        let directory = tempdir().expect("temp directory");
        let runner = Arc::new(ApprovalThenCompleteRunner {
            executions: AtomicUsize::new(0),
            seen_session_ids: Mutex::new(Vec::new()),
        });
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            runner.clone(),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let envelope = test_envelope();
        let session = engine
            .submit_envelope_with_grants(
                "approval-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let paused = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task pauses");
        assert_eq!(paused.state, TaskSessionState::Blocked);
        assert_eq!(
            paused.opencode_session_id.as_deref(),
            Some("opencode-session-resume")
        );

        let resumed = engine
            .resume_after_approval(
                session.id,
                "approval-agent-resumed",
                &envelope,
                vec!["external_tools:jira".to_string()],
            )
            .expect("same task resumed");
        assert_eq!(resumed.id, session.id);
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("resumed task completes");
        assert_eq!(completed.state, TaskSessionState::Succeeded);
        assert_eq!(
            runner
                .seen_session_ids
                .lock()
                .expect("seen sessions lock")
                .as_slice(),
            &[None, Some("opencode-session-resume".to_string())]
        );
        let created_count = engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .iter()
            .filter(|event| event.payload["action"] == json!("created"))
            .count();
        assert_eq!(created_count, 1);
        let governance_events = engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .into_iter()
            .filter(|event| event.payload["type"] == "governance_resolved")
            .collect::<Vec<_>>();
        assert_eq!(governance_events.len(), 2);
        assert_eq!(governance_events[0].payload["reused"], false);
        assert_eq!(governance_events[1].payload["reused"], true);
        assert_eq!(
            governance_events[0].payload["selected_skill_ids"],
            governance_events[1].payload["selected_skill_ids"]
        );
    }

    #[test]
    fn generic_continuation_resumes_the_same_opencode_session_end_to_end() {
        let directory = tempdir().expect("temp directory");
        let runner = Arc::new(BlockedThenCompleteRunner {
            executions: AtomicUsize::new(0),
            seen_session_ids: Mutex::new(Vec::new()),
        });
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            runner.clone(),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let envelope = test_envelope();
        let session = engine
            .submit_envelope_with_grants(
                "blocked-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let blocked = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task blocks");
        assert_eq!(blocked.state, TaskSessionState::Blocked);

        let continued = engine
            .continue_interrupted_session(
                session.id,
                "continued-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
            )
            .expect("task continued");
        assert_eq!(continued.id, session.id);
        assert_eq!(
            continued.opencode_session_id.as_deref(),
            Some("opencode-session-continued")
        );
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("continued task completes");
        assert_eq!(completed.state, TaskSessionState::Succeeded);
        assert_eq!(
            runner
                .seen_session_ids
                .lock()
                .expect("seen sessions lock")
                .as_slice(),
            &[None, Some("opencode-session-continued".to_string())]
        );
    }

    #[test]
    fn blocked_continuation_projects_onto_reused_execution_run() {
        let directory = tempdir().expect("temp directory");
        let executions = crate::infrastructure::execution_store::ExecutionStore::open_at(
            directory.path().join("executions.db"),
        )
        .expect("execution store opens");
        let workspace_id = "workspace-personal";
        let conversation_id = "conversation-1";
        let run_id = "run-1";
        executions
            .save(&ExecutionRun {
                run_id: run_id.to_string(),
                contract: serde_json::json!({
                    "contract_id": format!("contract-{run_id}"),
                    "task_id": "task-1",
                    "workspace_id": workspace_id,
                    "version": 1,
                    "created_at": "2026-07-31T00:00:00Z"
                }),
                status: "running".to_string(),
                current_step_ids: vec!["worker.execute".to_string()],
                step_runs: BTreeMap::from([(
                    "worker.execute".to_string(),
                    StepRun {
                        step_id: "worker.execute".to_string(),
                        status: "ready".to_string(),
                        attempt: 0,
                        started_at: None,
                        completed_at: None,
                        summary: None,
                        lease_owner: None,
                        lease_expires_at: None,
                    },
                )]),
                started_at: "2026-07-31T00:00:00Z".to_string(),
                completed_at: None,
                revision: 0,
            })
            .expect("execution run saved");
        executions
            .append_conversation_message(
                workspace_id,
                conversation_id,
                "Agent card",
                &crate::infrastructure::execution_store::ConversationMessageInput {
                    id: "agent-context".to_string(),
                    role: "user".to_string(),
                    text: "Execute the contract".to_string(),
                },
            )
            .expect("conversation seeded");

        let runner = Arc::new(BlockedThenCompleteRunner {
            executions: AtomicUsize::new(0),
            seen_session_ids: Mutex::new(Vec::new()),
        });
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            runner.clone(),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor_and_projector(
            Arc::new(executor),
            Arc::new(executions.clone()),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");

        let envelope = test_envelope();
        let session = engine
            .submit_envelope_with_grants(
                "blocked-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let blocked = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task blocks");
        assert_eq!(blocked.state, TaskSessionState::Blocked);

        let continued = engine
            .continue_interrupted_session(
                session.id,
                "continued-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
            )
            .expect("task continued");
        assert_eq!(continued.id, session.id);
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("continued task completes");
        assert_eq!(completed.state, TaskSessionState::Succeeded);
        let step = executions
            .get(run_id)
            .expect("run loaded")
            .expect("run exists")
            .step_runs
            .remove("worker.execute")
            .expect("worker.execute step projected");
        assert_eq!(step.status, "completed");
    }

    #[test]
    fn scheduler_agent_executor_terminalizes_failed_tool_with_original_cause() {
        let directory = tempdir().expect("temp directory");
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(ToolFailureRunner),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "failed-tool-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task fails");

        assert_eq!(completed.state, TaskSessionState::Failed);
        assert_eq!(
            completed.error.as_deref(),
            Some(
                "External tool 'jira_search' failed. Cause: Connection refused while reading stdout."
            )
        );
        let events = engine.events_after(session.id, 0).expect("events replayed");
        let terminal = events
            .iter()
            .find(|event| {
                event.kind == TaskSessionEventKind::Lifecycle && event.payload["state"] == "failed"
            })
            .expect("terminal lifecycle event");
        assert_eq!(completed.attempt_id, terminal.attempt_id);
        assert_eq!(completed.fencing_token, terminal.fencing_token);
        assert_eq!(completed.last_event_sequence, terminal.sequence);
        assert_eq!(
            completed.progress,
            Some(TaskProgress {
                phase: "failed".to_string(),
                completed: 1,
                total: Some(1),
            })
        );
        assert!(events
            .windows(2)
            .all(|pair| pair[1].sequence == pair[0].sequence + 1));
        assert!(engine
            .task_session_result(session.id)
            .expect("authoritative result query")
            .is_none());
        assert!(events.iter().any(|event| {
            event.kind == TaskSessionEventKind::Tool
                && event.payload["type"] == "tool_completed"
                && event.payload["success"] == false
                && event.payload["error"] == "Connection refused while reading stdout."
        }));
        let recovery = events
            .iter()
            .filter(|event| event.payload["type"] == "runtime_recovery_decision")
            .collect::<Vec<_>>();
        assert_eq!(recovery.len(), 2);
        assert_eq!(recovery[0].payload["action"], "retry_current_assignment");
        assert_eq!(recovery[1].payload["action"], "stop_failed");
    }

    #[test]
    fn scheduler_agent_executor_rejects_mismatched_tool_completion_identity() {
        let directory = tempdir().expect("temp directory");
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(MismatchedToolCompletionRunner),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "mismatched-tool-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task fails");

        assert_eq!(completed.state, TaskSessionState::Failed);
        assert!(completed
            .error
            .as_deref()
            .is_some_and(|error| { error.contains("did not match its started tool identity") }));
    }

    #[test]
    fn transient_read_failure_retries_once_in_the_same_opencode_session() {
        let directory = tempdir().expect("temp directory");
        let executions = Arc::new(AtomicUsize::new(0));
        let resumed_session_ids = Arc::new(Mutex::new(Vec::new()));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(TransientThenCompleteRunner {
                executions: executions.clone(),
                resumed_session_ids: resumed_session_ids.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "transient-recovery-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task recovers");

        assert_eq!(completed.state, TaskSessionState::Succeeded);
        assert_eq!(executions.load(Ordering::SeqCst), 2);
        assert_eq!(
            resumed_session_ids
                .lock()
                .expect("sessions lock")
                .as_slice(),
            &[None, Some("opencode-recovery-session".to_string())]
        );
        let events = engine.events_after(session.id, 0).expect("events replayed");
        let recovery = events
            .iter()
            .find(|event| event.payload["type"] == "runtime_recovery_decision")
            .expect("recovery decision journaled");
        assert_eq!(recovery.payload["failure_class"], "transient_transport");
        assert_eq!(recovery.payload["action"], "retry_current_assignment");
        assert!(events.iter().any(|event| {
            event.kind == TaskSessionEventKind::Progress
                && event.progress.as_ref().is_some_and(|progress| {
                    progress.phase == "recovering_runtime" && progress.completed == 1
                })
        }));
    }

    #[test]
    fn transient_failure_after_mutation_blocks_without_replay() {
        let directory = tempdir().expect("temp directory");
        let executions = Arc::new(AtomicUsize::new(0));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(MutationThenTransientFailureRunner {
                executions: executions.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "uncertain-mutation-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task blocks");

        assert_eq!(completed.state, TaskSessionState::Blocked);
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        let events = engine.events_after(session.id, 0).expect("events replayed");
        let recovery = events
            .iter()
            .find(|event| event.payload["type"] == "runtime_recovery_decision")
            .expect("recovery decision journaled");
        assert_eq!(recovery.payload["action"], "review_uncertain_outcome");
        assert_eq!(recovery.payload["successful_mutation_observed"], true);
    }

    #[test]
    fn missing_read_tool_repairs_from_live_inventory_without_expanding_authority() {
        let directory = tempdir().expect("temp directory");
        let executions = Arc::new(AtomicUsize::new(0));
        let observed_alternatives = Arc::new(Mutex::new(Vec::new()));
        let executor = AgentTaskExecutor::new(
            Arc::new(CapabilityRepairResolver),
            Arc::new(CapabilityDriftThenCompleteRunner {
                executions: executions.clone(),
                observed_alternatives: observed_alternatives.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "capability-repair-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task repairs capability plan");

        assert_eq!(completed.state, TaskSessionState::Succeeded);
        assert_eq!(executions.load(Ordering::SeqCst), 2);
        assert_eq!(
            observed_alternatives
                .lock()
                .expect("alternatives lock")
                .as_slice(),
            &[Vec::<String>::new(), vec!["jira_read_issue".to_string()]]
        );
        let events = engine.events_after(session.id, 0).expect("events replayed");
        let repair = events
            .iter()
            .find(|event| event.payload["type"] == "capability_repair_decision")
            .expect("capability repair journaled");
        assert_eq!(repair.payload["repairable"], true);
        assert_eq!(repair.payload["connector_id"], "jira");
        assert_eq!(repair.payload["allowed_alternatives"][0], "jira_read_issue");
        assert!(events.iter().any(|event| {
            event.kind == TaskSessionEventKind::Progress
                && event.progress.as_ref().is_some_and(|progress| {
                    progress.phase == "repairing_capability_plan" && progress.completed == 1
                })
        }));
    }

    #[test]
    fn blocked_continuation_retains_completed_objective_checkpoint() {
        let directory = tempdir().expect("temp directory");
        let contract = json!({
            "contract": "checkpoint-test",
            "semantic_plan": {
                "status": "fallback",
                "planner_version": "agent-semantic-plan-v1",
                "objectives": [
                    { "id": "objective-1", "summary": "Trigger Bamboo build", "success_evidence": "successful build" },
                    { "id": "objective-2", "summary": "Deploy rollout", "success_evidence": "healthy rollout" }
                ]
            }
        });
        let mut envelope = test_envelope();
        let TaskSessionEnvelope::V1(session_envelope) = &mut envelope else {
            unreachable!();
        };
        session_envelope.context_digest = execution_contract_digest(&contract).expect("digest");
        let executions = Arc::new(AtomicUsize::new(0));
        let observed_checkpoints = Arc::new(Mutex::new(Vec::new()));
        let executor = AgentTaskExecutor::new(
            Arc::new(CheckpointResolver { contract }),
            Arc::new(CheckpointThenCompleteRunner {
                executions: executions.clone(),
                observed_checkpoints: observed_checkpoints.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "checkpoint-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let blocked = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task blocks with partial completion");
        assert_eq!(blocked.state, TaskSessionState::Blocked);

        engine
            .continue_interrupted_session(
                session.id,
                "checkpoint-agent-continued",
                &envelope,
                vec!["external_tools:jira".to_string()],
            )
            .expect("task continued");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("continued task completes");

        assert_eq!(completed.state, TaskSessionState::Succeeded);
        assert_eq!(executions.load(Ordering::SeqCst), 2);
        assert_eq!(
            observed_checkpoints
                .lock()
                .expect("checkpoint observations lock")
                .as_slice(),
            &[Vec::<String>::new(), vec!["objective-1".to_string()]]
        );
        let events = engine.events_after(session.id, 0).expect("events replayed");
        assert!(events.iter().any(|event| {
            event.payload["type"] == "objective_checkpointed"
                && event.payload["objective_id"] == "objective-1"
                && event.payload["new_checkpoint"] == true
        }));
        assert!(events.iter().any(|event| {
            event.payload["type"] == "task_examined"
                && event.payload["objective_checkpoint_count"] == 1
        }));
    }

    #[test]
    fn mutation_objective_rejects_checkpoint_without_successful_mutation_event() {
        let directory = tempdir().expect("temp directory");
        let contract = json!({
            "contract": "mutation-checkpoint-test",
            "semantic_plan": {
                "status": "fallback",
                "planner_version": "agent-semantic-plan-v1",
                "objectives": [
                    {
                        "id": "objective-1",
                        "summary": "Trigger Bamboo build",
                        "success_evidence": "successful build",
                        "mutation_expected": true
                    },
                    { "id": "objective-2", "summary": "Deploy rollout", "success_evidence": "healthy rollout" }
                ]
            }
        });
        let mut envelope = test_envelope();
        let TaskSessionEnvelope::V1(session_envelope) = &mut envelope else {
            unreachable!();
        };
        session_envelope.context_digest = execution_contract_digest(&contract).expect("digest");
        let executor = AgentTaskExecutor::new(
            Arc::new(CheckpointResolver { contract }),
            Arc::new(CheckpointThenCompleteRunner {
                executions: Arc::new(AtomicUsize::new(0)),
                observed_checkpoints: Arc::new(Mutex::new(Vec::new())),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "mutation-checkpoint-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let failed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("invalid mutation checkpoint fails");

        assert_eq!(failed.state, TaskSessionState::Failed);
        assert!(failed.error.as_deref().is_some_and(
            |error| error.contains("cannot checkpoint before a successful mutation tool event")
        ));
        let events = engine.events_after(session.id, 0).expect("events replayed");
        assert!(!events
            .iter()
            .any(|event| event.payload["type"] == "objective_checkpointed"));
    }

    #[test]
    fn continuation_rejects_identical_mutation_from_checkpoint_receipt() {
        let directory = tempdir().expect("temp directory");
        let contract = json!({
            "contract": "mutation-replay-test",
            "semantic_plan": {
                "status": "fallback",
                "planner_version": "agent-semantic-plan-v1",
                "objectives": [
                    {
                        "id": "objective-1",
                        "summary": "Trigger Bamboo build",
                        "success_evidence": "successful build",
                        "mutation_expected": true
                    },
                    { "id": "objective-2", "summary": "Deploy rollout", "success_evidence": "healthy rollout", "mutation_expected": true }
                ]
            }
        });
        let mut envelope = test_envelope();
        let TaskSessionEnvelope::V1(session_envelope) = &mut envelope else {
            unreachable!();
        };
        session_envelope.context_digest = execution_contract_digest(&contract).expect("digest");
        let executions = Arc::new(AtomicUsize::new(0));
        let executor = AgentTaskExecutor::new(
            Arc::new(CheckpointResolver { contract }),
            Arc::new(MutationCheckpointReplayRunner {
                executions: executions.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "mutation-replay-agent",
                &envelope,
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let blocked = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("first attempt blocks after checkpoint");
        assert_eq!(blocked.state, TaskSessionState::Blocked);

        let first_events = engine.events_after(session.id, 0).expect("events replayed");
        let checkpoint = first_events
            .iter()
            .find(|event| event.payload["type"] == "objective_checkpointed")
            .expect("checkpoint event retained");
        assert_eq!(checkpoint.payload["tool_receipt_count"], 1);
        assert_eq!(
            checkpoint.payload["tool_receipts"][0]["arguments_digest"],
            argument_digest(&json!({ "plan_key": "QCASH-BUILD" })).expect("digest")
        );

        engine
            .continue_interrupted_session(
                session.id,
                "mutation-replay-agent-continued",
                &envelope,
                vec!["external_tools:jira".to_string()],
            )
            .expect("task continued");
        let failed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("replayed mutation fails");

        assert_eq!(failed.state, TaskSessionState::Failed);
        assert!(failed.error.as_deref().is_some_and(|error| {
            error.contains("replays completed objective 'objective-1' with identical arguments")
        }));
        assert_eq!(executions.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn scheduler_agent_executor_rejects_resolver_profile_mismatch() {
        let directory = tempdir().expect("temp directory");
        let executions = Arc::new(AtomicUsize::new(0));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "stale-profile".to_string(),
            }),
            Arc::new(FakeRunner {
                executions: executions.clone(),
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "mismatched-agent",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task fails");
        assert_eq!(completed.state, TaskSessionState::Failed);
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        assert!(completed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("profile")));
    }

    #[test]
    fn scheduler_agent_executor_closes_detached_callbacks_before_completion() {
        let directory = tempdir().expect("temp directory");
        let callback = Arc::new(Mutex::new(None));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(DetachedCallbackRunner {
                callback: callback.clone(),
                panic_after_detach: false,
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "detached-callback",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task completes");
        let event_count = engine
            .events_after(session.id, 0)
            .expect("events replayed")
            .len();
        let mut detached = callback
            .lock()
            .expect("callback lock")
            .take()
            .expect("detached callback");
        assert!(detached(AiWorkerStreamEvent::TextDelta("late".to_string())).is_err());
        assert_eq!(
            engine
                .events_after(session.id, 0)
                .expect("events replayed")
                .len(),
            event_count
        );
    }

    #[test]
    fn resolved_opencode_model_must_match_the_envelope() {
        let envelope = match test_envelope() {
            TaskSessionEnvelope::V1(envelope) => envelope,
            TaskSessionEnvelope::V2(_) => panic!("expected V1 Agent envelope"),
        };
        let mut config = test_config();
        config.opencode_model = "other/model".to_string();
        let resolved = ResolvedAgentTask {
            runtime_profile_id: envelope.runtime_profile_id.clone(),
            config,
            task: AiWorkerTask {
                execution_contract: Some(json!({ "contract": "test" })),
                task_examination: None,
                session_key: None,
                opencode_session_id: None,
            },
            governance: test_governance(1),
            connector_capabilities: Vec::new(),
        };
        assert!(validate_resolved_task(&envelope, &resolved)
            .expect_err("model mismatch")
            .contains("model"));
    }

    #[test]
    fn runtime_governance_must_match_the_persisted_resolution() {
        let envelope = match test_envelope() {
            TaskSessionEnvelope::V1(envelope) => envelope,
            TaskSessionEnvelope::V2(_) => panic!("expected V1 Agent envelope"),
        };
        let mut config = test_config();
        config.agent_skills = "unpersisted skill instructions".to_string();
        let resolved = ResolvedAgentTask {
            runtime_profile_id: envelope.runtime_profile_id.clone(),
            config,
            task: AiWorkerTask {
                execution_contract: Some(json!({ "contract": "test" })),
                task_examination: None,
                session_key: None,
                opencode_session_id: None,
            },
            governance: test_governance(1),
            connector_capabilities: Vec::new(),
        };
        assert!(validate_resolved_task(&envelope, &resolved)
            .expect_err("governance mismatch")
            .contains("authoritative governance snapshot"));
    }

    #[test]
    fn scheduler_agent_executor_closes_detached_callback_when_runner_panics() {
        let directory = tempdir().expect("temp directory");
        let callback = Arc::new(Mutex::new(None));
        let executor = AgentTaskExecutor::new(
            Arc::new(FakeResolver {
                attempts: Arc::new(Mutex::new(Vec::new())),
                runtime_profile_id: "profile-1".to_string(),
            }),
            Arc::new(DetachedCallbackRunner {
                callback: callback.clone(),
                panic_after_detach: true,
            }),
        );
        let engine = ExecutionEngine::open_persistent_at_with_executor(
            Arc::new(executor),
            directory.path().join("scheduler.db"),
        )
        .expect("engine starts");
        let session = engine
            .submit_envelope_with_grants(
                "panic-callback",
                &test_envelope(),
                vec!["external_tools:jira".to_string()],
                "test-approval",
            )
            .expect("task submitted");
        let completed = engine
            .wait_for_terminal(session.id, Duration::from_secs(5))
            .expect("task fails");
        assert_eq!(completed.state, TaskSessionState::Failed);
        let mut detached = callback
            .lock()
            .expect("callback lock")
            .take()
            .expect("detached callback");
        assert!(detached(AiWorkerStreamEvent::TextDelta("late".to_string())).is_err());
    }

    fn test_config() -> AiWorkerConfig {
        AiWorkerConfig {
            workspace_id: "workspace-personal".to_string(),
            runtime: "opencode".to_string(),
            provider_name: "OpenCode".to_string(),
            provider_id: "openai".to_string(),
            base_url: String::new(),
            api_style: String::new(),
            api_key: String::new(),
            model: "gpt-5".to_string(),
            opencode_command: "opencode".to_string(),
            opencode_model: "openai/gpt-5".to_string(),
            opencode_workdir: Some("/tmp".to_string()),
            opencode_auto_approve: true,
            agent_rules: String::new(),
            agent_skills: String::new(),
            governance_schema_version: 0,
            skill_catalog: Vec::new(),
            temperature: 0.0,
            restrict_tools: false,
            fenced_tools_only: false,
            isolated_opencode_process: false,
            task_tool_authority: None,
            mcp_servers: vec![AiWorkerMcpServer {
                name: "jira".to_string(),
                secret_id: "jira".to_string(),
                command: vec!["jira-mcp".to_string()],
                environment: HashMap::from([("JIRA_URL".to_string(), "test".to_string())]),
                proxy_authority: None,
            }],
        }
    }

    fn repository_fact(local_path: Option<String>) -> RepositoryRuleFact {
        RepositoryRuleFact {
            id: "qcash-deployment".to_string(),
            remote_url: "https://bitbucket.example/projects/OPS/repos/qcash-deployment".to_string(),
            local_path,
            backend_path: Some("service".to_string()),
            frontend_path: Some("frontend".to_string()),
            source: "global.agent_rules".to_string(),
            source_line: 2,
        }
    }

    fn deployment_target_fact(
        label: &str,
        target: &str,
        branch: &str,
        namespace: &str,
    ) -> DeploymentTargetRuleFact {
        DeploymentTargetRuleFact {
            label: label.to_string(),
            target: target.to_string(),
            branch: branch.to_string(),
            namespace: namespace.to_string(),
            source: "global.agent_rules".to_string(),
            source_line: 10,
        }
    }

    fn connector_rule(base_url: &str, operations: &[&str]) -> ConnectorRuleFact {
        ConnectorRuleFact {
            id: "corporate-confluence".to_string(),
            connector_type: "confluence".to_string(),
            base_url: base_url.to_string(),
            required_operations: operations
                .iter()
                .map(|operation| operation.to_string())
                .collect(),
            source: "global.agent_rules".to_string(),
            source_line: 20,
        }
    }

    fn confluence_server(base_url: &str) -> AiWorkerMcpServer {
        AiWorkerMcpServer {
            name: "corporate-confluence".to_string(),
            secret_id: "corporate-confluence".to_string(),
            command: vec!["confluence-mcp".to_string()],
            environment: HashMap::from([("CONFLUENCE_URL".to_string(), base_url.to_string())]),
            proxy_authority: None,
        }
    }

    fn confluence_capabilities(
        tools: &[&str],
    ) -> crate::domain::task_examination::ConnectorCapabilitySnapshot {
        crate::domain::task_examination::ConnectorCapabilitySnapshot {
            connector_id: "corporate-confluence".to_string(),
            status: ConnectorDiscoveryStatus::Available,
            tools: tools
                .iter()
                .map(
                    |name| crate::domain::task_examination::DiscoveredToolCapability {
                        name: name.to_string(),
                        risk: "read".to_string(),
                        argument_names: Vec::new(),
                    },
                )
                .collect(),
            error: None,
            warnings: Vec::new(),
        }
    }

    fn verification_binding(tools: &[&str]) -> VerificationPolicyBindingRecord {
        VerificationPolicyBindingRecord {
            schema_version: 1,
            policy_id: "confluence-source-read".to_string(),
            connector_id: "corporate-confluence".to_string(),
            status: "ready".to_string(),
            matched_labels: vec!["NQLA_PRESTAGE".to_string()],
            required_operations: tools.iter().map(|tool| tool.to_string()).collect(),
            verified_tools: tools
                .iter()
                .map(|tool| format!("confluence_{tool}"))
                .collect(),
            source: "global.agent_rules".to_string(),
            source_line: 30,
            reason: "bound".to_string(),
        }
    }

    fn read_receipt(tool_name: &str) -> AgentTaskObjectiveToolReceipt {
        AgentTaskObjectiveToolReceipt {
            tool_call_id: format!("call-{tool_name}"),
            tool_name: tool_name.to_string(),
            risk: "read".to_string(),
            arguments_digest: "digest".to_string(),
            resource_operation_key: None,
        }
    }

    fn git_evidence_binding(states: &[&str]) -> EvidenceVerifierBindingRecord {
        EvidenceVerifierBindingRecord {
            schema_version: 1,
            verifier_id: "git-release-state".to_string(),
            provider: "git".to_string(),
            status: "ready".to_string(),
            matched_labels: Vec::new(),
            required_states: states.iter().map(|state| state.to_string()).collect(),
            resource_kind: None,
            resource_name: None,
            namespace: None,
            poll_interval_seconds: None,
            timeout_seconds: None,
            source: "global.agent_rules".to_string(),
            source_line: 40,
            reason: "bound".to_string(),
        }
    }

    fn run_test_git(repository: &Path, arguments: &[&str]) -> String {
        let output = std::process::Command::new(
            crate::infrastructure::git::git_executable().expect("git executable"),
        )
        .args(arguments)
        .current_dir(repository)
        .output()
        .expect("git command runs");
        assert!(output.status.success(), "git {:?} failed", arguments);
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn commit_test_file(repository: &Path, name: &str, content: &str) -> String {
        std::fs::write(repository.join(name), content).expect("test file");
        run_test_git(repository, &["add", name]);
        run_test_git(repository, &["commit", "--quiet", "-m", name]);
        run_test_git(repository, &["rev-parse", "HEAD"])
    }

    fn deployment_evidence(
        available: bool,
    ) -> crate::infrastructure::ocp::DeploymentAvailabilityEvidence {
        crate::infrastructure::ocp::DeploymentAvailabilityEvidence {
            desired_replicas: 2,
            updated_replicas: 2,
            ready_replicas: if available { 2 } else { 1 },
            available_replicas: if available { 2 } else { 1 },
            generation_observed: true,
            available,
        }
    }

    #[test]
    fn deployment_availability_polling_retries_progress_and_stops_when_satisfied() {
        let elapsed = Arc::new(Mutex::new(Duration::ZERO));
        let waits = Arc::new(Mutex::new(Vec::new()));
        let budgets = Arc::new(Mutex::new(Vec::new()));
        let attempts = AtomicUsize::new(0);
        let result = poll_deployment_availability(
            Some(Duration::from_secs(5)),
            Some(Duration::from_secs(15)),
            {
                let budgets = budgets.clone();
                move |budget| {
                    budgets.lock().expect("budgets lock").push(budget);
                    Ok(deployment_evidence(
                        attempts.fetch_add(1, Ordering::SeqCst) > 0,
                    ))
                }
            },
            || Ok::<(), &'static str>(()),
            {
                let elapsed = elapsed.clone();
                let waits = waits.clone();
                move |duration| {
                    *elapsed.lock().expect("elapsed lock") += duration;
                    waits.lock().expect("waits lock").push(duration);
                    Ok::<(), &'static str>(())
                }
            },
            {
                let elapsed = elapsed.clone();
                move || *elapsed.lock().expect("elapsed lock")
            },
        )
        .expect("polling succeeds");
        assert_eq!(result.status, DeploymentAvailabilityPollStatus::Satisfied);
        assert_eq!(result.attempts, 2);
        assert_eq!(
            waits.lock().expect("waits lock").as_slice(),
            &[Duration::from_secs(5)]
        );
        assert_eq!(
            budgets.lock().expect("budgets lock").as_slice(),
            &[Duration::from_secs(15), Duration::from_secs(10)]
        );
    }

    #[test]
    fn deployment_availability_polling_times_out_with_last_observed_state() {
        let elapsed = Arc::new(Mutex::new(Duration::ZERO));
        let result = poll_deployment_availability(
            Some(Duration::from_secs(4)),
            Some(Duration::from_secs(10)),
            |_| Ok(deployment_evidence(false)),
            || Ok::<(), &'static str>(()),
            {
                let elapsed = elapsed.clone();
                move |duration| {
                    *elapsed.lock().expect("elapsed lock") += duration;
                    Ok::<(), &'static str>(())
                }
            },
            {
                let elapsed = elapsed.clone();
                move || *elapsed.lock().expect("elapsed lock")
            },
        )
        .expect("polling completes");
        assert_eq!(result.status, DeploymentAvailabilityPollStatus::TimedOut);
        assert_eq!(result.attempts, 3);
        assert_eq!(result.evidence, Some(deployment_evidence(false)));
    }

    #[test]
    fn deployment_availability_polling_propagates_cancellation_during_wait() {
        let result = poll_deployment_availability(
            Some(Duration::from_secs(5)),
            Some(Duration::from_secs(15)),
            |_| Ok(deployment_evidence(false)),
            || Ok::<(), &'static str>(()),
            |_| Err("cancelled"),
            || Duration::ZERO,
        );
        assert_eq!(result, Err("cancelled"));
    }

    #[test]
    fn deployment_availability_polling_does_not_retry_unavailable_reads() {
        let attempts = AtomicUsize::new(0);
        let result = poll_deployment_availability(
            Some(Duration::from_secs(5)),
            Some(Duration::from_secs(15)),
            |_| {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err("redacted connector failure".to_string())
            },
            || Ok::<(), &'static str>(()),
            |_| Ok::<(), &'static str>(()),
            || Duration::ZERO,
        )
        .expect("read failure becomes evidence status");
        assert_eq!(result.status, DeploymentAvailabilityPollStatus::Unavailable);
        assert_eq!(result.attempts, 1);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn git_evidence_verifier_observes_clean_new_commit_and_upstream_state() {
        let directory = tempdir().expect("workspace");
        let repository = directory.path().join("repository");
        initialize_repository(&repository);
        run_test_git(
            &repository,
            &["config", "user.email", "spacesly@example.invalid"],
        );
        run_test_git(&repository, &["config", "user.name", "Spacesly Test"]);
        let baseline = commit_test_file(&repository, "baseline.txt", "baseline");
        let contract = json!({"repository": {"head_commit": baseline}});
        let bindings = vec![git_evidence_binding(&["clean_worktree", "new_commit"])];

        let (_, initial_failures) = verify_git_evidence_states(&bindings, &repository, &contract);
        assert_eq!(initial_failures, vec!["new_commit"]);

        commit_test_file(&repository, "change.txt", "change");
        let (evidence, failures) = verify_git_evidence_states(&bindings, &repository, &contract);
        assert!(failures.is_empty());
        assert!(evidence.iter().all(|item| item["status"] == "satisfied"));

        std::fs::write(repository.join("change.txt"), "dirty").expect("dirty file");
        let (_, failures) = verify_git_evidence_states(&bindings, &repository, &contract);
        assert_eq!(failures, vec!["clean_worktree"]);
    }

    #[test]
    fn git_evidence_verifier_requires_head_to_be_contained_by_upstream() {
        let directory = tempdir().expect("workspace");
        let remote = directory.path().join("remote.git");
        std::fs::create_dir_all(&remote).expect("remote directory");
        run_test_git(&remote, &["init", "--quiet", "--bare"]);
        let repository = directory.path().join("repository");
        initialize_repository(&repository);
        run_test_git(
            &repository,
            &["config", "user.email", "spacesly@example.invalid"],
        );
        run_test_git(&repository, &["config", "user.name", "Spacesly Test"]);
        commit_test_file(&repository, "baseline.txt", "baseline");
        run_test_git(
            &repository,
            &[
                "remote",
                "add",
                "origin",
                remote.to_str().expect("remote path"),
            ],
        );
        run_test_git(&repository, &["push", "--quiet", "-u", "origin", "HEAD"]);
        let bindings = vec![git_evidence_binding(&["pushed_upstream"])];
        let contract = json!({});
        assert!(
            verify_git_evidence_states(&bindings, &repository, &contract)
                .1
                .is_empty()
        );

        commit_test_file(&repository, "local.txt", "local only");
        assert_eq!(
            verify_git_evidence_states(&bindings, &repository, &contract).1,
            vec!["pushed_upstream"]
        );
    }

    #[test]
    fn evidence_verifier_binding_is_label_scoped_and_fails_closed_for_unsupported_provider() {
        let git_rule = crate::domain::governance::EvidenceVerifierRuleFact {
            id: "git-release-state".to_string(),
            provider: "git".to_string(),
            applies_to_labels: vec!["release".to_string()],
            required_states: vec!["clean_worktree".to_string(), "new_commit".to_string()],
            source: "global.agent_rules".to_string(),
            source_line: 40,
            ..Default::default()
        };
        let bamboo_rule = crate::domain::governance::EvidenceVerifierRuleFact {
            id: "bamboo-build-state".to_string(),
            provider: "bamboo".to_string(),
            applies_to_labels: Vec::new(),
            required_states: vec!["successful_build".to_string()],
            source: "global.agent_rules".to_string(),
            source_line: 50,
            ..Default::default()
        };
        let facts = RuleFactsRecord {
            connectors: vec![ConnectorRuleFact {
                id: "corporate-bamboo".to_string(),
                connector_type: "bamboo".to_string(),
                ..Default::default()
            }],
            evidence_verifiers: vec![git_rule, bamboo_rule],
            ..Default::default()
        };
        let directory = tempdir().expect("repository");
        let (records, blockers) = resolve_evidence_verifier_bindings(
            &json!({
                "ticket": {"labels": ["RELEASE"]},
                "repository": {"head_commit": "abc"}
            }),
            &["git".to_string()],
            &[],
            &facts,
            Some(directory.path()),
            None,
            false,
        );
        assert!(blockers.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "ready");

        let (records, blockers) = resolve_evidence_verifier_bindings(
            &json!({"ticket": {"labels": ["RELEASE"]}, "repository": {}}),
            &["git".to_string()],
            &[],
            &facts,
            Some(directory.path()),
            None,
            false,
        );
        assert_eq!(records[0].status, "missing_repository");
        assert_eq!(blockers.len(), 1);

        let (records, blockers) = resolve_evidence_verifier_bindings(
            &json!({}),
            &["external_tools:corporate-bamboo".to_string()],
            &["corporate-bamboo".to_string()],
            &facts,
            None,
            None,
            false,
        );
        assert_eq!(records[0].status, "unsupported_provider");
        assert_eq!(blockers.len(), 1);

        let mut invalid_git_polling = facts.clone();
        invalid_git_polling.evidence_verifiers[0].poll_interval_seconds = Some(5);
        invalid_git_polling.evidence_verifiers[0].timeout_seconds = Some(30);
        let (records, blockers) = resolve_evidence_verifier_bindings(
            &json!({
                "ticket": {"labels": ["RELEASE"]},
                "repository": {"head_commit": "abc"}
            }),
            &["git".to_string()],
            &[],
            &invalid_git_polling,
            Some(directory.path()),
            None,
            false,
        );
        assert_eq!(records[0].status, "invalid_rule");
        assert_eq!(records[0].poll_interval_seconds, None);
        assert_eq!(records[0].timeout_seconds, None);
        assert_eq!(blockers.len(), 1);
    }

    #[test]
    fn kubernetes_evidence_verifier_binds_exact_workload_and_resolved_namespace() {
        let facts = RuleFactsRecord {
            connectors: vec![ConnectorRuleFact {
                id: "ocp-dev".to_string(),
                connector_type: "ocp".to_string(),
                ..Default::default()
            }],
            evidence_verifiers: vec![crate::domain::governance::EvidenceVerifierRuleFact {
                id: "deployment-health".to_string(),
                provider: "kubernetes".to_string(),
                applies_to_labels: Vec::new(),
                required_states: vec!["deployment_available".to_string()],
                poll_interval_seconds: Some(5),
                timeout_seconds: Some(120),
                source: "global.agent_rules".to_string(),
                source_line: 60,
                ..Default::default()
            }],
            ..Default::default()
        };
        let target =
            deployment_target_fact("PRESTAGE", "prerelease", "prerelease", "qcash-prerelease");
        let (records, blockers) = resolve_evidence_verifier_bindings(
            &json!({"deployment": {"workload": "payroll-api"}}),
            &["external_tools:ocp-dev".to_string()],
            &["ocp-dev".to_string()],
            &facts,
            None,
            Some(&target),
            true,
        );
        assert!(blockers.is_empty());
        assert_eq!(records[0].status, "ready");
        assert_eq!(records[0].resource_kind.as_deref(), Some("deployment"));
        assert_eq!(records[0].resource_name.as_deref(), Some("payroll-api"));
        assert_eq!(records[0].namespace.as_deref(), Some("qcash-prerelease"));
        assert_eq!(records[0].poll_interval_seconds, Some(5));
        assert_eq!(records[0].timeout_seconds, Some(120));

        let (missing, blockers) = resolve_evidence_verifier_bindings(
            &json!({"deployment": {}}),
            &["external_tools:ocp-dev".to_string()],
            &["ocp-dev".to_string()],
            &facts,
            None,
            Some(&target),
            true,
        );
        assert_eq!(missing[0].status, "missing_resource");
        assert_eq!(blockers.len(), 1);

        let (unsupported, blockers) = resolve_evidence_verifier_bindings(
            &json!({"deployment": {"workload": "payroll-api"}}),
            &["external_tools:ocp-dev".to_string()],
            &["ocp-dev".to_string()],
            &facts,
            None,
            Some(&target),
            false,
        );
        assert_eq!(unsupported[0].status, "unsupported_provider");
        assert_eq!(blockers.len(), 1);

        let mut invalid_polling = facts.clone();
        invalid_polling.evidence_verifiers[0].timeout_seconds = None;
        let (invalid, blockers) = resolve_evidence_verifier_bindings(
            &json!({"deployment": {"workload": "payroll-api"}}),
            &["external_tools:ocp-dev".to_string()],
            &["ocp-dev".to_string()],
            &invalid_polling,
            None,
            Some(&target),
            true,
        );
        assert_eq!(invalid[0].status, "invalid_rule");
        assert_eq!(blockers.len(), 1);
    }

    #[test]
    fn verification_policy_binds_only_for_matching_task_label() {
        let facts = RuleFactsRecord {
            connectors: vec![connector_rule("https://confluence.example", &["get_page"])],
            verification_policies: vec![crate::domain::governance::VerificationRuleFact {
                id: "confluence-source-read".to_string(),
                connector_id: "corporate-confluence".to_string(),
                applies_to_labels: vec!["NQLA_PRESTAGE".to_string()],
                required_operations: vec!["get_page".to_string()],
                source: "global.agent_rules".to_string(),
                source_line: 30,
            }],
            ..Default::default()
        };
        let capabilities = vec![confluence_capabilities(&["confluence_get_page"])];
        let (matched, blockers) = resolve_verification_policy_bindings(
            &json!({"ticket": {"labels": ["nqla_prestage"]}}),
            &["corporate-confluence".to_string()],
            &facts,
            &capabilities,
        );
        assert!(blockers.is_empty());
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].verified_tools, vec!["confluence_get_page"]);

        let (unmatched, blockers) = resolve_verification_policy_bindings(
            &json!({"ticket": {"labels": ["NQLA_DEV"]}}),
            &["corporate-confluence".to_string()],
            &facts,
            &capabilities,
        );
        assert!(blockers.is_empty());
        assert!(unmatched.is_empty());
    }

    #[test]
    fn completion_verification_requires_every_bound_successful_tool() {
        let policies = vec![verification_binding(&["search", "get_page"])];
        let missing =
            missing_verification_tools(&policies, &[], &[read_receipt("confluence_search")]);
        assert_eq!(missing, vec!["confluence-source-read:confluence_get_page"]);
    }

    #[test]
    fn completion_verification_accepts_checkpoint_and_current_attempt_receipts() {
        let policies = vec![verification_binding(&["search", "get_page"])];
        let checkpoints = vec![crate::domain::task_session::AgentTaskObjectiveCheckpoint {
            objective_id: "read-source".to_string(),
            evidence: vec!["search completed".to_string()],
            tool_receipts: vec![read_receipt("confluence_search")],
            source_attempt_id: 1,
            recorded_at: 1,
        }];
        assert!(missing_verification_tools(
            &policies,
            &checkpoints,
            &[read_receipt("confluence_get_page")],
        )
        .is_empty());
    }

    #[test]
    fn rule_contradictions_are_task_scoped_and_cover_authoritative_identifiers() {
        let mut repository_a = repository_fact(Some("/workspace/a".to_string()));
        repository_a.source_line = 2;
        let mut repository_b = repository_fact(Some("/workspace/b".to_string()));
        repository_b.source_line = 3;
        let mut target_a = deployment_target_fact(
            "NQLA_PRESTAGE",
            "prerelease",
            "prerelease",
            "qcash-prerelease",
        );
        target_a.source_line = 10;
        let mut target_b = deployment_target_fact("NQLA_PRESTAGE", "drc", "drc", "qcash-drc");
        target_b.source_line = 11;
        let mut connector_a = connector_rule("https://confluence.example", &["get_page"]);
        connector_a.source_line = 20;
        let mut connector_b = connector_rule("https://other.example", &["search"]);
        connector_b.source_line = 21;
        let policy = crate::domain::governance::VerificationRuleFact {
            id: "source-read".to_string(),
            connector_id: "corporate-confluence".to_string(),
            applies_to_labels: vec!["nqla_prestage".to_string()],
            required_operations: vec!["get_page".to_string()],
            source: "global.agent_rules".to_string(),
            source_line: 30,
        };
        let mut conflicting_policy = policy.clone();
        conflicting_policy.required_operations = vec!["search".to_string()];
        conflicting_policy.source_line = 31;
        let facts = RuleFactsRecord {
            repositories: vec![repository_a, repository_b],
            deployment_targets: vec![target_a, target_b],
            connectors: vec![connector_a, connector_b],
            verification_policies: vec![policy, conflicting_policy],
            ..Default::default()
        };

        let contradictions = resolve_rule_contradictions(
            &json!({
                "repository": {"id": "qcash-deployment"},
                "ticket": {"labels": ["NQLA_PRESTAGE"]}
            }),
            &["git".to_string()],
            &["corporate-confluence".to_string()],
            &facts,
        );
        assert_eq!(contradictions.len(), 4);
        assert_eq!(
            contradictions
                .iter()
                .map(|record| record.domain.as_str())
                .collect::<Vec<_>>(),
            vec![
                "connector",
                "deployment_target",
                "repository",
                "verification"
            ]
        );
        assert!(contradictions.iter().all(|record| {
            record.source_references.len() == 2
                && record
                    .source_references
                    .iter()
                    .all(|source| source.starts_with("global.agent_rules:"))
        }));

        let mut unrelated_facts = facts.clone();
        unrelated_facts.repositories[1].id = "other-repository".to_string();
        let unrelated = resolve_rule_contradictions(
            &json!({"ticket": {"labels": ["NQLA_DEV"]}}),
            &[],
            &[],
            &unrelated_facts,
        );
        assert!(unrelated.is_empty());
    }

    fn initialize_repository(path: &Path) {
        std::fs::create_dir_all(path).expect("repository directory");
        let status = std::process::Command::new(
            crate::infrastructure::git::git_executable().expect("git executable"),
        )
        .args(["init", "--quiet"])
        .current_dir(path)
        .status()
        .expect("git init runs");
        assert!(status.success());
    }

    #[test]
    fn repository_preflight_discovers_unique_named_checkout_and_records_provenance() {
        let directory = tempdir().expect("workspace");
        let repository = directory.path().join("BRI").join("qcash-deployment");
        initialize_repository(&repository);
        let mut fact = repository_fact(None);
        let secret = "repository-secret";
        fact.remote_url = format!(
            "https://user:{secret}@bitbucket.example/projects/OPS/repos/qcash-deployment?token={secret}"
        );
        let facts = RuleFactsRecord {
            repositories: vec![fact],
            ..Default::default()
        };

        let preflight = resolve_repository_preflight(
            &json!({ "objective": { "summary": "Create qcash-deployment Helm template" } }),
            &facts,
            directory.path(),
        );

        assert!(preflight.blocker.is_none());
        assert_eq!(
            preflight.repository_root.as_deref(),
            Some(
                repository
                    .canonicalize()
                    .expect("repository root")
                    .as_path()
            )
        );
        let record = preflight.record.expect("resolution record");
        assert_eq!(record.status, "resolved");
        assert_eq!(record.repository_id.as_deref(), Some("qcash-deployment"));
        assert_eq!(record.source_line, 2);
        assert_eq!(record.backend_path.as_deref(), Some("service"));
        assert!(!serde_json::to_string(&record)
            .expect("resolution serializes")
            .contains(secret));
    }

    #[test]
    fn repository_preflight_blocks_ambiguous_and_escaping_checkouts() {
        let directory = tempdir().expect("workspace");
        for parent in ["one", "two"] {
            initialize_repository(&directory.path().join(parent).join("qcash-deployment"));
        }
        let facts = RuleFactsRecord {
            repositories: vec![repository_fact(None)],
            ..Default::default()
        };
        let ambiguous = resolve_repository_preflight(&json!({}), &facts, directory.path());
        assert_eq!(
            ambiguous.record.expect("ambiguous record").status,
            "ambiguous"
        );
        assert!(ambiguous.blocker.is_some());

        let outside = tempdir().expect("outside");
        initialize_repository(outside.path());
        let escaping_facts = RuleFactsRecord {
            repositories: vec![repository_fact(Some(
                outside.path().to_string_lossy().to_string(),
            ))],
            ..Default::default()
        };
        let escaping = resolve_repository_preflight(&json!({}), &escaping_facts, directory.path());
        assert_eq!(
            escaping.record.expect("escape record").status,
            "outside_workspace"
        );
        assert!(escaping.blocker.is_some());
    }

    #[test]
    fn repository_preflight_blocks_conflicting_contract_and_rule_paths() {
        let directory = tempdir().expect("workspace");
        let ruled = directory.path().join("ruled").join("qcash-deployment");
        let contracted = directory.path().join("contracted").join("qcash-deployment");
        initialize_repository(&ruled);
        initialize_repository(&contracted);
        let facts = RuleFactsRecord {
            repositories: vec![repository_fact(Some(ruled.to_string_lossy().to_string()))],
            ..Default::default()
        };

        let preflight = resolve_repository_preflight(
            &json!({
                "repository": {
                    "root_path": contracted.to_string_lossy(),
                }
            }),
            &facts,
            directory.path(),
        );

        assert_eq!(
            preflight.record.expect("conflict record").status,
            "conflict"
        );
        assert!(preflight
            .blocker
            .as_deref()
            .is_some_and(|reason| reason.contains("conflicts")));
    }

    #[test]
    fn deployment_target_preflight_resolves_exact_label_with_provenance() {
        let facts = RuleFactsRecord {
            deployment_targets: vec![deployment_target_fact(
                "NQLA_PRESTAGE",
                "prerelease",
                "prerelease",
                "qcash-prerelease",
            )],
            ..Default::default()
        };
        let preflight = resolve_deployment_target_preflight(
            &json!({ "ticket": { "labels": ["nqla_prestage"] } }),
            &facts,
        );

        assert!(preflight.blocker.is_none());
        let target = preflight.target.expect("target resolved");
        assert_eq!(target.branch, "prerelease");
        assert_eq!(target.namespace, "qcash-prerelease");
        let record = preflight.record.expect("resolution retained");
        assert_eq!(record.status, "resolved");
        assert_eq!(record.selector_kind.as_deref(), Some("ticket_label"));
        assert_eq!(record.selector_value.as_deref(), Some("NQLA_PRESTAGE"));
        assert_eq!(record.source_line, 10);
    }

    #[test]
    fn deployment_target_preflight_resolves_explicit_contract_target_without_ticket_label() {
        let facts = RuleFactsRecord {
            deployment_targets: vec![deployment_target_fact(
                "NQLA_PRESTAGE",
                "prerelease",
                "prerelease",
                "qcash-prerelease",
            )],
            ..Default::default()
        };
        let preflight = resolve_deployment_target_preflight(
            &json!({"ticket": {"labels": []}, "deployment": {"target": "prerelease"}}),
            &facts,
        );

        assert!(preflight.blocker.is_none());
        let target = preflight.target.expect("explicit target resolved");
        assert_eq!(target.branch, "prerelease");
        assert_eq!(target.namespace, "qcash-prerelease");
        let record = preflight.record.expect("resolution retained");
        assert_eq!(record.selector_kind.as_deref(), Some("explicit_target"));
        assert_eq!(record.selector_value.as_deref(), Some("prerelease"));
        assert_eq!(record.matched_label.as_deref(), Some("NQLA_PRESTAGE"));
    }

    #[test]
    fn deployment_target_preflight_requires_label_and_explicit_target_to_agree() {
        let facts = RuleFactsRecord {
            deployment_targets: vec![
                deployment_target_fact("NQLA_PRESTAGE", "prerelease", "prerelease", "pre"),
                deployment_target_fact("NQLA_DRC", "drc", "drc", "disaster-recovery"),
            ],
            ..Default::default()
        };
        let matching = resolve_deployment_target_preflight(
            &json!({
                "ticket": {"labels": ["NQLA_PRESTAGE"]},
                "deployment": {"target": "prerelease"}
            }),
            &facts,
        );
        assert!(matching.blocker.is_none());
        assert_eq!(
            matching
                .record
                .expect("combined resolution")
                .selector_kind
                .as_deref(),
            Some("combined")
        );

        let conflicting = resolve_deployment_target_preflight(
            &json!({
                "ticket": {"labels": ["NQLA_PRESTAGE"]},
                "deployment": {"target": "drc"}
            }),
            &facts,
        );
        assert!(conflicting.target.is_none());
        assert_eq!(
            conflicting.record.expect("conflict retained").status,
            "conflict"
        );
        assert!(conflicting.blocker.is_some());
    }

    #[test]
    fn deployment_target_preflight_blocks_unknown_invalid_and_ambiguous_explicit_targets() {
        let facts = RuleFactsRecord {
            deployment_targets: vec![
                deployment_target_fact("NQLA_PRESTAGE", "shared", "prerelease", "pre"),
                deployment_target_fact("NQLA_DRC", "shared", "drc", "disaster-recovery"),
            ],
            ..Default::default()
        };
        let unknown = resolve_deployment_target_preflight(
            &json!({"deployment": {"target": "production"}}),
            &facts,
        );
        assert_eq!(
            unknown.record.expect("unknown retained").status,
            "unresolved"
        );

        let invalid = resolve_deployment_target_preflight(
            &json!({"deployment": {"target": "../production"}}),
            &facts,
        );
        assert_eq!(invalid.record.expect("invalid retained").status, "invalid");

        let ambiguous = resolve_deployment_target_preflight(
            &json!({"deployment": {"target": "shared"}}),
            &facts,
        );
        assert_eq!(
            ambiguous.record.expect("ambiguity retained").status,
            "ambiguous"
        );
        assert!(ambiguous.target.is_none());
        let contradictions = resolve_rule_contradictions(
            &json!({"deployment": {"target": "shared"}}),
            &[],
            &[],
            &facts,
        );
        assert_eq!(contradictions.len(), 1);
        assert_eq!(contradictions[0].key, "target:shared");
    }

    #[test]
    fn deployment_target_preflight_blocks_conflicting_labels() {
        let facts = RuleFactsRecord {
            deployment_targets: vec![
                deployment_target_fact("NQLA_PRESTAGE", "prerelease", "prerelease", "pre"),
                deployment_target_fact("NQLA_DEV", "dev", "dev", "development"),
            ],
            ..Default::default()
        };
        let preflight = resolve_deployment_target_preflight(
            &json!({ "ticket": { "labels": ["NQLA_PRESTAGE", "NQLA_DEV"] } }),
            &facts,
        );

        assert!(preflight.target.is_none());
        assert_eq!(
            preflight.record.expect("ambiguity retained").status,
            "ambiguous"
        );
        assert!(preflight.blocker.is_some());

        let invalid_facts = RuleFactsRecord {
            deployment_targets: vec![deployment_target_fact(
                "NQLA_BAD",
                "bad",
                "--delete",
                "../production",
            )],
            ..Default::default()
        };
        let invalid = resolve_deployment_target_preflight(
            &json!({ "ticket": { "labels": ["NQLA_BAD"] } }),
            &invalid_facts,
        );
        assert_eq!(invalid.record.expect("invalid retained").status, "invalid");
        assert!(invalid.target.is_none());
    }

    #[test]
    fn connector_configuration_preflight_verifies_url_and_live_operations() {
        let facts = RuleFactsRecord {
            connectors: vec![connector_rule(
                "https://confluence.example",
                &["search", "get_page"],
            )],
            ..Default::default()
        };
        let (records, blockers) = resolve_connector_configuration_preflights(
            &["corporate-confluence".to_string()],
            &facts,
            &[confluence_server("https://confluence.example/")],
            &[confluence_capabilities(&[
                "confluence_search",
                "confluence_get_page",
            ])],
        );

        assert!(blockers.is_empty());
        assert_eq!(records[0].status, "ready");
        assert_eq!(
            records[0].verified_tools,
            vec!["confluence_get_page", "confluence_search"]
        );
        assert_eq!(records[0].source_line, 20);
    }

    #[test]
    fn connector_configuration_preflight_blocks_mismatch_missing_and_ambiguous_tools() {
        let facts = RuleFactsRecord {
            connectors: vec![connector_rule("https://confluence.example", &["search"])],
            ..Default::default()
        };
        let (mismatch, blockers) = resolve_connector_configuration_preflights(
            &["corporate-confluence".to_string()],
            &facts,
            &[confluence_server("'https://wrong.example'")],
            &[confluence_capabilities(&["confluence_search"])],
        );
        assert_eq!(mismatch[0].status, "missing_configuration");
        assert_eq!(blockers.len(), 1);

        let (wrong_url, _) = resolve_connector_configuration_preflights(
            &["corporate-confluence".to_string()],
            &facts,
            &[confluence_server("https://wrong.example")],
            &[confluence_capabilities(&["confluence_search"])],
        );
        assert_eq!(wrong_url[0].status, "url_mismatch");

        let (missing, _) = resolve_connector_configuration_preflights(
            &["corporate-confluence".to_string()],
            &facts,
            &[confluence_server("https://confluence.example")],
            &[confluence_capabilities(&["confluence_get_page"])],
        );
        assert_eq!(missing[0].status, "missing_operations");

        let (ambiguous, _) = resolve_connector_configuration_preflights(
            &["corporate-confluence".to_string()],
            &facts,
            &[confluence_server("https://confluence.example")],
            &[confluence_capabilities(&[
                "legacy_search",
                "alternate_search",
            ])],
        );
        assert_eq!(ambiguous[0].status, "ambiguous_operation");

        let (missing_rule, _) =
            resolve_connector_configuration_preflights(&["bamboo".to_string()], &facts, &[], &[]);
        assert_eq!(missing_rule[0].status, "missing_rule");
    }

    fn test_governance(task_session_id: u64) -> GovernanceResolutionRecord {
        use crate::domain::governance::{
            GovernanceResolutionStatus, RulesResolutionRecord, SkillResolutionRecord,
            GOVERNANCE_RESOLUTION_VERSION,
        };
        GovernanceResolutionRecord {
            schema_version: GOVERNANCE_RESOLUTION_VERSION,
            task_session_id,
            resolved_at: 1,
            status: GovernanceResolutionStatus::LegacyUnavailable,
            rules: RulesResolutionRecord {
                normalization_version: "legacy_unavailable".to_string(),
                final_digest: crate::infrastructure::runtime_profile_store::content_revision(""),
                entries: Vec::new(),
                facts: Default::default(),
                snapshot: String::new(),
            },
            skills: SkillResolutionRecord {
                catalog_revision: None,
                selected_skill_ids: Vec::new(),
                entries: Vec::new(),
                snapshot: String::new(),
            },
        }
    }

    fn test_envelope() -> TaskSessionEnvelope {
        let contract = json!({ "contract": "test" });
        TaskSessionEnvelope::V1(TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: Some("card-1".to_string()),
            conversation_id: Some("conversation-1".to_string()),
            execution_run_id: Some("run-1".to_string()),
            context_digest: execution_contract_digest(&contract).expect("contract digest"),
            runtime_profile_id: "profile-1".to_string(),
            model: "openai/gpt-5".to_string(),
            connector_ids: vec!["jira".to_string()],
            requested_capabilities: vec!["external_tools:jira".to_string()],
            prompt_template_version: "prompt-v1".to_string(),
            context_revision: None,
            rules_revision: None,
            skills_revision: None,
        })
    }
}
