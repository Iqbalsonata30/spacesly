//! Replayable, provider-neutral evaluation fixtures for deterministic Agent policy.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::domain::task_recovery::{
    decide_runtime_recovery, RuntimeFailureClass, RuntimeRecoveryAction, RuntimeRecoveryContext,
};

pub const AGENT_EVALUATION_SCHEMA_VERSION: u32 = 1;
const MAX_FIXTURES: usize = 1_024;
const MAX_FIXTURE_ERROR_BYTES: usize = 4_096;
const EMBEDDED_RECOVERY_CORPUS: &str =
    include_str!("../../evaluation-fixtures/runtime-recovery-v1.json");

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvaluationCategory {
    Planning,
    SafeExecution,
    Recovery,
    EvidenceQuality,
}

impl AgentEvaluationCategory {
    fn all() -> [Self; 4] {
        [
            Self::Planning,
            Self::SafeExecution,
            Self::Recovery,
            Self::EvidenceQuality,
        ]
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentEvaluationCorpus {
    pub schema_version: u32,
    pub corpus_id: String,
    pub fixtures: Vec<AgentEvaluationFixture>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentEvaluationFixture {
    pub fixture_id: String,
    pub category: AgentEvaluationCategory,
    #[serde(flatten)]
    pub scenario: AgentEvaluationScenario,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "scenario", rename_all = "snake_case")]
pub enum AgentEvaluationScenario {
    RuntimeRecovery {
        error: String,
        retries_attempted: u8,
        max_automatic_retries: u8,
        successful_mutation_observed: bool,
        cancellation_requested: bool,
        expected: RuntimeRecoveryExpectation,
    },
    ModelResult {
        response: String,
        expected_objective_ids: Vec<String>,
        sensitive_approval_required: bool,
        approval_granted: bool,
        expected: ModelResultExpectation,
    },
    PlanningProposal {
        response: String,
        expected: PlanningProposalExpectation,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeRecoveryExpectation {
    pub failure_class: RuntimeFailureClass,
    pub action: RuntimeRecoveryAction,
    pub retryable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelResultExpectation {
    pub completion_status: String,
    pub objective_result_count: usize,
    pub blocked_reason_present: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelResultObservation {
    pub completion_status: String,
    pub objective_result_count: usize,
    pub blocked_reason_present: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanningProposalExpectation {
    pub accepted: bool,
    pub objective_count: usize,
    pub mutation_objective_count: usize,
    pub objective_ids: Vec<String>,
    pub operation_hints: Vec<String>,
    pub resource_hints: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningProposalObservation {
    pub accepted: bool,
    pub objective_count: usize,
    pub mutation_objective_count: usize,
    pub objective_ids: Vec<String>,
    pub operation_hints: Vec<String>,
    pub resource_hints: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentEvaluationReport {
    pub schema_version: u32,
    pub corpus_id: String,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub pass_rate_basis_points: u16,
    pub categories: BTreeMap<AgentEvaluationCategory, AgentEvaluationCategoryReport>,
    pub failures: Vec<AgentEvaluationFailure>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentEvaluationCategoryReport {
    pub evaluated: bool,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub pass_rate_basis_points: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentEvaluationFailure {
    pub fixture_id: String,
    pub category: AgentEvaluationCategory,
    pub mismatches: Vec<String>,
}

pub fn embedded_agent_evaluation_corpus() -> Result<AgentEvaluationCorpus, String> {
    parse_agent_evaluation_corpus(EMBEDDED_RECOVERY_CORPUS)
}

pub fn parse_agent_evaluation_corpus(value: &str) -> Result<AgentEvaluationCorpus, String> {
    let corpus: AgentEvaluationCorpus = serde_json::from_str(value)
        .map_err(|error| format!("Agent evaluation corpus is invalid JSON: {error}"))?;
    validate_corpus(&corpus)?;
    Ok(corpus)
}

pub fn evaluate_agent_corpus(
    corpus: &AgentEvaluationCorpus,
    model_result_validator: impl Fn(&str, &[String], bool, bool) -> ModelResultObservation,
    planning_proposal_validator: impl Fn(&str) -> PlanningProposalObservation,
) -> Result<AgentEvaluationReport, String> {
    validate_corpus(corpus)?;
    let mut failures = Vec::new();
    let mut category_counts = BTreeMap::<AgentEvaluationCategory, (usize, usize)>::new();
    for fixture in &corpus.fixtures {
        let mismatches = evaluate_fixture(
            fixture,
            &model_result_validator,
            &planning_proposal_validator,
        );
        let counts = category_counts.entry(fixture.category).or_default();
        counts.0 += 1;
        if mismatches.is_empty() {
            counts.1 += 1;
        } else {
            failures.push(AgentEvaluationFailure {
                fixture_id: fixture.fixture_id.clone(),
                category: fixture.category,
                mismatches,
            });
        }
    }
    let total = corpus.fixtures.len();
    let failed = failures.len();
    let passed = total.saturating_sub(failed);
    let categories = AgentEvaluationCategory::all()
        .into_iter()
        .map(|category| {
            let (total, passed) = category_counts.get(&category).copied().unwrap_or_default();
            let failed = total.saturating_sub(passed);
            (
                category,
                AgentEvaluationCategoryReport {
                    evaluated: total > 0,
                    total,
                    passed,
                    failed,
                    pass_rate_basis_points: (total > 0).then(|| pass_rate(passed, total)),
                },
            )
        })
        .collect();
    Ok(AgentEvaluationReport {
        schema_version: AGENT_EVALUATION_SCHEMA_VERSION,
        corpus_id: corpus.corpus_id.clone(),
        total,
        passed,
        failed,
        pass_rate_basis_points: pass_rate(passed, total),
        categories,
        failures,
    })
}

fn evaluate_fixture(
    fixture: &AgentEvaluationFixture,
    model_result_validator: &impl Fn(&str, &[String], bool, bool) -> ModelResultObservation,
    planning_proposal_validator: &impl Fn(&str) -> PlanningProposalObservation,
) -> Vec<String> {
    match &fixture.scenario {
        AgentEvaluationScenario::RuntimeRecovery {
            error,
            retries_attempted,
            max_automatic_retries,
            successful_mutation_observed,
            cancellation_requested,
            expected,
        } => {
            let observed = decide_runtime_recovery(
                error,
                RuntimeRecoveryContext {
                    retries_attempted: *retries_attempted,
                    max_automatic_retries: *max_automatic_retries,
                    successful_mutation_observed: *successful_mutation_observed,
                    cancellation_requested: *cancellation_requested,
                },
            );
            let mut mismatches = Vec::new();
            if observed.failure_class != expected.failure_class {
                mismatches.push("failure_class".to_string());
            }
            if observed.action != expected.action {
                mismatches.push("action".to_string());
            }
            if observed.retryable != expected.retryable {
                mismatches.push("retryable".to_string());
            }
            mismatches
        }
        AgentEvaluationScenario::ModelResult {
            response,
            expected_objective_ids,
            sensitive_approval_required,
            approval_granted,
            expected,
        } => {
            let observed = model_result_validator(
                response,
                expected_objective_ids,
                *sensitive_approval_required,
                *approval_granted,
            );
            let mut mismatches = Vec::new();
            if observed.completion_status != expected.completion_status {
                mismatches.push("completion_status".to_string());
            }
            if observed.objective_result_count != expected.objective_result_count {
                mismatches.push("objective_result_count".to_string());
            }
            if observed.blocked_reason_present != expected.blocked_reason_present {
                mismatches.push("blocked_reason_present".to_string());
            }
            mismatches
        }
        AgentEvaluationScenario::PlanningProposal { response, expected } => {
            let observed = planning_proposal_validator(response);
            let mut mismatches = Vec::new();
            if observed.accepted != expected.accepted {
                mismatches.push("accepted".to_string());
            }
            if observed.objective_count != expected.objective_count {
                mismatches.push("objective_count".to_string());
            }
            if observed.mutation_objective_count != expected.mutation_objective_count {
                mismatches.push("mutation_objective_count".to_string());
            }
            if observed.objective_ids != expected.objective_ids {
                mismatches.push("objective_ids".to_string());
            }
            if observed.operation_hints != expected.operation_hints {
                mismatches.push("operation_hints".to_string());
            }
            if observed.resource_hints != expected.resource_hints {
                mismatches.push("resource_hints".to_string());
            }
            mismatches
        }
    }
}

fn validate_corpus(corpus: &AgentEvaluationCorpus) -> Result<(), String> {
    if corpus.schema_version != AGENT_EVALUATION_SCHEMA_VERSION {
        return Err("Agent evaluation corpus schema version is unsupported.".to_string());
    }
    if !valid_identifier(&corpus.corpus_id) {
        return Err("Agent evaluation corpus ID is invalid.".to_string());
    }
    if corpus.fixtures.is_empty() || corpus.fixtures.len() > MAX_FIXTURES {
        return Err("Agent evaluation corpus fixture count is invalid.".to_string());
    }
    let mut fixture_ids = HashSet::new();
    for fixture in &corpus.fixtures {
        if !valid_identifier(&fixture.fixture_id) || !fixture_ids.insert(&fixture.fixture_id) {
            return Err("Agent evaluation fixture IDs must be unique and canonical.".to_string());
        }
        match &fixture.scenario {
            AgentEvaluationScenario::RuntimeRecovery {
                error,
                max_automatic_retries,
                ..
            } => {
                if error.trim().is_empty()
                    || error.len() > MAX_FIXTURE_ERROR_BYTES
                    || error.chars().any(char::is_control)
                    || *max_automatic_retries > 10
                {
                    return Err("Agent runtime-recovery fixture is invalid.".to_string());
                }
            }
            AgentEvaluationScenario::ModelResult {
                response,
                expected_objective_ids,
                expected,
                ..
            } => {
                if response.trim().is_empty()
                    || response.len() > 64 * 1024
                    || expected_objective_ids.len() > 8
                    || expected_objective_ids
                        .iter()
                        .any(|id| !valid_identifier(id))
                    || !matches!(expected.completion_status.as_str(), "completed" | "blocked")
                    || expected.objective_result_count > 8
                {
                    return Err("Agent model-result fixture is invalid.".to_string());
                }
            }
            AgentEvaluationScenario::PlanningProposal { response, expected } => {
                let mut expected_values = expected
                    .operation_hints
                    .iter()
                    .chain(expected.resource_hints.iter());
                if response.trim().is_empty()
                    || response.len() > 64 * 1024
                    || expected.objective_count > 8
                    || (expected.accepted && expected.objective_count == 0)
                    || expected.mutation_objective_count > expected.objective_count
                    || expected.objective_ids.len() != expected.objective_count
                    || expected
                        .objective_ids
                        .iter()
                        .any(|id| !valid_identifier(id))
                    || expected.operation_hints.len() > 128
                    || expected.resource_hints.len() > 128
                    || expected_values.any(|value| {
                        value.trim().is_empty()
                            || value.len() > 120
                            || value.chars().any(char::is_control)
                    })
                    || (!expected.accepted
                        && (expected.objective_count != 0
                            || expected.mutation_objective_count != 0
                            || !expected.objective_ids.is_empty()
                            || !expected.operation_hints.is_empty()
                            || !expected.resource_hints.is_empty()))
                {
                    return Err("Agent planning-proposal fixture is invalid.".to_string());
                }
            }
        }
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn pass_rate(passed: usize, total: usize) -> u16 {
    if total == 0 {
        return 0;
    }
    u16::try_from(passed.saturating_mul(10_000) / total).unwrap_or(10_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_model_validator(
        response: &str,
        expected_objective_ids: &[String],
        sensitive_approval_required: bool,
        _approval_granted: bool,
    ) -> ModelResultObservation {
        let parsed = serde_json::from_str::<serde_json::Value>(response).ok();
        let objective_result_count = parsed
            .as_ref()
            .and_then(|value| value.get("objective_results"))
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        let missing_objective_evidence = parsed
            .as_ref()
            .and_then(|value| value.get("objective_results"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|objectives| {
                objectives.iter().any(|objective| {
                    objective
                        .get("evidence")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(Vec::is_empty)
                })
            });
        let observed_ids = parsed
            .as_ref()
            .and_then(|value| value.get("objective_results"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|objective| objective.get("objective_id"))
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>();
        let unique_ids = observed_ids.iter().copied().collect::<HashSet<_>>();
        let coverage_invalid = observed_ids.len() != expected_objective_ids.len()
            || unique_ids.len() != observed_ids.len();
        let invalid = parsed.is_none()
            || sensitive_approval_required
            || missing_objective_evidence
            || coverage_invalid;
        ModelResultObservation {
            completion_status: if invalid { "blocked" } else { "completed" }.to_string(),
            objective_result_count: if missing_objective_evidence {
                0
            } else {
                objective_result_count
            },
            blocked_reason_present: invalid,
        }
    }

    fn fixture_planning_validator(response: &str) -> PlanningProposalObservation {
        let parsed = serde_json::from_str::<serde_json::Value>(response).ok();
        let objectives = parsed
            .as_ref()
            .and_then(|value| value.get("objectives"))
            .and_then(serde_json::Value::as_array);
        let accepted = objectives.is_some_and(|objectives| {
            !objectives.is_empty()
                && objectives.len() <= 8
                && objectives.iter().all(|objective| {
                    objective
                        .get("summary")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty())
                        && objective
                            .get("success_evidence")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|value| !value.trim().is_empty())
                })
        });
        if !accepted {
            return PlanningProposalObservation {
                accepted: false,
                objective_count: 0,
                mutation_objective_count: 0,
                objective_ids: Vec::new(),
                operation_hints: Vec::new(),
                resource_hints: Vec::new(),
            };
        }
        let objectives = objectives.expect("accepted proposal has objectives");
        let mut operation_hints = fixture_planning_hints(objectives, "operation_hints");
        let mut resource_hints = fixture_planning_hints(objectives, "resource_hints");
        operation_hints.sort();
        operation_hints.dedup();
        resource_hints.sort();
        resource_hints.dedup();
        PlanningProposalObservation {
            accepted: true,
            objective_count: objectives.len(),
            mutation_objective_count: objectives
                .iter()
                .filter(|objective| {
                    objective
                        .get("mutation_expected")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or_default()
                })
                .count(),
            objective_ids: (1..=objectives.len())
                .map(|index| format!("objective-{index}"))
                .collect(),
            operation_hints,
            resource_hints,
        }
    }

    fn fixture_planning_hints(objectives: &[serde_json::Value], field: &str) -> Vec<String> {
        objectives
            .iter()
            .filter_map(|objective| objective.get(field))
            .filter_map(serde_json::Value::as_array)
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(|value| value.trim().to_lowercase())
            .filter(|value| !value.is_empty() && value.len() <= 120)
            .collect()
    }

    #[test]
    fn embedded_corpus_passes_and_never_invents_uncovered_scores() {
        let corpus = embedded_agent_evaluation_corpus().expect("embedded corpus parses");
        let report =
            evaluate_agent_corpus(&corpus, fixture_model_validator, fixture_planning_validator)
                .expect("corpus evaluates");
        assert_eq!(report.total, 18);
        assert_eq!(report.passed, 18);
        assert_eq!(report.failed, 0);
        assert_eq!(report.pass_rate_basis_points, 10_000);
        assert!(report.categories[&AgentEvaluationCategory::Recovery].evaluated);
        assert!(report.categories[&AgentEvaluationCategory::SafeExecution].evaluated);
        assert!(report.categories[&AgentEvaluationCategory::Planning].evaluated);
        assert_eq!(
            report.categories[&AgentEvaluationCategory::Planning].pass_rate_basis_points,
            Some(10_000)
        );
        assert!(report.categories[&AgentEvaluationCategory::EvidenceQuality].evaluated);

        let mut without_planning = corpus;
        without_planning
            .fixtures
            .retain(|fixture| fixture.category != AgentEvaluationCategory::Planning);
        let uncovered_report = evaluate_agent_corpus(
            &without_planning,
            fixture_model_validator,
            fixture_planning_validator,
        )
        .expect("partial corpus evaluates");
        assert!(!uncovered_report.categories[&AgentEvaluationCategory::Planning].evaluated);
        assert_eq!(
            uncovered_report.categories[&AgentEvaluationCategory::Planning].pass_rate_basis_points,
            None
        );
    }

    #[test]
    fn changed_expectation_produces_deterministic_secret_free_failure() {
        let mut corpus = embedded_agent_evaluation_corpus().expect("embedded corpus parses");
        let AgentEvaluationScenario::RuntimeRecovery {
            expected, error, ..
        } = &mut corpus.fixtures[0].scenario
        else {
            panic!("first fixture must exercise runtime recovery");
        };
        *error = "timeout token=private-value".to_string();
        expected.action = RuntimeRecoveryAction::StopFailed;
        let report =
            evaluate_agent_corpus(&corpus, fixture_model_validator, fixture_planning_validator)
                .expect("corpus evaluates");
        assert_eq!(report.failed, 1);
        assert_eq!(report.failures[0].mismatches, ["action"]);
        let encoded = serde_json::to_string(&report).expect("report serializes");
        assert!(!encoded.contains("private-value"));
        assert!(!encoded.contains("token="));
    }

    #[test]
    fn planning_failure_report_never_echoes_the_model_response() {
        let mut corpus = embedded_agent_evaluation_corpus().expect("embedded corpus parses");
        let fixture = corpus
            .fixtures
            .iter_mut()
            .find(|fixture| {
                matches!(
                    fixture.scenario,
                    AgentEvaluationScenario::PlanningProposal { .. }
                )
            })
            .expect("planning fixture exists");
        let AgentEvaluationScenario::PlanningProposal { response, expected } =
            &mut fixture.scenario
        else {
            unreachable!("selected fixture is a planning proposal");
        };
        *response = r#"{"objectives":[{"summary":"token=planning-private-value","success_evidence":"verified","mutation_expected":true}]}"#.to_string();
        expected.objective_count = 1;
        expected.mutation_objective_count = 0;
        expected.objective_ids = vec!["objective-1".to_string()];
        expected.operation_hints.clear();
        expected.resource_hints.clear();

        let report =
            evaluate_agent_corpus(&corpus, fixture_model_validator, fixture_planning_validator)
                .expect("corpus evaluates");
        assert_eq!(report.failed, 1);
        assert_eq!(report.failures[0].mismatches, ["mutation_objective_count"]);
        let encoded = serde_json::to_string(&report).expect("report serializes");
        assert!(!encoded.contains("planning-private-value"));
        assert!(!encoded.contains("token="));
    }

    #[test]
    fn malformed_or_duplicate_fixtures_fail_closed() {
        let mut corpus = embedded_agent_evaluation_corpus().expect("embedded corpus parses");
        corpus.fixtures[1].fixture_id = corpus.fixtures[0].fixture_id.clone();
        assert!(evaluate_agent_corpus(
            &corpus,
            fixture_model_validator,
            fixture_planning_validator,
        )
        .expect_err("duplicate fixture rejected")
        .contains("unique"));
        assert!(parse_agent_evaluation_corpus(r#"{"schema_version":2}"#).is_err());
    }
}
