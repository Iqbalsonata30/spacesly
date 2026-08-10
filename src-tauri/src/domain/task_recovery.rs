//! Provider-neutral runtime failure classification and bounded recovery policy.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::domain::task_examination::{
    CapabilityRepairGuidance, ConnectorDiscoveryStatus, TaskExaminationRecord,
};

/// Stable class assigned to a runtime or external-tool failure.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeFailureClass {
    ApprovalRequired,
    Cancelled,
    TransientTransport,
    RateLimited,
    Authentication,
    Authorization,
    MissingCapability,
    InvalidRequest,
    Unknown,
}

/// Backend-owned action selected for a classified runtime failure.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeRecoveryAction {
    RetryCurrentAssignment,
    AwaitOperator,
    ReviewUncertainOutcome,
    StopCancelled,
    StopFailed,
}

/// Facts that bound recovery without granting new authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeRecoveryContext {
    pub retries_attempted: u8,
    pub max_automatic_retries: u8,
    pub successful_mutation_observed: bool,
    pub cancellation_requested: bool,
}

/// Auditable deterministic recovery decision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeRecoveryDecision {
    pub schema_version: u32,
    pub failure_class: RuntimeFailureClass,
    pub action: RuntimeRecoveryAction,
    pub retryable: bool,
    pub reason: String,
}

/// Deterministic outcome of checking whether a missing read tool has a safe live replacement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilityRepairDecision {
    pub schema_version: u32,
    pub repairable: bool,
    pub reason: String,
    pub guidance: Option<CapabilityRepairGuidance>,
}

impl RuntimeRecoveryDecision {
    /// Returns true only for the bounded automatic replay action.
    pub fn should_retry(&self) -> bool {
        self.action == RuntimeRecoveryAction::RetryCurrentAssignment
    }
}

/// Classifies a failure and selects a conservative recovery action.
pub fn decide_runtime_recovery(
    error: &str,
    context: RuntimeRecoveryContext,
) -> RuntimeRecoveryDecision {
    let failure_class = classify_runtime_failure(error, context.cancellation_requested);
    let (action, retryable, reason) = if context.cancellation_requested
        || failure_class == RuntimeFailureClass::Cancelled
    {
        (
            RuntimeRecoveryAction::StopCancelled,
            false,
            "Execution cancellation was requested; Spacesly will not retry.".to_string(),
        )
    } else if matches!(
        failure_class,
        RuntimeFailureClass::ApprovalRequired
            | RuntimeFailureClass::Authentication
            | RuntimeFailureClass::Authorization
            | RuntimeFailureClass::MissingCapability
    ) {
        (
            RuntimeRecoveryAction::AwaitOperator,
            false,
            "Operator action or configuration is required before execution can continue."
                .to_string(),
        )
    } else if context.successful_mutation_observed {
        (
            RuntimeRecoveryAction::ReviewUncertainOutcome,
            false,
            "A mutation succeeded before the failure, so automatic replay could duplicate an external change. Review the recorded tool evidence before continuing.".to_string(),
        )
    } else {
        match failure_class {
            RuntimeFailureClass::TransientTransport | RuntimeFailureClass::RateLimited
                if context.retries_attempted < context.max_automatic_retries =>
            {
                (
                    RuntimeRecoveryAction::RetryCurrentAssignment,
                    true,
                    "The failure is transient and no successful mutation was observed; retry the same fenced assignment once.".to_string(),
                )
            }
            RuntimeFailureClass::TransientTransport | RuntimeFailureClass::RateLimited => (
                RuntimeRecoveryAction::StopFailed,
                false,
                "The bounded automatic retry was exhausted.".to_string(),
            ),
            RuntimeFailureClass::ApprovalRequired
            | RuntimeFailureClass::Authentication
            | RuntimeFailureClass::Authorization
            | RuntimeFailureClass::MissingCapability => unreachable!(),
            RuntimeFailureClass::Cancelled => unreachable!(),
            RuntimeFailureClass::InvalidRequest | RuntimeFailureClass::Unknown => (
                RuntimeRecoveryAction::StopFailed,
                false,
                "The failure is not safe to retry automatically.".to_string(),
            ),
        }
    };

    RuntimeRecoveryDecision {
        schema_version: 1,
        failure_class,
        action,
        retryable,
        reason,
    }
}

