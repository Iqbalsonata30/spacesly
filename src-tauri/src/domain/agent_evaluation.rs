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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeRecoveryExpectation {
    pub failure_class: RuntimeFailureClass,
    pub action: RuntimeRecoveryAction,
    pub retryable: bool,
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
) -> Result<AgentEvaluationReport, String> {
    validate_corpus(corpus)?;
    let mut failures = Vec::new();
    let mut category_counts = BTreeMap::<AgentEvaluationCategory, (usize, usize)>::new();
    for fixture in &corpus.fixtures {
        let mismatches = evaluate_fixture(fixture);
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

fn evaluate_fixture(fixture: &AgentEvaluationFixture) -> Vec<String> {
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

    #[test]
    fn embedded_recovery_corpus_passes_and_never_invents_uncovered_scores() {
        let corpus = embedded_agent_evaluation_corpus().expect("embedded corpus parses");
        let report = evaluate_agent_corpus(&corpus).expect("corpus evaluates");
        assert_eq!(report.total, 8);
        assert_eq!(report.passed, 8);
        assert_eq!(report.failed, 0);
        assert_eq!(report.pass_rate_basis_points, 10_000);
        assert!(report.categories[&AgentEvaluationCategory::Recovery].evaluated);
        assert!(report.categories[&AgentEvaluationCategory::SafeExecution].evaluated);
        assert!(!report.categories[&AgentEvaluationCategory::Planning].evaluated);
        assert_eq!(
            report.categories[&AgentEvaluationCategory::Planning].pass_rate_basis_points,
            None
        );
        assert!(!report.categories[&AgentEvaluationCategory::EvidenceQuality].evaluated);
    }

    #[test]
    fn changed_expectation_produces_deterministic_secret_free_failure() {
        let mut corpus = embedded_agent_evaluation_corpus().expect("embedded corpus parses");
        let AgentEvaluationScenario::RuntimeRecovery {
            expected, error, ..
        } = &mut corpus.fixtures[0].scenario;
        *error = "timeout token=private-value".to_string();
        expected.action = RuntimeRecoveryAction::StopFailed;
        let report = evaluate_agent_corpus(&corpus).expect("corpus evaluates");
        assert_eq!(report.failed, 1);
        assert_eq!(report.failures[0].mismatches, ["action"]);
        let encoded = serde_json::to_string(&report).expect("report serializes");
        assert!(!encoded.contains("private-value"));
        assert!(!encoded.contains("token="));
    }

    #[test]
    fn malformed_or_duplicate_fixtures_fail_closed() {
        let mut corpus = embedded_agent_evaluation_corpus().expect("embedded corpus parses");
        corpus.fixtures[1].fixture_id = corpus.fixtures[0].fixture_id.clone();
        assert!(evaluate_agent_corpus(&corpus)
            .expect_err("duplicate fixture rejected")
            .contains("unique"));
        assert!(parse_agent_evaluation_corpus(r#"{"schema_version":2}"#).is_err());
    }
}
