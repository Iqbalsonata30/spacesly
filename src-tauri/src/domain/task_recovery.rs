//! Provider-neutral runtime failure classification and bounded recovery policy.

use serde::{Deserialize, Serialize};

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
}