/// Selects strongly similar read-only alternatives from the same authorized live connector.
pub fn decide_capability_repair(
    examination: &TaskExaminationRecord,
    failure: &str,
    failed_tool: &str,
    failed_risk: &str,
    repairs_attempted: u8,
    successful_mutation_observed: bool,
) -> CapabilityRepairDecision {
    let blocked = |reason: &str| CapabilityRepairDecision {
        schema_version: 1,
        repairable: false,
        reason: reason.to_string(),
        guidance: None,
    };
    if repairs_attempted > 0 {
        return blocked("The bounded capability repair was already attempted.");
    }
    let failure = failure.to_ascii_lowercase();
    if !contains_any(
        &failure,
        &[
            "tool not found",
            "unknown tool",
            "tool is not exposed",
            "tool was not exposed",
            "no such tool",
        ],
    ) {
        return blocked("The failure does not prove that the MCP tool inventory drifted.");
    }
    if successful_mutation_observed {
        return blocked("A successful mutation was observed before capability drift.");
    }
    if !matches!(
        failed_risk.trim().to_ascii_lowercase().as_str(),
        "read" | "low"
    ) {
        return blocked("Only read-only tool substitutions can be repaired automatically.");
    }
    let failed_tool = failed_tool.trim();
    if failed_tool.is_empty() {
        return blocked("The failed runtime tool was not identified.");
    }

    let mapped_connector = examination.capability_mappings.iter().find(|mapping| {
        mapping.planned_tools.iter().any(|tool| tool == failed_tool)
            || failed_tool
                .to_ascii_lowercase()
                .contains(&mapping.connector_id.to_ascii_lowercase())
    });
    let snapshot = mapped_connector
        .and_then(|mapping| {
            examination
                .connector_capabilities
                .iter()
                .find(|snapshot| snapshot.connector_id == mapping.connector_id)
        })
        .or_else(|| {
            let available = examination
                .connector_capabilities
                .iter()
                .filter(|snapshot| snapshot.status == ConnectorDiscoveryStatus::Available)
                .collect::<Vec<_>>();
            (available.len() == 1).then_some(available[0])
        });
    let Some(snapshot) = snapshot else {
        return blocked("The failed tool could not be bound to one authorized live connector.");
    };
    if snapshot.status != ConnectorDiscoveryStatus::Available {
        return blocked("The owning connector has no live tool inventory.");
    }

    let failed_tokens = repair_tokens(failed_tool);
    let mut alternatives = snapshot
        .tools
        .iter()
        .filter(|tool| tool.name != failed_tool && tool.risk == "read")
        .filter_map(|tool| {
            let candidate_tokens = repair_tokens(&tool.name);
            let score = failed_tokens.intersection(&candidate_tokens).count();
            (score >= 2).then_some((score, tool.name.clone()))
        })
        .collect::<Vec<_>>();
    alternatives.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    alternatives.dedup_by(|left, right| left.1 == right.1);
    let alternatives = alternatives
        .into_iter()
        .take(5)
        .map(|(_, tool)| tool)
        .collect::<Vec<_>>();
    if alternatives.is_empty() {
        return blocked(
            "No strongly similar read-only replacement exists in the live connector inventory.",
        );
    }

    let reason = "The planned read tool is unavailable; use only the strongly similar live alternatives from the same authorized connector.".to_string();
    CapabilityRepairDecision {
        schema_version: 1,
        repairable: true,
        reason: reason.clone(),
        guidance: Some(CapabilityRepairGuidance {
            schema_version: 1,
            connector_id: snapshot.connector_id.clone(),
            failed_tool: failed_tool.to_string(),
            allowed_alternatives: alternatives,
            reason,
        }),
    }
}

fn repair_tokens(value: &str) -> HashSet<String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .map(str::trim)
        .filter(|token| token.len() > 1)
        .map(str::to_ascii_lowercase)
        .filter(|token| {
            !matches!(
                token.as_str(),
                "get" | "read" | "fetch" | "find" | "list" | "search" | "tool"
            )
        })
        .collect()
}

