//! Safe, read-only context composition projection for developer inspection.
//!
//! This module intentionally omits all prompt and snapshot contents. It projects only durable,
//! allowlisted metadata and labels measurements that Spacesly does not own as unavailable.

use crate::application::agent_task_executor::execution_contract_digest;
use crate::domain::execution::ExecutionRun;
use crate::domain::governance::{
    GovernanceResolutionRecord, GovernanceResolutionStatus, RuleScope,
};
use crate::domain::task_session::{
    TaskMcpConnectorContext, TaskMcpContext, TaskSessionEnvelope, TaskSessionId,
    TaskSessionInputV2, TaskSessionKind, TaskSessionSnapshot, TaskSessionState,
};
use serde::Serialize;
use serde_json::Value;

pub const CONTEXT_INSPECTION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextInspectionStatus {
    Partial,
    LegacyUnavailable,
    Corrupt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMeasurement {
    CharsDiv4Estimate,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextContributionKind {
    SystemInstructions,
    Rules,
    Skills,
    Task,
    Workspace,
    External,
    ToolDefinitions,
    Conversation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ContextContribution {
    pub kind: ContextContributionKind,
    pub source: String,
    pub revision: Option<String>,
    pub digest: Option<String>,
    pub stored_content_bytes: Option<u64>,
    pub estimated_tokens: Option<u64>,
    pub token_measurement: ContextMeasurement,
    pub item_count: Option<u64>,
    pub note: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SafeRulesInspection {
    pub status: String,
    pub normalization_version: Option<String>,
    pub final_digest: Option<String>,
    pub entries: Vec<SafeRuleInspectionEntry>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SafeSkillsInspection {
    pub status: String,
    pub catalog_revision: Option<String>,
    pub selected_skill_ids: Vec<String>,
    pub entries: Vec<SafeSkillInspectionEntry>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SafeRuleInspectionEntry {
    pub rule_id: String,
    pub scope: RuleScope,
    pub source: String,
    pub revision: String,
    pub precedence: u32,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SafeSkillInspectionEntry {
    pub skill_id: String,
    pub selected: bool,
    pub trigger: String,
    pub matched_domains: Vec<String>,
    pub matched_intents: Vec<String>,
    pub priority: u8,
    pub reason: String,
    pub selection_order: Option<u32>,
}

struct SafeGovernanceProjection {
    rules: SafeRulesInspection,
    skills: SafeSkillsInspection,
    rules_size: Option<(u64, u64)>,
    skills_size: Option<(u64, u64)>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ContextIdentity {
    pub kind: Option<TaskSessionKind>,
    pub state: TaskSessionState,
    pub workspace_id: Option<String>,
    pub subject_id: Option<String>,
    pub conversation_id: Option<String>,
    pub execution_run_id: Option<String>,
    pub runtime_profile_id: Option<String>,
    pub model: Option<String>,
    pub prompt_template_version: Option<String>,
    pub context_digest: Option<String>,
    pub context_revision: Option<String>,
    pub rules_revision: Option<String>,
    pub skills_revision: Option<String>,
    pub opencode_session_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskContextInspection {
    pub schema_version: u32,
    pub task_session_id: TaskSessionId,
    pub status: ContextInspectionStatus,
    pub identity: ContextIdentity,
    pub known_stored_content_bytes: u64,
    pub known_estimated_tokens: u64,
    pub total_is_partial: bool,
    pub contributions: Vec<ContextContribution>,
    pub rules: SafeRulesInspection,
    pub skills: SafeSkillsInspection,
    pub connectors: Vec<TaskMcpConnectorContext>,
    pub unknown_fields: Vec<String>,
}

/// Builds an omission-by-construction projection. `envelope` may be unavailable for legacy rows or
/// corrupt for rows that claim a versioned schema; neither case causes raw decoder text to escape.
pub fn inspect_task_context(
    snapshot: &TaskSessionSnapshot,
    envelope: Result<Option<TaskSessionEnvelope>, String>,
    governance: Option<&GovernanceResolutionRecord>,
    mcp: Option<&TaskMcpContext>,
    execution_run: Option<&ExecutionRun>,
) -> TaskContextInspection {
    let (status, envelope) = match envelope {
        Ok(Some(envelope)) => (ContextInspectionStatus::Partial, Some(envelope)),
        Ok(None) => (ContextInspectionStatus::LegacyUnavailable, None),
        Err(_) => (ContextInspectionStatus::Corrupt, None),
    };
    let common = envelope.as_ref().map(TaskSessionEnvelope::session);
    let mut unknown_fields = Vec::new();
    if envelope.is_none() {
        unknown_fields.push(
            match status {
                ContextInspectionStatus::LegacyUnavailable => "versioned_envelope_unavailable",
                ContextInspectionStatus::Corrupt => "versioned_envelope_corrupt",
                ContextInspectionStatus::Partial => unreachable!(),
            }
            .to_string(),
        );
    }

    let safe_governance = safe_governance(governance);
    if governance.is_none() {
        unknown_fields.push("governance_not_resolved".to_string());
    }

    let mut contributions = vec![unknown_contribution(
        ContextContributionKind::SystemInstructions,
        "spacesly_prompt_template",
        common.map(|value| value.prompt_template_version.clone()),
        "Historical system-instruction content was not captured.",
    )];
    contributions.push(measured_or_unknown(
        ContextContributionKind::Rules,
        "scheduler_task_governance",
        common.and_then(|value| value.rules_revision.clone()),
        governance.map(|value| value.rules.final_digest.clone()),
        safe_governance.rules_size,
        governance.map(|value| value.rules.entries.len() as u64),
        "Exact durable Rules resolution; content is intentionally hidden.",
    ));
    contributions.push(measured_or_unknown(
        ContextContributionKind::Skills,
        "scheduler_task_governance",
        common.and_then(|value| value.skills_revision.clone()),
        governance.and_then(|value| value.skills.catalog_revision.clone()),
        safe_governance.skills_size,
        governance.map(|value| value.skills.selected_skill_ids.len() as u64),
        "Exact durable Skill decisions; instructions are intentionally hidden.",
    ));
    contributions.push(task_contribution(
        envelope.as_ref(),
        execution_run,
        &mut unknown_fields,
    ));
    contributions.push(unknown_contribution(
        ContextContributionKind::Workspace,
        "workspace_reference",
        common.and_then(|value| value.context_revision.clone()),
        "Workspace content is loaded on demand and was not captured as prompt metadata.",
    ));
    contributions.push(unknown_contribution(
        ContextContributionKind::External,
        "connector_references",
        None,
        "External context may be embedded in task context; no separate attribution was captured.",
    ));
    contributions.push(unknown_contribution(
        ContextContributionKind::ToolDefinitions,
        "opencode_runtime",
        None,
        "OpenCode owns dynamic tool definitions; Spacesly has no durable size capture.",
    ));
    contributions.push(unknown_contribution(
        ContextContributionKind::Conversation,
        "opencode_runtime",
        None,
        "OpenCode retained conversation context was not captured by Spacesly.",
    ));
    unknown_fields.extend([
        "system_instruction_tokens".to_string(),
        "workspace_context_tokens".to_string(),
        "external_context_tokens".to_string(),
        "tool_definition_tokens".to_string(),
        "conversation_context_tokens".to_string(),
    ]);

    let known_stored_content_bytes = contributions
        .iter()
        .filter_map(|item| item.stored_content_bytes)
        .sum();
    let known_estimated_tokens = contributions
        .iter()
        .filter_map(|item| item.estimated_tokens)
        .sum();

    TaskContextInspection {
        schema_version: CONTEXT_INSPECTION_SCHEMA_VERSION,
        task_session_id: snapshot.id,
        status,
        identity: ContextIdentity {
            kind: common.map(|value| value.kind),
            state: snapshot.state,
            workspace_id: common.map(|value| bounded(&value.workspace_id)),
            subject_id: common.and_then(|value| value.subject_id.as_deref().map(bounded)),
            conversation_id: common.and_then(|value| value.conversation_id.as_deref().map(bounded)),
            execution_run_id: common
                .and_then(|value| value.execution_run_id.as_deref().map(bounded)),
            runtime_profile_id: common.map(|value| bounded(&value.runtime_profile_id)),
            model: common.map(|value| bounded(&value.model)),
            prompt_template_version: common.map(|value| bounded(&value.prompt_template_version)),
            context_digest: common.map(|value| bounded(&value.context_digest)),
            context_revision: common
                .and_then(|value| value.context_revision.as_deref().map(bounded)),
            rules_revision: common.and_then(|value| value.rules_revision.as_deref().map(bounded)),
            skills_revision: common.and_then(|value| value.skills_revision.as_deref().map(bounded)),
            opencode_session_id: snapshot.opencode_session_id.as_deref().map(bounded),
        },
        known_stored_content_bytes,
        known_estimated_tokens,
        total_is_partial: true,
        contributions,
        rules: safe_governance.rules,
        skills: safe_governance.skills,
        connectors: mcp
            .map(|value| value.connectors.clone())
            .unwrap_or_default(),
        unknown_fields,
    }
}

fn safe_governance(governance: Option<&GovernanceResolutionRecord>) -> SafeGovernanceProjection {
    let Some(record) = governance else {
        return SafeGovernanceProjection {
            rules: SafeRulesInspection {
                status: "not_resolved".to_string(),
                normalization_version: None,
                final_digest: None,
                entries: Vec::new(),
                truncated: false,
            },
            skills: SafeSkillsInspection {
                status: "not_resolved".to_string(),
                catalog_revision: None,
                selected_skill_ids: Vec::new(),
                entries: Vec::new(),
                truncated: false,
            },
            rules_size: None,
            skills_size: None,
        };
    };
    let status = match record.status {
        GovernanceResolutionStatus::Authoritative => "authoritative",
        GovernanceResolutionStatus::LegacyUnavailable => "legacy_unavailable",
    }
    .to_string();
    const MAX_ENTRIES: usize = 64;
    let rules_truncated = record.rules.entries.len() > MAX_ENTRIES;
    let skills_truncated = record.skills.entries.len() > MAX_ENTRIES;
    let rules_size = text_size(&record.rules.snapshot);
    let skills_size = text_size(&record.skills.snapshot);
    SafeGovernanceProjection {
        rules: SafeRulesInspection {
            status: status.clone(),
            normalization_version: Some(bounded(&record.rules.normalization_version)),
            final_digest: Some(bounded(&record.rules.final_digest)),
            entries: record
                .rules
                .entries
                .iter()
                .take(MAX_ENTRIES)
                .map(|entry| SafeRuleInspectionEntry {
                    rule_id: bounded(&entry.rule_id),
                    scope: entry.scope.clone(),
                    source: bounded(&entry.source),
                    revision: bounded(&entry.revision),
                    precedence: entry.precedence,
                    digest: bounded(&entry.digest),
                })
                .collect(),
            truncated: rules_truncated,
        },
        skills: SafeSkillsInspection {
            status,
            catalog_revision: record.skills.catalog_revision.as_deref().map(bounded),
            selected_skill_ids: record
                .skills
                .selected_skill_ids
                .iter()
                .take(MAX_ENTRIES)
                .map(|value| bounded(value))
                .collect(),
            entries: record
                .skills
                .entries
                .iter()
                .take(MAX_ENTRIES)
                .map(|entry| SafeSkillInspectionEntry {
                    skill_id: bounded(&entry.skill_id),
                    selected: entry.selected,
                    trigger: bounded(&entry.trigger),
                    matched_domains: bounded_list(&entry.matched_domains),
                    matched_intents: bounded_list(&entry.matched_intents),
                    priority: entry.priority,
                    reason: bounded(&entry.reason),
                    selection_order: entry.selection_order,
                })
                .collect(),
            truncated: skills_truncated,
        },
        rules_size: Some(rules_size),
        skills_size: Some(skills_size),
    }
}

fn task_contribution(
    envelope: Option<&TaskSessionEnvelope>,
    execution_run: Option<&ExecutionRun>,
    unknown_fields: &mut Vec<String>,
) -> ContextContribution {
    let Some(envelope) = envelope else {
        return unknown_contribution(
            ContextContributionKind::Task,
            "task_session_envelope",
            None,
            "Task context is unavailable for this legacy or corrupt envelope.",
        );
    };
    let common = envelope.session();
    match envelope {
        TaskSessionEnvelope::V1(_) if common.kind == TaskSessionKind::Agent => {
            let Some(run) = execution_run else {
                unknown_fields.push("execution_contract_unavailable".to_string());
                return unknown_contribution(
                    ContextContributionKind::Task,
                    "execution_contract",
                    Some(common.context_digest.clone()),
                    "The referenced execution contract is unavailable.",
                );
            };
            if execution_contract_digest(&run.contract).ok().as_deref()
                != Some(common.context_digest.as_str())
            {
                unknown_fields.push("execution_contract_digest_mismatch".to_string());
                return unknown_contribution(
                    ContextContributionKind::Task,
                    "execution_contract",
                    Some(common.context_digest.clone()),
                    "The stored execution contract did not match the immutable envelope digest.",
                );
            }
            let rendered = sanitized_contract(&run.contract);
            measured_contribution(
                ContextContributionKind::Task,
                "execution_contract",
                common.context_revision.clone(),
                Some(common.context_digest.clone()),
                text_size(&rendered),
                Some(run.contract.as_object().map_or(0, |value| value.len()) as u64),
                "Sanitized contract serialization size; raw contract content is hidden.",
            )
        }
        TaskSessionEnvelope::V2(value) => {
            let (bytes, chars, count, note) = match &value.prompt_input {
                TaskSessionInputV2::Chat(input) => (
                    input.message.len()
                        + input.terminal_context.as_ref().map_or(0, String::len)
                        + input.session_context.as_ref().map_or(0, String::len),
                    input.message.chars().count()
                        + input.terminal_context.as_ref().map_or(0, |value| value.chars().count())
                        + input.session_context.as_ref().map_or(0, |value| value.chars().count()),
                    1,
                    "Immutable Chat input source bytes; retained conversation context is separate and unknown.",
                ),
                TaskSessionInputV2::Edit(input) => (
                    input.instruction.len()
                        + input.content.len()
                        + input.selection.as_ref().map_or(0, |value| value.text.len())
                        + input.context_files.iter().map(|value| value.content.len()).sum::<usize>()
                        + input.diagnostics.iter().map(String::len).sum::<usize>(),
                    input.instruction.chars().count()
                        + input.content.chars().count()
                        + input.selection.as_ref().map_or(0, |value| value.text.chars().count())
                        + input.context_files.iter().map(|value| value.content.chars().count()).sum::<usize>()
                        + input.diagnostics.iter().map(|value| value.chars().count()).sum::<usize>(),
                    1 + input.context_files.len() as u64,
                    "Immutable Edit input source bytes; file paths and content are hidden.",
                ),
            };
            measured_contribution(
                ContextContributionKind::Task,
                "task_session_prompt_input",
                common.context_revision.clone(),
                Some(common.context_digest.clone()),
                (bytes as u64, chars.div_ceil(4) as u64),
                Some(count),
                note,
            )
        }
        _ => unknown_contribution(
            ContextContributionKind::Task,
            "task_session_envelope",
            common.context_revision.clone(),
            "This legacy envelope does not contain safely attributable task-context metadata.",
        ),
    }
}

fn sanitized_contract(contract: &Value) -> String {
    let mut value = contract.clone();
    if let Some(inputs) = value
        .get_mut("runtime_inputs")
        .and_then(Value::as_object_mut)
    {
        inputs.remove("agent_rules_snapshot");
        inputs.remove("selected_skills_snapshot");
    }
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
}

fn text_size(value: &str) -> (u64, u64) {
    (value.len() as u64, value.chars().count().div_ceil(4) as u64)
}

fn measured_or_unknown(
    kind: ContextContributionKind,
    source: &str,
    revision: Option<String>,
    digest: Option<String>,
    size: Option<(u64, u64)>,
    item_count: Option<u64>,
    note: &str,
) -> ContextContribution {
    match size {
        Some(size) => measured_contribution(kind, source, revision, digest, size, item_count, note),
        None => unknown_contribution(kind, source, revision, note),
    }
}

fn measured_contribution(
    kind: ContextContributionKind,
    source: &str,
    revision: Option<String>,
    digest: Option<String>,
    size: (u64, u64),
    item_count: Option<u64>,
    note: &str,
) -> ContextContribution {
    ContextContribution {
        kind,
        source: source.to_string(),
        revision: revision.map(|value| bounded(&value)),
        digest: digest.map(|value| bounded(&value)),
        stored_content_bytes: Some(size.0),
        estimated_tokens: Some(size.1),
        token_measurement: ContextMeasurement::CharsDiv4Estimate,
        item_count,
        note: note.to_string(),
    }
}

fn unknown_contribution(
    kind: ContextContributionKind,
    source: &str,
    revision: Option<String>,
    note: &str,
) -> ContextContribution {
    ContextContribution {
        kind,
        source: source.to_string(),
        revision: revision.map(|value| bounded(&value)),
        digest: None,
        stored_content_bytes: None,
        estimated_tokens: None,
        token_measurement: ContextMeasurement::Unavailable,
        item_count: None,
        note: note.to_string(),
    }
}

fn bounded(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect()
}

fn bounded_list(values: &[String]) -> Vec<String> {
    values.iter().take(16).map(|value| bounded(value)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::governance::{
        RuleResolutionEntry, RuleScope, RulesResolutionRecord, SkillResolutionEntry,
        SkillResolutionRecord, GOVERNANCE_RESOLUTION_VERSION,
    };
    use crate::domain::task_session::{TaskRequest, TaskSessionEnvelopeV1};
    use std::collections::BTreeMap;

    fn snapshot(envelope: &TaskSessionEnvelope) -> TaskSessionSnapshot {
        TaskSessionSnapshot {
            id: TaskSessionId(7),
            request: TaskRequest::from_envelope("safe", envelope).unwrap(),
            state: TaskSessionState::Succeeded,
            worker_id: Some(1),
            dispatch_sequence: Some(1),
            attempt: 1,
            attempt_id: Some(2),
            fencing_token: 3,
            opencode_session_id: Some("session-safe".to_string()),
            lease_expires_at: None,
            progress: None,
            last_event_sequence: 4,
            error: None,
            created_at: 1,
            started_at: Some(2),
            completed_at: Some(3),
        }
    }

    #[test]
    fn inspector_omits_raw_governance_and_contract_content() {
        let secret = "SENTINEL_PRIVATE_PROMPT";
        let contract = serde_json::json!({"objective": secret, "runtime_inputs": {"agent_rules_snapshot": secret}});
        let digest = execution_contract_digest(&contract).unwrap();
        let envelope = TaskSessionEnvelope::V1(TaskSessionEnvelopeV1 {
            workspace_id: "workspace-safe".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: Some("task-safe".to_string()),
            conversation_id: Some("conversation-safe".to_string()),
            execution_run_id: Some("run-safe".to_string()),
            context_digest: digest,
            runtime_profile_id: "runtime-safe".to_string(),
            model: "model-safe".to_string(),
            connector_ids: vec![],
            requested_capabilities: vec![],
            prompt_template_version: "prompt-v1".to_string(),
            context_revision: Some("context-v1".to_string()),
            rules_revision: Some("rules-v1".to_string()),
            skills_revision: Some("skills-v1".to_string()),
        });
        let governance = GovernanceResolutionRecord {
            schema_version: GOVERNANCE_RESOLUTION_VERSION,
            task_session_id: 7,
            resolved_at: 2,
            status: GovernanceResolutionStatus::Authoritative,
            rules: RulesResolutionRecord {
                normalization_version: "v1".to_string(),
                final_digest: "safe-rule-digest".to_string(),
                entries: vec![RuleResolutionEntry {
                    rule_id: "platform".to_string(),
                    scope: RuleScope::Platform,
                    source: "platform".to_string(),
                    revision: "1".to_string(),
                    precedence: 0,
                    digest: "safe-entry-digest".to_string(),
                }],
                snapshot: secret.to_string(),
            },
            skills: SkillResolutionRecord {
                catalog_revision: Some("catalog-v1".to_string()),
                selected_skill_ids: vec!["diagnostics".to_string()],
                entries: vec![SkillResolutionEntry {
                    skill_id: "diagnostics".to_string(),
                    selected: true,
                    trigger: "contextual".to_string(),
                    matched_domains: vec!["rust".to_string()],
                    matched_intents: vec!["lint".to_string()],
                    priority: 2,
                    reason: "domain and intent matched".to_string(),
                    selection_order: Some(0),
                }],
                snapshot: secret.to_string(),
            },
        };
        let run = ExecutionRun {
            run_id: "run-safe".to_string(),
            contract,
            status: "completed".to_string(),
            current_step_ids: vec![],
            step_runs: BTreeMap::new(),
            started_at: "now".to_string(),
            completed_at: None,
            revision: 0,
        };
        let snapshot = snapshot(&envelope);
        let inspection = inspect_task_context(
            &snapshot,
            Ok(Some(envelope)),
            Some(&governance),
            None,
            Some(&run),
        );
        let encoded = serde_json::to_string(&inspection).unwrap();
        assert!(!encoded.contains(secret));
        assert!(!encoded.contains("snapshot"));
        assert_eq!(
            inspection.skills.entries[0].reason,
            "domain and intent matched"
        );
        assert_eq!(inspection.rules.entries[0].precedence, 0);
        assert!(inspection.known_estimated_tokens > 0);
        assert!(inspection.total_is_partial);
    }

    #[test]
    fn legacy_and_corrupt_envelopes_are_safe_typed_results() {
        let mut legacy = snapshot(&TaskSessionEnvelope::V1(TaskSessionEnvelopeV1 {
            workspace_id: "w".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: None,
            conversation_id: None,
            execution_run_id: None,
            context_digest: "d".to_string(),
            runtime_profile_id: "r".to_string(),
            model: "m".to_string(),
            connector_ids: vec![],
            requested_capabilities: vec![],
            prompt_template_version: "p".to_string(),
            context_revision: None,
            rules_revision: None,
            skills_revision: None,
        }));
        legacy.request.payload.clear();
        let unavailable = inspect_task_context(&legacy, Ok(None), None, None, None);
        assert_eq!(
            unavailable.status,
            ContextInspectionStatus::LegacyUnavailable
        );
        let corrupt = inspect_task_context(
            &legacy,
            Err("secret decoder detail".into()),
            None,
            None,
            None,
        );
        assert_eq!(corrupt.status, ContextInspectionStatus::Corrupt);
        assert!(!serde_json::to_string(&corrupt)
            .unwrap()
            .contains("secret decoder detail"));
    }
}