fn classify_runtime_failure(error: &str, cancellation_requested: bool) -> RuntimeFailureClass {
    if cancellation_requested {
        return RuntimeFailureClass::Cancelled;
    }
    let error = error.to_ascii_lowercase();
    if error.contains("[approval_required]") || error.contains("approval required") {
        RuntimeFailureClass::ApprovalRequired
    } else if contains_any(&error, &["cancelled", "canceled", "terminated by operator"]) {
        RuntimeFailureClass::Cancelled
    } else if contains_any(
        &error,
        &[
            "unauthorized",
            "authentication",
            "invalid token",
            "expired token",
            "status 401",
            "http 401",
        ],
    ) {
        RuntimeFailureClass::Authentication
    } else if contains_any(
        &error,
        &[
            "forbidden",
            "permission denied",
            "access denied",
            "status 403",
            "http 403",
        ],
    ) {
        RuntimeFailureClass::Authorization
    } else if contains_any(
        &error,
        &[
            "tool not found",
            "unknown tool",
            "tool is not exposed",
            "tool was not exposed",
            "no such tool",
            "missing capability",
            "capability preflight",
            "no file/git/shell tools",
            "no connection adapters",
        ],
    ) {
        RuntimeFailureClass::MissingCapability
    } else if contains_any(
        &error,
        &["rate limit", "too many requests", "status 429", "http 429"],
    ) {
        RuntimeFailureClass::RateLimited
    } else if contains_any(
        &error,
        &[
            "timeout",
            "timed out",
            "connection refused",
            "failed to connect",
            "connection reset",
            "broken pipe",
            "dns",
            "temporary failure",
            "temporarily unavailable",
            "service unavailable",
            "bad gateway",
            "gateway timeout",
            "status 502",
            "status 503",
            "status 504",
            "http 502",
            "http 503",
            "http 504",
        ],
    ) {
        RuntimeFailureClass::TransientTransport
    } else if contains_any(
        &error,
        &[
            "invalid request",
            "bad request",
            "validation failed",
            "status 400",
            "http 400",
        ],
    ) {
        RuntimeFailureClass::InvalidRequest
    } else {
        RuntimeFailureClass::Unknown
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::task_examination::{
        ConnectorCapabilityMapping, ConnectorCapabilitySnapshot, DiscoveredToolCapability,
    };

    fn context(
        retries_attempted: u8,
        successful_mutation_observed: bool,
    ) -> RuntimeRecoveryContext {
        RuntimeRecoveryContext {
            retries_attempted,
            max_automatic_retries: 1,
            successful_mutation_observed,
            cancellation_requested: false,
        }
    }

    #[test]
    fn transient_read_failure_retries_once() {
        let decision = decide_runtime_recovery("connection refused", context(0, false));
        assert_eq!(
            decision.failure_class,
            RuntimeFailureClass::TransientTransport
        );
        assert_eq!(
            decision.action,
            RuntimeRecoveryAction::RetryCurrentAssignment
        );
        assert!(decision.retryable);

        let exhausted = decide_runtime_recovery("connection refused", context(1, false));
        assert_eq!(exhausted.action, RuntimeRecoveryAction::StopFailed);
    }

    #[test]
    fn successful_mutation_prevents_automatic_replay() {
        let decision = decide_runtime_recovery("gateway timeout", context(0, true));
        assert_eq!(
            decision.action,
            RuntimeRecoveryAction::ReviewUncertainOutcome
        );
        assert!(!decision.retryable);
    }

    #[test]
    fn authority_failures_require_operator_action() {
        for error in [
            "[approval_required] restart",
            "HTTP 401 expired token",
            "HTTP 403 forbidden",
            "unknown tool confluence_get_page",
        ] {
            let decision = decide_runtime_recovery(error, context(0, false));
            assert_eq!(decision.action, RuntimeRecoveryAction::AwaitOperator);
        }
    }

    #[test]
    fn missing_read_tool_repairs_only_with_same_connector_live_alternatives() {
        let examination = TaskExaminationRecord {
            capability_mappings: vec![ConnectorCapabilityMapping {
                connector_id: "confluence".to_string(),
                planned_tools: vec!["confluence_get_page".to_string()],
                ..Default::default()
            }],
            connector_capabilities: vec![ConnectorCapabilitySnapshot {
                connector_id: "confluence".to_string(),
                status: ConnectorDiscoveryStatus::Available,
                tools: vec![
                    DiscoveredToolCapability {
                        name: "confluence_read_page".to_string(),
                        risk: "read".to_string(),
                        argument_names: vec!["page_id".to_string()],
                    },
                    DiscoveredToolCapability {
                        name: "confluence_delete_page".to_string(),
                        risk: "destructive".to_string(),
                        argument_names: vec!["page_id".to_string()],
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        let decision = decide_capability_repair(
            &examination,
            "unknown tool confluence_get_page",
            "confluence_get_page",
            "read",
            0,
            false,
        );
        assert!(decision.repairable);
        assert_eq!(
            decision
                .guidance
                .expect("repair guidance")
                .allowed_alternatives,
            vec!["confluence_read_page"]
        );
    }

    #[test]
    fn capability_repair_rejects_mutations_and_weak_matches() {
        let examination = TaskExaminationRecord {
            connector_capabilities: vec![ConnectorCapabilitySnapshot {
                connector_id: "bamboo".to_string(),
                status: ConnectorDiscoveryStatus::Available,
                tools: vec![DiscoveredToolCapability {
                    name: "bamboo_list_projects".to_string(),
                    risk: "read".to_string(),
                    argument_names: Vec::new(),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(
            !decide_capability_repair(
                &examination,
                "unknown tool bamboo_trigger_build",
                "bamboo_trigger_build",
                "mutation",
                0,
                false,
            )
            .repairable
        );
        assert!(
            !decide_capability_repair(
                &examination,
                "unknown tool bamboo_get_build",
                "bamboo_get_build",
                "read",
                0,
                false,
            )
            .repairable
        );
        assert!(
            !decide_capability_repair(
                &examination,
                "No connection adapters were found for the Confluence URL",
                "bamboo_get_build",
                "read",
                0,
                false,
            )
            .repairable
        );
    }
}
