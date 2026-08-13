//! Durable, backend-authoritative Rules and Skills resolution for Agent Task Sessions.

use crate::infrastructure::runtime_profile_store::{content_revision, AgentRuntimeProfile};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub const GOVERNANCE_RESOLUTION_VERSION: u32 = 1;
pub const RULES_NORMALIZATION_VERSION: &str = "agent-rules-lines-v1";
pub const RULE_FACTS_SCHEMA_VERSION: u32 = 7;
pub const RULE_FACTS_COMPILER_VERSION: &str = "agent-rule-facts-v7";
const RECENT_RULE_FACTS_SCHEMA_VERSION: u32 = 6;
const RECENT_RULE_FACTS_COMPILER_VERSION: &str = "agent-rule-facts-v6";
const PRIOR_RULE_FACTS_SCHEMA_VERSION: u32 = 5;
const PRIOR_RULE_FACTS_COMPILER_VERSION: &str = "agent-rule-facts-v5";
const PREVIOUS_RULE_FACTS_SCHEMA_VERSION: u32 = 4;
const PREVIOUS_RULE_FACTS_COMPILER_VERSION: &str = "agent-rule-facts-v4";
const OLDER_RULE_FACTS_SCHEMA_VERSION: u32 = 3;
const OLDER_RULE_FACTS_COMPILER_VERSION: &str = "agent-rule-facts-v3";
const ANCIENT_RULE_FACTS_SCHEMA_VERSION: u32 = 2;
const ANCIENT_RULE_FACTS_COMPILER_VERSION: &str = "agent-rule-facts-v2";
const LEGACY_RULE_FACTS_SCHEMA_VERSION: u32 = 1;
const LEGACY_RULE_FACTS_COMPILER_VERSION: &str = "agent-rule-facts-v1";
const MAX_SELECTED_SKILLS: usize = 16;
const MAX_SELECTED_SKILL_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSkillDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    #[serde(default)]
    pub custom_category: String,
    pub trigger: String,
    pub priority: u8,
    pub enabled: bool,
    pub instructions: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceResolutionStatus {
    Authoritative,
    LegacyUnavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillResolutionEntry {
    pub skill_id: String,
    pub selected: bool,
    pub trigger: String,
    pub matched_domains: Vec<String>,
    pub matched_intents: Vec<String>,
    pub priority: u8,
    pub reason: String,
    pub selection_order: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillResolutionRecord {
    pub catalog_revision: Option<String>,
    pub selected_skill_ids: Vec<String>,
    pub entries: Vec<SkillResolutionEntry>,
    /// Exact selected instructions injected into the runtime. Never emitted in metrics.
    pub snapshot: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleScope {
    Platform,
    Global,
    Workspace,
    Task,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuleResolutionEntry {
    pub rule_id: String,
    pub scope: RuleScope,
    pub source: String,
    pub revision: String,
    pub precedence: u32,
    pub digest: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryRuleFact {
    pub id: String,
    pub remote_url: String,
    pub local_path: Option<String>,
    pub backend_path: Option<String>,
    pub frontend_path: Option<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_line: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProtectedBranchRulePolicy {
    pub branches: Vec<String>,
    pub operations: Vec<String>,
    pub approval_required: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeploymentTargetRuleFact {
    pub label: String,
    pub target: String,
    pub branch: String,
    pub namespace: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_line: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectorRuleFact {
    pub id: String,
    pub connector_type: String,
    pub base_url: String,
    pub required_operations: Vec<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_line: u32,
}

/// User-authored evidence requirements for accepting a connector-backed task as complete.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerificationRuleFact {
    pub id: String,
    pub connector_id: String,
    #[serde(default)]
    pub applies_to_labels: Vec<String>,
    pub required_operations: Vec<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_line: u32,
}

/// User-authored terminal state checks executed independently of the model.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvidenceVerifierRuleFact {
    pub id: String,
    pub provider: String,
    #[serde(default)]
    pub connector_id: Option<String>,
    #[serde(default)]
    pub read_operation: Option<String>,
    #[serde(default)]
    pub expected_status: Option<String>,
    #[serde(default)]
    pub applies_to_labels: Vec<String>,
    pub required_states: Vec<String>,
    #[serde(default)]
    pub poll_interval_seconds: Option<u64>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polling_configuration_error: Option<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_line: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuleFactsRecord {
    pub schema_version: u32,
    pub compiler_version: String,
    pub source_digest: String,
    pub repositories: Vec<RepositoryRuleFact>,
    pub protected_branches: Vec<ProtectedBranchRulePolicy>,
    pub deployment_targets: Vec<DeploymentTargetRuleFact>,
    #[serde(default)]
    pub connectors: Vec<ConnectorRuleFact>,
    #[serde(default)]
    pub verification_policies: Vec<VerificationRuleFact>,
    #[serde(default)]
    pub evidence_verifiers: Vec<EvidenceVerifierRuleFact>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RulesResolutionRecord {
    pub normalization_version: String,
    pub final_digest: String,
    pub entries: Vec<RuleResolutionEntry>,
    #[serde(default)]
    pub facts: RuleFactsRecord,
    /// Exact normalized user Rules injected below platform instructions.
    pub snapshot: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GovernanceResolutionRecord {
    pub schema_version: u32,
    pub task_session_id: u64,
    pub resolved_at: u64,
    pub status: GovernanceResolutionStatus,
    pub rules: RulesResolutionRecord,
    pub skills: SkillResolutionRecord,
}

impl GovernanceResolutionRecord {
    pub fn validate_for(&self, task_session_id: u64) -> Result<(), String> {
        if self.task_session_id != task_session_id {
            return Err("Governance snapshot belongs to a different Task Session.".to_string());
        }
        if self.schema_version != GOVERNANCE_RESOLUTION_VERSION {
            return Err("Governance snapshot schema is not supported.".to_string());
        }
        if self.rules.final_digest != content_revision(&self.rules.snapshot) {
            return Err("Governance Rules snapshot digest is invalid.".to_string());
        }
        if self.status == GovernanceResolutionStatus::Authoritative {
            // Schema 0 is the serde default for retained v1 governance snapshots created before
            // Rule facts existed. They remain valid and immutable; new resolutions always emit v1.
            if self.rules.facts.schema_version != 0 {
                let supported_compiler = (self.rules.facts.schema_version
                    == RULE_FACTS_SCHEMA_VERSION
                    && self.rules.facts.compiler_version == RULE_FACTS_COMPILER_VERSION)
                    || (self.rules.facts.schema_version == RECENT_RULE_FACTS_SCHEMA_VERSION
                        && self.rules.facts.compiler_version == RECENT_RULE_FACTS_COMPILER_VERSION)
                    || (self.rules.facts.schema_version == PRIOR_RULE_FACTS_SCHEMA_VERSION
                        && self.rules.facts.compiler_version == PRIOR_RULE_FACTS_COMPILER_VERSION)
                    || (self.rules.facts.schema_version == PREVIOUS_RULE_FACTS_SCHEMA_VERSION
                        && self.rules.facts.compiler_version
                            == PREVIOUS_RULE_FACTS_COMPILER_VERSION)
                    || (self.rules.facts.schema_version == OLDER_RULE_FACTS_SCHEMA_VERSION
                        && self.rules.facts.compiler_version == OLDER_RULE_FACTS_COMPILER_VERSION)
                    || (self.rules.facts.schema_version == ANCIENT_RULE_FACTS_SCHEMA_VERSION
                        && self.rules.facts.compiler_version
                            == ANCIENT_RULE_FACTS_COMPILER_VERSION)
                    || (self.rules.facts.schema_version == LEGACY_RULE_FACTS_SCHEMA_VERSION
                        && self.rules.facts.compiler_version == LEGACY_RULE_FACTS_COMPILER_VERSION);
                if !supported_compiler || self.rules.facts.source_digest != self.rules.final_digest
                {
                    return Err("Governance Rule facts are stale or unsupported.".to_string());
                }
            }
            if self.rules.facts.repositories.len() > 32
                || self.rules.facts.protected_branches.len() > 32
                || self.rules.facts.deployment_targets.len() > 64
                || self.rules.facts.connectors.len() > 32
                || self.rules.facts.verification_policies.len() > 64
                || self.rules.facts.evidence_verifiers.len() > 64
                || self.rules.facts.warnings.len() > 32
            {
                return Err("Governance Rule facts exceed bounded limits.".to_string());
            }
        }
        if self.status == GovernanceResolutionStatus::Authoritative
            && self.rules.normalization_version != RULES_NORMALIZATION_VERSION
        {
            return Err("Governance snapshot normalization version is not supported.".to_string());
        }
        let mut selected = self
            .skills
            .entries
            .iter()
            .filter(|entry| entry.selected)
            .collect::<Vec<_>>();
        selected.sort_by_key(|entry| entry.selection_order);
        if self.status == GovernanceResolutionStatus::Authoritative
            && (selected.len() != self.skills.selected_skill_ids.len()
                || selected.iter().enumerate().any(|(index, entry)| {
                    entry.selection_order != Some(index as u32)
                        || self.skills.selected_skill_ids[index] != entry.skill_id
                }))
        {
            return Err("Governance selected Skill ordering is inconsistent.".to_string());
        }
        Ok(())
    }
}

pub fn skill_catalog_revision(catalog: &[AgentSkillDefinition]) -> Result<String, String> {
    validate_catalog(catalog)?;
    serde_json::to_string(catalog)
        .map(|encoded| content_revision(&encoded))
        .map_err(|error| format!("Failed to encode Agent Skill catalog: {error}"))
}

pub fn resolve_governance(
    task_session_id: u64,
    profile: &AgentRuntimeProfile,
    contract: &Value,
) -> Result<GovernanceResolutionRecord, String> {
    if profile.governance_schema_version == 0 {
        return Ok(legacy_resolution(task_session_id, profile, contract));
    }
    if profile.governance_schema_version != GOVERNANCE_RESOLUTION_VERSION {
        return Err(format!(
            "Agent governance schema version {} is not supported.",
            profile.governance_schema_version
        ));
    }
    if contract
        .pointer("/runtime_inputs/selected_skills_snapshot")
        .is_some()
        || contract
            .pointer("/runtime_inputs/agent_rules_snapshot")
            .is_some()
    {
        return Err(
            "Legacy renderer governance snapshots cannot be submitted with backend-authoritative governance."
                .to_string(),
        );
    }
    let rules_started = std::time::Instant::now();
    let rules = resolve_rules(profile)?;
    crate::infrastructure::performance::record_duration_with_context(
        "rules_resolution_latency_ms",
        "agent_runtime",
        rules_started.elapsed(),
        BTreeMap::from([
            ("task_session_id".to_string(), task_session_id.to_string()),
            ("rules_revision".to_string(), profile.rules_revision.clone()),
            (
                "resolved_rule_count".to_string(),
                rules.entries.len().to_string(),
            ),
            (
                "rules_prompt_bytes".to_string(),
                rules.snapshot.len().to_string(),
            ),
            (
                "rules_estimated_prompt_tokens".to_string(),
                estimated_tokens(&rules.snapshot).to_string(),
            ),
        ]),
    );
    let skills_started = std::time::Instant::now();
    let skills = resolve_skills(profile, contract)?;
    crate::infrastructure::performance::record_duration_with_context(
        "skill_resolution_latency_ms",
        "agent_runtime",
        skills_started.elapsed(),
        BTreeMap::from([
            ("task_session_id".to_string(), task_session_id.to_string()),
            (
                "skill_catalog_revision".to_string(),
                profile.skills_revision.clone(),
            ),
            (
                "selected_skill_count".to_string(),
                skills.selected_skill_ids.len().to_string(),
            ),
            (
                "resolved_rule_count".to_string(),
                rules.entries.len().to_string(),
            ),
            (
                "skills_prompt_bytes".to_string(),
                skills.snapshot.len().to_string(),
            ),
            (
                "skills_estimated_prompt_tokens".to_string(),
                estimated_tokens(&skills.snapshot).to_string(),
            ),
        ]),
    );
    Ok(GovernanceResolutionRecord {
        schema_version: GOVERNANCE_RESOLUTION_VERSION,
        task_session_id,
        resolved_at: now_millis()?,
        status: GovernanceResolutionStatus::Authoritative,
        rules,
        skills,
    })
}

fn resolve_rules(profile: &AgentRuntimeProfile) -> Result<RulesResolutionRecord, String> {
    let snapshot = normalize_rules(&profile.agent_rules)?;
    let final_digest = content_revision(&snapshot);
    if final_digest != profile.rules_revision {
        return Err(
            "Agent Rules revision does not match backend normalization version agent-rules-lines-v1."
                .to_string(),
        );
    }
    let mut entries = vec![RuleResolutionEntry {
        rule_id: "platform.agent_worker.system".to_string(),
        scope: RuleScope::Platform,
        source: "runtime.ai_worker".to_string(),
        revision: profile.prompt_template_version.clone(),
        precedence: 0,
        digest: content_revision(&format!(
            "platform-agent-worker:{}",
            profile.prompt_template_version
        )),
    }];
    if !snapshot.is_empty() {
        entries.push(RuleResolutionEntry {
            rule_id: "global.agent_rules".to_string(),
            scope: RuleScope::Global,
            source: "settings.ai_worker.agent_rules".to_string(),
            revision: profile.rules_revision.clone(),
            precedence: 1_000,
            digest: content_revision(&snapshot),
        });
    }
    entries.sort_by(|left, right| {
        left.precedence
            .cmp(&right.precedence)
            .then_with(|| left.rule_id.cmp(&right.rule_id))
    });
    Ok(RulesResolutionRecord {
        normalization_version: RULES_NORMALIZATION_VERSION.to_string(),
        final_digest,
        entries,
        facts: compile_rule_facts(&snapshot),
        snapshot,
    })
}

pub fn compile_rule_facts(snapshot: &str) -> RuleFactsRecord {
    let source_digest = content_revision(snapshot);
    let url_pattern = Regex::new(r#"https?://[^\s`<>\"']+"#).expect("valid URL regex");
    let backend_pattern = Regex::new(r"(?i)backend helm templates.*?folder\s+([^\s,]+)")
        .expect("valid backend path regex");
    let frontend_pattern = Regex::new(r"(?i)frontend helm templates.*?folder\s+([^\s,]+)")
        .expect("valid frontend path regex");
    let local_pattern = Regex::new(r"(?i)local (?:checkout|repository|repo).*?(/[^\s`]+)")
        .expect("valid local path regex");
    let protected_pattern = Regex::new(r"(?i)modify on\s+([a-z0-9_, -]+?)\s+must ask approval")
        .expect("valid protected branch regex");

    let mut remote_urls = Vec::new();
    let mut local_path = None;
    let mut backend_path = None;
    let mut frontend_path = None;
    let mut protected_branches = Vec::new();
    let mut deployment_targets = Vec::new();
    let mut in_deployment_table = false;
    let connectors = compile_connector_rule_facts(snapshot);
    let verification_policies = compile_verification_rule_facts(snapshot);
    let evidence_verifiers = compile_evidence_verifier_rule_facts(snapshot);

    for (line_index, line) in snapshot.lines().enumerate() {
        for matched in url_pattern.find_iter(line) {
            let url = matched
                .as_str()
                .trim_end_matches(|character: char| matches!(character, '.' | ',' | ')' | ']'))
                .to_string();
            if url.contains("/repos/")
                && !remote_urls
                    .iter()
                    .any(|(candidate, _): &(String, u32)| candidate == &url)
            {
                remote_urls.push((url, (line_index + 1) as u32));
            }
        }
        if local_path.is_none() {
            local_path = local_pattern
                .captures(line)
                .and_then(|captures| captures.get(1))
                .map(|value| clean_rule_path(value.as_str()));
        }
        if backend_path.is_none() {
            backend_path = backend_pattern
                .captures(line)
                .and_then(|captures| captures.get(1))
                .map(|value| clean_rule_path(value.as_str()));
        }
        if frontend_path.is_none() {
            frontend_path = frontend_pattern
                .captures(line)
                .and_then(|captures| captures.get(1))
                .map(|value| clean_rule_path(value.as_str()));
        }
        if let Some(value) = protected_pattern
            .captures(line)
            .and_then(|captures| captures.get(1))
        {
            protected_branches.extend(
                value
                    .as_str()
                    .split(',')
                    .map(|branch| branch.trim().trim_matches('`').to_lowercase())
                    .filter(|branch| !branch.is_empty()),
            );
        }
        if line.trim_start().starts_with('|') {
            let cells = line
                .split('|')
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .collect::<Vec<_>>();
            if cells.len() >= 4
                && cells[0].to_ascii_lowercase().contains("label")
                && cells[1].to_ascii_lowercase().contains("environment")
                && cells[2].to_ascii_lowercase().contains("branch")
                && cells[3].to_ascii_lowercase().contains("namespace")
            {
                in_deployment_table = true;
                continue;
            }
            let separator = cells
                .iter()
                .all(|cell| cell.chars().all(|character| matches!(character, '-' | ':')));
            if in_deployment_table
                && cells.len() >= 4
                && !separator
                && cells[..4].iter().all(|cell| !cell.trim().is_empty())
            {
                deployment_targets.push(DeploymentTargetRuleFact {
                    label: cells[0].to_string(),
                    target: cells[1].to_string(),
                    branch: cells[2].to_string(),
                    namespace: cells[3].to_string(),
                    source: "global.agent_rules".to_string(),
                    source_line: (line_index + 1) as u32,
                });
            }
        } else {
            in_deployment_table = false;
        }
    }
    protected_branches.sort();
    protected_branches.dedup();
    deployment_targets.sort_by(|left, right| {
        (&left.label, &left.target, &left.branch, &left.namespace).cmp(&(
            &right.label,
            &right.target,
            &right.branch,
            &right.namespace,
        ))
    });
    deployment_targets.dedup_by(|left, right| {
        left.label == right.label
            && left.target == right.target
            && left.branch == right.branch
            && left.namespace == right.namespace
    });

    let repositories = remote_urls
        .into_iter()
        .map(|(remote_url, source_line)| RepositoryRuleFact {
            id: remote_url
                .split("/repos/")
                .nth(1)
                .unwrap_or("repository")
                .trim_matches('/')
                .to_string(),
            remote_url,
            local_path: local_path.clone(),
            backend_path: backend_path.clone(),
            frontend_path: frontend_path.clone(),
            source: "global.agent_rules".to_string(),
            source_line,
        })
        .collect::<Vec<_>>();
    let mut warnings = Vec::new();
    if !repositories.is_empty() && local_path.is_none() {
        warnings.push(
            "Repository remote was compiled, but Rules do not declare a local checkout path."
                .to_string(),
        );
    }
    RuleFactsRecord {
        schema_version: RULE_FACTS_SCHEMA_VERSION,
        compiler_version: RULE_FACTS_COMPILER_VERSION.to_string(),
        source_digest,
        repositories,
        protected_branches: if protected_branches.is_empty() {
            Vec::new()
        } else {
            vec![ProtectedBranchRulePolicy {
                branches: protected_branches,
                operations: vec![
                    "modify".to_string(),
                    "stage".to_string(),
                    "commit".to_string(),
                    "push".to_string(),
                ],
                approval_required: true,
            }]
        },
        deployment_targets,
        connectors,
        verification_policies,
        evidence_verifiers,
        warnings,
    }
}

fn compile_evidence_verifier_rule_facts(snapshot: &str) -> Vec<EvidenceVerifierRuleFact> {
    let heading = Regex::new(r"(?i)^#{1,6}\s+evidence verifier\s*:\s*([a-z0-9_.-]+)\s*$")
        .expect("valid evidence verifier heading regex");
    let field = Regex::new(
        r"(?i)^(?:[-*]|\d+[.)])?\s*(provider|connector|read operation|expected status|applies to labels|required states|poll interval seconds|timeout seconds)\s*:\s*(.*)$",
    )
    .expect("valid evidence verifier field regex");
    let mut verifiers = Vec::new();
    let mut current: Option<EvidenceVerifierRuleFact> = None;
    let mut collecting = None::<String>;
    for (line_index, line) in snapshot.lines().enumerate() {
        if let Some(id) = heading.captures(line).and_then(|captures| captures.get(1)) {
            if let Some(verifier) = current.take() {
                verifiers.push(verifier);
            }
            current = Some(EvidenceVerifierRuleFact {
                id: id.as_str().to_ascii_lowercase(),
                source: "global.agent_rules".to_string(),
                source_line: (line_index + 1) as u32,
                ..Default::default()
            });
            collecting = None;
            continue;
        }
        let Some(verifier) = current.as_mut() else {
            continue;
        };
        if line.starts_with('#') {
            verifiers.push(current.take().expect("evidence verifier exists"));
            collecting = None;
            continue;
        }
        if let Some(captures) = field.captures(line) {
            let name = captures[1].to_ascii_lowercase();
            let value = clean_rule_scalar(&captures[2]);
            collecting = matches!(name.as_str(), "applies to labels" | "required states")
                .then_some(name.clone());
            match name.as_str() {
                "provider" => verifier.provider = value.to_ascii_lowercase(),
                "connector" => verifier.connector_id = Some(value.to_ascii_lowercase()),
                "read operation" => verifier.read_operation = Some(value.to_ascii_lowercase()),
                "expected status" => verifier.expected_status = Some(value),
                "applies to labels" => verifier
                    .applies_to_labels
                    .extend(split_rule_operations(&value)),
                "required states" => verifier
                    .required_states
                    .extend(split_rule_operations(&value)),
                "poll interval seconds" => match value.parse::<u64>() {
                    Ok(value) => verifier.poll_interval_seconds = Some(value),
                    Err(_) => {
                        verifier.polling_configuration_error =
                            Some("Poll interval seconds must be an integer.".to_string())
                    }
                },
                "timeout seconds" => match value.parse::<u64>() {
                    Ok(value) => verifier.timeout_seconds = Some(value),
                    Err(_) => {
                        verifier.polling_configuration_error =
                            Some("Timeout seconds must be an integer.".to_string())
                    }
                },
                _ => unreachable!(),
            }
            continue;
        }
        if let Some(field) = collecting.as_deref() {
            let value = line
                .trim()
                .trim_start_matches(['-', '*'])
                .trim()
                .trim_matches('`');
            if !value.is_empty() && !value.contains(':') {
                match field {
                    "applies to labels" => verifier
                        .applies_to_labels
                        .extend(split_rule_operations(value)),
                    "required states" => verifier
                        .required_states
                        .extend(split_rule_operations(value)),
                    _ => unreachable!(),
                }
            }
        }
    }
    if let Some(verifier) = current {
        verifiers.push(verifier);
    }
    for verifier in &mut verifiers {
        verifier.applies_to_labels.sort();
        verifier.applies_to_labels.dedup();
        verifier.required_states.sort();
        verifier.required_states.dedup();
    }
    verifiers.sort_by(|left, right| left.id.cmp(&right.id));
    verifiers
}

fn compile_verification_rule_facts(snapshot: &str) -> Vec<VerificationRuleFact> {
    let heading = Regex::new(r"(?i)^#{1,6}\s+verification\s*:\s*([a-z0-9_.-]+)\s*$")
        .expect("valid verification heading regex");
    let field = Regex::new(
        r"(?i)^(?:[-*]|\d+[.)])?\s*(connector|applies to labels|required successful operations)\s*:\s*(.*)$",
    )
    .expect("valid verification field regex");
    let mut policies = Vec::new();
    let mut current: Option<VerificationRuleFact> = None;
    let mut collecting = None::<String>;
    for (line_index, line) in snapshot.lines().enumerate() {
        if let Some(id) = heading.captures(line).and_then(|captures| captures.get(1)) {
            if let Some(policy) = current.take() {
                policies.push(policy);
            }
            current = Some(VerificationRuleFact {
                id: id.as_str().to_ascii_lowercase(),
                source: "global.agent_rules".to_string(),
                source_line: (line_index + 1) as u32,
                ..Default::default()
            });
            collecting = None;
            continue;
        }
        let Some(policy) = current.as_mut() else {
            continue;
        };
        if line.starts_with('#') {
            policies.push(current.take().expect("verification policy exists"));
            collecting = None;
            continue;
        }
        if let Some(captures) = field.captures(line) {
            let name = captures[1].to_ascii_lowercase();
            let value = clean_rule_scalar(&captures[2]);
            collecting = matches!(
                name.as_str(),
                "applies to labels" | "required successful operations"
            )
            .then_some(name.clone());
            match name.as_str() {
                "connector" => policy.connector_id = value.to_ascii_lowercase(),
                "applies to labels" => policy
                    .applies_to_labels
                    .extend(split_rule_operations(&value)),
                "required successful operations" => policy
                    .required_operations
                    .extend(split_rule_operations(&value)),
                _ => unreachable!(),
            }
            continue;
        }
        if let Some(field) = collecting.as_deref() {
            let value = line
                .trim()
                .trim_start_matches(['-', '*'])
                .trim()
                .trim_matches('`');
            if !value.is_empty() && !value.contains(':') {
                match field {
                    "applies to labels" => policy
                        .applies_to_labels
                        .extend(split_rule_operations(value)),
                    "required successful operations" => policy
                        .required_operations
                        .extend(split_rule_operations(value)),
                    _ => unreachable!(),
                }
            }
        }
    }
    if let Some(policy) = current {
        policies.push(policy);
    }
    for policy in &mut policies {
        policy.applies_to_labels.sort();
        policy.applies_to_labels.dedup();
        policy.required_operations.sort();
        policy.required_operations.dedup();
    }
    policies.sort_by(|left, right| left.id.cmp(&right.id));
    policies
}

fn compile_connector_rule_facts(snapshot: &str) -> Vec<ConnectorRuleFact> {
    let heading = Regex::new(r"(?i)^#{1,6}\s+connector\s*:\s*([a-z0-9_.-]+)\s*$")
        .expect("valid connector heading regex");
    let field =
        Regex::new(r"(?i)^(?:[-*]|\d+[.)])?\s*(type|base url|required operations)\s*:\s*(.*)$")
            .expect("valid connector field regex");
    let mut connectors = Vec::new();
    let mut current: Option<ConnectorRuleFact> = None;
    let mut collecting_operations = false;
    for (line_index, line) in snapshot.lines().enumerate() {
        if let Some(id) = heading.captures(line).and_then(|captures| captures.get(1)) {
            if let Some(connector) = current.take() {
                connectors.push(connector);
            }
            current = Some(ConnectorRuleFact {
                id: id.as_str().to_ascii_lowercase(),
                source: "global.agent_rules".to_string(),
                source_line: (line_index + 1) as u32,
                ..Default::default()
            });
            collecting_operations = false;
            continue;
        }
        let Some(connector) = current.as_mut() else {
            continue;
        };
        if line.starts_with('#') {
            connectors.push(current.take().expect("connector exists"));
            collecting_operations = false;
            continue;
        }
        if let Some(captures) = field.captures(line) {
            let name = captures[1].to_ascii_lowercase();
            let value = clean_rule_scalar(&captures[2]);
            collecting_operations = name == "required operations";
            match name.as_str() {
                "type" => connector.connector_type = value.to_ascii_lowercase(),
                "base url" => connector.base_url = value,
                "required operations" => {
                    connector
                        .required_operations
                        .extend(split_rule_operations(&value));
                }
                _ => unreachable!(),
            }
            continue;
        }
        if collecting_operations {
            let value = line
                .trim()
                .trim_start_matches(['-', '*'])
                .trim()
                .trim_matches('`');
            if !value.is_empty() && !value.contains(':') {
                connector
                    .required_operations
                    .extend(split_rule_operations(value));
            }
        }
    }
    if let Some(connector) = current {
        connectors.push(connector);
    }
    for connector in &mut connectors {
        connector.required_operations.sort();
        connector.required_operations.dedup();
    }
    connectors.sort_by(|left, right| left.id.cmp(&right.id));
    connectors
}

fn clean_rule_scalar(value: &str) -> String {
    value
        .trim()
        .trim_matches(|character| matches!(character, '`' | '\'' | '"'))
        .trim()
        .to_string()
}

fn split_rule_operations(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(clean_rule_scalar)
        .map(|value| value.to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect()
}

fn clean_rule_path(value: &str) -> String {
    value
        .trim()
        .trim_matches(|character: char| matches!(character, '`' | '.' | ',' | ')' | ']'))
        .trim_end_matches('/')
        .to_string()
}

fn resolve_skills(
    profile: &AgentRuntimeProfile,
    contract: &Value,
) -> Result<SkillResolutionRecord, String> {
    validate_catalog(&profile.skill_catalog)?;
    let revision = skill_catalog_revision(&profile.skill_catalog)?;
    if revision != profile.skills_revision {
        return Err("Agent Skill catalog revision did not match its content.".to_string());
    }
    let contract_text = normalized_contract_text(contract);
    let category_matches = category_matches(&contract_text);
    let requested = contract
        .pointer("/runtime_inputs/requested_skill_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<HashSet<_>>();
    let mut decisions = Vec::with_capacity(profile.skill_catalog.len());
    for (catalog_index, skill) in profile.skill_catalog.iter().enumerate() {
        let mut domains = Vec::new();
        let mut intents = Vec::new();
        let (selected, reason, manual) = if !skill.enabled || skill.trigger == "disabled" {
            (false, "Skill is disabled.".to_string(), false)
        } else if skill.trigger == "automatic" {
            (true, "Automatic trigger is enabled.".to_string(), false)
        } else if skill.trigger == "manual" {
            if requested.contains(skill.id.as_str()) {
                (
                    true,
                    "Explicitly requested for this task.".to_string(),
                    true,
                )
            } else {
                (false, "Manual Skill was not requested.".to_string(), false)
            }
        } else if skill.trigger == "contextual" {
            if skill.category == "custom" {
                let term = normalize_text(&skill.custom_category);
                if !term.is_empty() && includes_phrase(&contract_text, &term) {
                    domains.push(skill.custom_category.trim().to_string());
                    intents.push(term.clone());
                    (true, format!("Matched custom category '{term}'."), false)
                } else {
                    (false, "No contextual category matched.".to_string(), false)
                }
            } else if let Some(terms) = category_matches.get(skill.category.as_str()) {
                domains.push(skill.category.clone());
                intents.extend(terms.iter().cloned());
                (
                    true,
                    format!("Matched {} task context.", skill.category),
                    false,
                )
            } else {
                (false, "No contextual category matched.".to_string(), false)
            }
        } else {
            return Err(format!(
                "Agent Skill '{}' has unsupported trigger '{}'.",
                skill.id, skill.trigger
            ));
        };
        decisions.push((
            catalog_index,
            skill,
            selected,
            manual,
            domains,
            intents,
            reason,
        ));
    }
    let mut selected = decisions.iter().filter(|entry| entry.2).collect::<Vec<_>>();
    selected.sort_by(|left, right| {
        right
            .3
            .cmp(&left.3)
            .then_with(|| right.1.priority.cmp(&left.1.priority))
            .then_with(|| left.0.cmp(&right.0))
    });
    if selected.len() > MAX_SELECTED_SKILLS {
        return Err(format!("Cannot resolve Agent Skills: {} skills matched this task, exceeding the {MAX_SELECTED_SKILLS}-skill execution limit.", selected.len()));
    }
    let mut sections = Vec::with_capacity(selected.len());
    let mut ids = Vec::with_capacity(selected.len());
    let mut orders = HashMap::new();
    let mut bytes = 0;
    for (order, entry) in selected.iter().enumerate() {
        let skill = entry.1;
        let section = format!(
            "Skill: {}\nSkill ID: {}\nCategory: {}\nDescription: {}\nInstructions:\n{}",
            skill.name.trim(),
            skill.id,
            display_category(skill),
            skill.description.trim(),
            skill.instructions.trim()
        );
        let separator = usize::from(!sections.is_empty()) * 2;
        if bytes + separator + section.len() > MAX_SELECTED_SKILL_BYTES {
            return Err(format!("Cannot resolve Agent Skills: selected Skills exceed the {} KiB prompt limit at '{}'.", MAX_SELECTED_SKILL_BYTES / 1024, skill.name));
        }
        bytes += separator + section.len();
        sections.push(section);
        ids.push(skill.id.clone());
        orders.insert(skill.id.as_str(), order as u32);
    }
    let entries = decisions
        .into_iter()
        .map(
            |(_, skill, selected, _, matched_domains, matched_intents, reason)| {
                SkillResolutionEntry {
                    skill_id: skill.id.clone(),
                    selected,
                    trigger: skill.trigger.clone(),
                    matched_domains,
                    matched_intents,
                    priority: skill.priority,
                    reason,
                    selection_order: orders.get(skill.id.as_str()).copied(),
                }
            },
        )
        .collect();
    Ok(SkillResolutionRecord {
        catalog_revision: Some(revision),
        selected_skill_ids: ids,
        entries,
        snapshot: sections.join("\n\n"),
    })
}

fn legacy_resolution(
    task_session_id: u64,
    profile: &AgentRuntimeProfile,
    contract: &Value,
) -> GovernanceResolutionRecord {
    let ids = contract
        .pointer("/runtime_inputs/selected_skill_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect();
    let rules = contract
        .pointer("/runtime_inputs/agent_rules_snapshot")
        .and_then(Value::as_str)
        .unwrap_or(&profile.agent_rules)
        .to_string();
    let skills = contract
        .pointer("/runtime_inputs/selected_skills_snapshot")
        .and_then(Value::as_str)
        .unwrap_or(&profile.agent_skills)
        .to_string();
    GovernanceResolutionRecord {
        schema_version: GOVERNANCE_RESOLUTION_VERSION,
        task_session_id,
        resolved_at: now_millis().unwrap_or_default(),
        status: GovernanceResolutionStatus::LegacyUnavailable,
        rules: RulesResolutionRecord {
            normalization_version: "legacy_unavailable".to_string(),
            final_digest: content_revision(&rules),
            entries: Vec::new(),
            facts: compile_rule_facts(&rules),
            snapshot: rules,
        },
        skills: SkillResolutionRecord {
            catalog_revision: None,
            selected_skill_ids: ids,
            entries: Vec::new(),
            snapshot: skills,
        },
    }
}

fn normalize_rules(value: &str) -> Result<String, String> {
    if value.len() > 32 * 1024 {
        return Err("Agent Rules exceed the 32 KiB execution limit.".to_string());
    }
    let mut seen = HashSet::new();
    Ok(value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| seen.insert((*line).to_string()))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn validate_catalog(catalog: &[AgentSkillDefinition]) -> Result<(), String> {
    if catalog.len() > 64 {
        return Err("Agent Skill catalog exceeds the 64-skill limit.".to_string());
    }
    let mut ids = HashSet::new();
    for skill in catalog {
        if skill.id.trim().is_empty()
            || !ids.insert(skill.id.as_str())
            || skill.name.trim().is_empty()
            || skill.description.trim().is_empty()
            || skill.instructions.trim().is_empty()
            || skill.instructions.len() > 8 * 1024
            || skill.priority > 100
            || skill.updated_at.trim().is_empty()
            || !matches!(
                skill.trigger.as_str(),
                "automatic" | "contextual" | "manual" | "disabled"
            )
            || !matches!(
                skill.category.as_str(),
                "diagnostics"
                    | "deployment"
                    | "infrastructure"
                    | "git"
                    | "coding"
                    | "testing"
                    | "security"
                    | "database"
                    | "documentation"
                    | "custom"
            )
            || (skill.category == "custom" && skill.custom_category.trim().is_empty())
        {
            return Err(format!("Agent Skill '{}' is invalid.", skill.id));
        }
    }
    Ok(())
}

fn display_category(skill: &AgentSkillDefinition) -> &str {
    if skill.category == "custom" {
        skill.custom_category.trim()
    } else {
        skill.category.as_str()
    }
}

fn normalized_contract_text(contract: &Value) -> String {
    let mut values = Vec::new();
    collect_string(contract.pointer("/objective/summary"), &mut values);
    collect_array(contract.pointer("/objective/success_criteria"), &mut values);
    collect_string(contract.pointer("/task_context/description"), &mut values);
    collect_string(
        contract.pointer("/task_context/execution_detail"),
        &mut values,
    );
    collect_string(contract.pointer("/ticket/title"), &mut values);
    collect_array(contract.pointer("/ticket/labels"), &mut values);
    collect_string(contract.pointer("/deployment/target"), &mut values);
    collect_string(contract.pointer("/deployment/workload"), &mut values);
    collect_string(contract.pointer("/build/result_key"), &mut values);
    collect_string(contract.pointer("/build/provider"), &mut values);
    if let Some(workflow) = contract.get("workflow").and_then(Value::as_array) {
        for step in workflow {
            collect_string(step.get("title"), &mut values);
            collect_string(step.get("type"), &mut values);
        }
    }
    collect_string(
        contract.pointer("/runtime_inputs/operator_notes"),
        &mut values,
    );
    normalize_text(&values.join(" "))
}

fn collect_string(value: Option<&Value>, output: &mut Vec<String>) {
    if let Some(value) = value.and_then(Value::as_str) {
        output.push(value.to_string());
    }
}
fn collect_array(value: Option<&Value>, output: &mut Vec<String>) {
    if let Some(values) = value.and_then(Value::as_array) {
        output.extend(
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string),
        );
    }
}

fn category_matches(text: &str) -> BTreeMap<&'static str, Vec<String>> {
    category_terms()
        .into_iter()
        .filter_map(|(category, terms)| {
            let matched = terms
                .into_iter()
                .filter(|term| includes_phrase(text, term))
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            (!matched.is_empty()).then_some((category, matched))
        })
        .collect()
}

fn category_terms() -> BTreeMap<&'static str, Vec<&'static str>> {
    BTreeMap::from([
        (
            "diagnostics",
            vec![
                "diagnose",
                "diagnostic",
                "troubleshoot",
                "failure",
                "failed",
                "error",
                "incident",
                "logs",
                "events",
                "crash",
                "timeout",
            ],
        ),
        (
            "deployment",
            vec![
                "deploy",
                "deployment",
                "release",
                "rollout",
                "bamboo",
                "build plan",
            ],
        ),
        (
            "infrastructure",
            vec![
                "infrastructure",
                "kubernetes",
                "openshift",
                "ocp",
                "pod",
                "namespace",
                "cluster",
                "terraform",
                "helm",
            ],
        ),
        (
            "git",
            vec![
                "git",
                "commit",
                "branch",
                "merge",
                "pull request",
                "repository",
            ],
        ),
        (
            "coding",
            vec![
                "code",
                "implement",
                "refactor",
                "function",
                "component",
                "module",
                "bug",
                "fix",
            ],
        ),
        (
            "testing",
            vec![
                "test",
                "testing",
                "lint",
                "coverage",
                "verify",
                "validation",
            ],
        ),
        (
            "security",
            vec![
                "security",
                "vulnerability",
                "credential",
                "secret",
                "permission",
                "authentication",
                "authorization",
            ],
        ),
        (
            "database",
            vec![
                "database",
                "sql",
                "postgres",
                "mysql",
                "sqlite",
                "migration",
                "schema",
            ],
        ),
        (
            "documentation",
            vec!["documentation", "docs", "readme", "runbook", "guide"],
        ),
    ])
}

fn normalize_text(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '#' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn includes_phrase(text: &str, phrase: &str) -> bool {
    format!(" {text} ").contains(&format!(" {phrase} "))
}
fn estimated_tokens(value: &str) -> usize {
    value.chars().count().div_ceil(4)
}
fn now_millis() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis()
        .try_into()
        .map_err(|_| "Timestamp exceeds u64 milliseconds.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(id: &str, category: &str, trigger: &str, priority: u8) -> AgentSkillDefinition {
        AgentSkillDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: format!("{id} guidance"),
            category: category.to_string(),
            custom_category: String::new(),
            trigger: trigger.to_string(),
            priority,
            enabled: true,
            instructions: format!("Follow {id}."),
            updated_at: "2026-08-08T00:00:00.000Z".to_string(),
        }
    }

    fn profile(catalog: Vec<AgentSkillDefinition>) -> AgentRuntimeProfile {
        let agent_rules = "Verify first.\nAsk before deleting.\nVerify first.".to_string();
        let skills_revision = skill_catalog_revision(&catalog).expect("catalog revision");
        AgentRuntimeProfile {
            id: "agent-governance-test".to_string(),
            runtime: "opencode".to_string(),
            model: "openai/gpt-5".to_string(),
            opencode_command: "opencode".to_string(),
            opencode_workdir: None,
            rules_revision: content_revision("Verify first.\nAsk before deleting."),
            skills_revision,
            agent_rules,
            agent_skills: String::new(),
            temperature: 0.2,
            connector_ids: Vec::new(),
            prompt_template_version: "agent-task-v1".to_string(),
            governance_schema_version: GOVERNANCE_RESOLUTION_VERSION,
            skill_catalog: catalog,
        }
    }

    fn contract(summary: &str) -> Value {
        serde_json::json!({
            "objective": { "summary": summary, "success_criteria": ["Verify the result"] },
            "task_context": { "description": "", "execution_detail": "" },
            "ticket": { "title": summary, "labels": [] },
            "workflow": [],
            "runtime_inputs": { "operator_notes": null, "requested_skill_ids": [] }
        })
    }

    #[test]
    fn kubernetes_selection_records_matches_and_rejections() {
        let profile = profile(vec![
            skill("kubernetes", "infrastructure", "contextual", 90),
            skill("documentation", "documentation", "contextual", 80),
        ]);
        let resolution = resolve_governance(
            42,
            &profile,
            &contract("Diagnose the failed Kubernetes pod in the cluster"),
        )
        .expect("governance resolves");
        assert_eq!(resolution.skills.selected_skill_ids, vec!["kubernetes"]);
        let kubernetes = &resolution.skills.entries[0];
        assert!(kubernetes.selected);
        assert_eq!(kubernetes.matched_domains, vec!["infrastructure"]);
        assert!(kubernetes
            .matched_intents
            .contains(&"kubernetes".to_string()));
        assert!(kubernetes.matched_intents.contains(&"pod".to_string()));
        assert!(!resolution.skills.entries[1].selected);
        assert_eq!(
            resolution.skills.entries[1].reason,
            "No contextual category matched."
        );
        assert_eq!(
            resolution.skills.catalog_revision.as_deref(),
            Some(profile.skills_revision.as_str())
        );
        assert!(resolution.skills.snapshot.contains("Skill ID: kubernetes"));
        assert!(!resolution
            .skills
            .snapshot
            .contains("Skill ID: documentation"));
    }

    #[test]
    fn rules_are_normalized_and_ordered_deterministically() {
        let profile = profile(Vec::new());
        let first = resolve_governance(7, &profile, &contract("Review a change"))
            .expect("first resolution");
        let second = resolve_governance(8, &profile, &contract("Review a change"))
            .expect("second resolution");
        assert_eq!(first.rules.snapshot, "Verify first.\nAsk before deleting.");
        assert_eq!(first.rules.entries, second.rules.entries);
        assert_eq!(first.rules.entries[0].scope, RuleScope::Platform);
        assert_eq!(first.rules.entries[1].scope, RuleScope::Global);
        assert!(first.rules.entries[0].precedence < first.rules.entries[1].precedence);
        assert_eq!(
            first.rules.normalization_version,
            RULES_NORMALIZATION_VERSION
        );
        assert_eq!(first.rules.facts.source_digest, first.rules.final_digest);
        assert_eq!(first.rules.facts.schema_version, RULE_FACTS_SCHEMA_VERSION);
    }

    #[test]
    fn rule_compiler_extracts_helm_repository_policy_and_environment_facts() {
        let rules = r#"
## Helm Repository
1. Local checkout: `/home/user/BRI/qcash-deployment`
2. Helm Repository is at Bitbucket repo https://bitbucket.example/projects/OPS/repos/qcash-deployment
3. Backend helm templates are at folder service/, dan frontend helm templates are at folder frontend/
4. Modify on master,eks-green,drc,cloud must ask approval

| Jira Label | Environment | Git Branch | OpenShift Namespace |
|------------|-------------|------------|---------------------|
| NQLA_PRESTAGE | prerelease | prerelease | qcash-prerelease |
| NQLA_DEV | dev | dev | bricams |
"#;
        let facts = compile_rule_facts(rules);
        assert_eq!(facts.schema_version, RULE_FACTS_SCHEMA_VERSION);
        assert_eq!(facts.compiler_version, RULE_FACTS_COMPILER_VERSION);
        assert_eq!(facts.repositories.len(), 1);
        assert_eq!(facts.repositories[0].id, "qcash-deployment");
        assert_eq!(
            facts.repositories[0].local_path.as_deref(),
            Some("/home/user/BRI/qcash-deployment")
        );
        assert_eq!(
            facts.repositories[0].backend_path.as_deref(),
            Some("service")
        );
        assert_eq!(
            facts.repositories[0].frontend_path.as_deref(),
            Some("frontend")
        );
        assert_eq!(facts.repositories[0].source, "global.agent_rules");
        assert_eq!(facts.repositories[0].source_line, 4);
        assert_eq!(
            facts.protected_branches[0].branches,
            vec!["cloud", "drc", "eks-green", "master"]
        );
        assert!(facts.protected_branches[0].approval_required);
        assert_eq!(facts.deployment_targets[0].label, "NQLA_DEV");
        assert_eq!(facts.deployment_targets[0].target, "dev");
        assert_eq!(facts.deployment_targets[0].source, "global.agent_rules");
        assert!(facts.deployment_targets[0].source_line > 0);
        assert_eq!(facts.deployment_targets[1].label, "NQLA_PRESTAGE");
        assert!(facts.warnings.is_empty());
    }

    #[test]
    fn rule_compiler_preserves_conflicting_rows_for_preflight_diagnostics() {
        let facts = compile_rule_facts(
            r#"
| Jira Label | Environment | Git Branch | OpenShift Namespace |
|------------|-------------|------------|---------------------|
| PRESTAGE_LOCAL | prerelease | prerelease | qcash-prerelease |
| PRESTAGE_LOCAL | drc | drc | qcash-drc |
| PRESTAGE_LOCAL | drc | drc | qcash-drc |
"#,
        );
        assert_eq!(facts.deployment_targets.len(), 2);
        assert_eq!(facts.deployment_targets[0].label, "PRESTAGE_LOCAL");
        assert_eq!(facts.deployment_targets[0].target, "drc");
        assert_eq!(facts.deployment_targets[1].target, "prerelease");
    }

    #[test]
    fn rule_compiler_warns_when_remote_repository_has_no_local_checkout() {
        let facts = compile_rule_facts(
            "Helm Repository is https://bitbucket.example/projects/OPS/repos/qcash-deployment",
        );
        assert_eq!(facts.repositories.len(), 1);
        assert_eq!(facts.warnings.len(), 1);
    }

    #[test]
    fn rule_compiler_extracts_generic_connector_configuration_with_provenance() {
        let facts = compile_rule_facts(
            r#"
## Connector: corporate-confluence
- Type: confluence
- Base URL: `https://confluence.example`
- Required operations:
  - search
  - get_page

## Other Rules
- Verify changes.
"#,
        );
        assert_eq!(facts.connectors.len(), 1);
        let connector = &facts.connectors[0];
        assert_eq!(connector.id, "corporate-confluence");
        assert_eq!(connector.connector_type, "confluence");
        assert_eq!(connector.base_url, "https://confluence.example");
        assert_eq!(connector.required_operations, vec!["get_page", "search"]);
        assert_eq!(connector.source, "global.agent_rules");
        assert_eq!(connector.source_line, 2);
    }

    #[test]
    fn rule_compiler_extracts_label_scoped_verification_policy() {
        let facts = compile_rule_facts(
            r#"
## Verification: confluence-source-read
- Connector: corporate-confluence
- Applies to labels:
  - NQLA_PRESTAGE
- Required successful operations:
  - search
  - get_page
"#,
        );
        assert_eq!(facts.verification_policies.len(), 1);
        let policy = &facts.verification_policies[0];
        assert_eq!(policy.id, "confluence-source-read");
        assert_eq!(policy.connector_id, "corporate-confluence");
        assert_eq!(policy.applies_to_labels, vec!["nqla_prestage"]);
        assert_eq!(policy.required_operations, vec!["get_page", "search"]);
        assert_eq!(policy.source, "global.agent_rules");
        assert_eq!(policy.source_line, 2);
    }

    #[test]
    fn rule_compiler_extracts_git_evidence_verifier() {
        let facts = compile_rule_facts(
            r#"
## Evidence Verifier: git-release-state
- Provider: git
- Applies to labels:
  - RELEASE
- Required states:
  - clean_worktree
  - new_commit
  - pushed_upstream
"#,
        );
        assert_eq!(facts.evidence_verifiers.len(), 1);
        let verifier = &facts.evidence_verifiers[0];
        assert_eq!(verifier.id, "git-release-state");
        assert_eq!(verifier.provider, "git");
        assert_eq!(verifier.applies_to_labels, vec!["release"]);
        assert_eq!(
            verifier.required_states,
            vec!["clean_worktree", "new_commit", "pushed_upstream"]
        );
        assert_eq!(verifier.source_line, 2);
    }

    #[test]
    fn rule_compiler_extracts_kubernetes_deployment_evidence_verifier() {
        let facts = compile_rule_facts(
            r#"
## Evidence Verifier: deployment-health
- Provider: kubernetes
- Required states:
  - deployment_available
- Poll interval seconds: 5
- Timeout seconds: 120
"#,
        );
        assert_eq!(facts.evidence_verifiers.len(), 1);
        assert_eq!(facts.evidence_verifiers[0].provider, "kubernetes");
        assert_eq!(
            facts.evidence_verifiers[0].required_states,
            vec!["deployment_available"]
        );
        assert_eq!(facts.evidence_verifiers[0].poll_interval_seconds, Some(5));
        assert_eq!(facts.evidence_verifiers[0].timeout_seconds, Some(120));
        assert!(facts.evidence_verifiers[0]
            .polling_configuration_error
            .is_none());
    }

    #[test]
    fn rule_compiler_retains_invalid_evidence_polling_configuration() {
        let facts = compile_rule_facts(
            r#"
## Evidence Verifier: deployment-health
- Provider: kubernetes
- Required states: deployment_available
- Poll interval seconds: soon
- Timeout seconds: 120
"#,
        );
        let verifier = &facts.evidence_verifiers[0];
        assert_eq!(verifier.poll_interval_seconds, None);
        assert_eq!(verifier.timeout_seconds, Some(120));
        assert_eq!(
            verifier.polling_configuration_error.as_deref(),
            Some("Poll interval seconds must be an integer.")
        );
    }

    #[test]
    fn rule_compiler_extracts_bamboo_connector_and_read_operation() {
        let facts = compile_rule_facts(
            r#"
## Evidence Verifier: bamboo-build-state
- Provider: bamboo
- Connector: corporate-bamboo
- Read operation: get_build
- Required states: successful_build
- Poll interval seconds: 5
- Timeout seconds: 120
"#,
        );
        let verifier = &facts.evidence_verifiers[0];
        assert_eq!(verifier.provider, "bamboo");
        assert_eq!(verifier.connector_id.as_deref(), Some("corporate-bamboo"));
        assert_eq!(verifier.read_operation.as_deref(), Some("get_build"));
        assert_eq!(verifier.required_states, vec!["successful_build"]);
        assert_eq!(verifier.poll_interval_seconds, Some(5));
        assert_eq!(verifier.timeout_seconds, Some(120));
    }

    #[test]
    fn rule_compiler_extracts_jira_issue_status_verifier() {
        let facts = compile_rule_facts(
            r#"
## Evidence Verifier: jira-in-progress
- Provider: jira
- Connector: corporate-jira
- Read operation: get_issue
- Required states: expected_status
- Expected status: In Progress
"#,
        );
        let verifier = &facts.evidence_verifiers[0];
        assert_eq!(verifier.provider, "jira");
        assert_eq!(verifier.connector_id.as_deref(), Some("corporate-jira"));
        assert_eq!(verifier.read_operation.as_deref(), Some("get_issue"));
        assert_eq!(verifier.required_states, vec!["expected_status"]);
        assert_eq!(verifier.expected_status.as_deref(), Some("In Progress"));
    }

    #[test]
    fn rule_compiler_extracts_jira_comment_verifier() {
        let facts = compile_rule_facts(
            r#"
## Evidence Verifier: jira-comment
- Provider: jira
- Connector: corporate-jira
- Read operation: get_issue
- Required states: comment_matches
"#,
        );
        let verifier = &facts.evidence_verifiers[0];
        assert_eq!(verifier.provider, "jira");
        assert_eq!(verifier.required_states, vec!["comment_matches"]);
        assert_eq!(verifier.expected_status, None);
    }

    #[test]
    fn rule_compiler_extracts_confluence_page_verifier() {
        let facts = compile_rule_facts(
            r#"
## Evidence Verifier: confluence-page
- Provider: confluence
- Connector: corporate-confluence
- Read operation: get_page
- Required states: page_exists
"#,
        );
        let verifier = &facts.evidence_verifiers[0];
        assert_eq!(verifier.provider, "confluence");
        assert_eq!(
            verifier.connector_id.as_deref(),
            Some("corporate-confluence")
        );
        assert_eq!(verifier.read_operation.as_deref(), Some("get_page"));
        assert_eq!(verifier.required_states, vec!["page_exists"]);
    }

    #[test]
    fn retained_resolution_is_self_consistent_and_catalog_independent() {
        let initial = profile(vec![skill("kubernetes", "infrastructure", "automatic", 90)]);
        let resolution = resolve_governance(5, &initial, &contract("Run diagnostics"))
            .expect("initial resolution");
        let changed = profile(vec![skill(
            "documentation",
            "documentation",
            "automatic",
            90,
        )]);
        assert_ne!(initial.skills_revision, changed.skills_revision);
        resolution
            .validate_for(5)
            .expect("retained snapshot remains authoritative after settings change");
        assert_eq!(resolution.skills.selected_skill_ids, vec!["kubernetes"]);
    }

    #[test]
    fn retained_pre_facts_governance_snapshot_remains_valid() {
        let mut resolution = resolve_governance(5, &profile(Vec::new()), &contract("Inspect"))
            .expect("governance resolves");
        resolution.rules.facts = RuleFactsRecord::default();
        resolution
            .validate_for(5)
            .expect("pre-facts retained governance remains compatible");
    }

    #[test]
    fn retained_prior_rule_facts_remain_valid_after_v7_compiler_upgrade() {
        let mut resolution = resolve_governance(5, &profile(Vec::new()), &contract("Inspect"))
            .expect("governance resolves");
        resolution.rules.facts.schema_version = RECENT_RULE_FACTS_SCHEMA_VERSION;
        resolution.rules.facts.compiler_version = RECENT_RULE_FACTS_COMPILER_VERSION.to_string();
        resolution
            .validate_for(5)
            .expect("retained v6 rule facts remain compatible");

        resolution.rules.facts.schema_version = PRIOR_RULE_FACTS_SCHEMA_VERSION;
        resolution.rules.facts.compiler_version = PRIOR_RULE_FACTS_COMPILER_VERSION.to_string();
        resolution
            .validate_for(5)
            .expect("retained v5 rule facts remain compatible");

        resolution.rules.facts.schema_version = PREVIOUS_RULE_FACTS_SCHEMA_VERSION;
        resolution.rules.facts.compiler_version = PREVIOUS_RULE_FACTS_COMPILER_VERSION.to_string();
        resolution
            .validate_for(5)
            .expect("retained v4 rule facts remain compatible");

        resolution.rules.facts.schema_version = OLDER_RULE_FACTS_SCHEMA_VERSION;
        resolution.rules.facts.compiler_version = OLDER_RULE_FACTS_COMPILER_VERSION.to_string();
        resolution
            .validate_for(5)
            .expect("retained v3 rule facts remain compatible");

        resolution.rules.facts.schema_version = ANCIENT_RULE_FACTS_SCHEMA_VERSION;
        resolution.rules.facts.compiler_version = ANCIENT_RULE_FACTS_COMPILER_VERSION.to_string();
        resolution
            .validate_for(5)
            .expect("retained v2 rule facts remain compatible");

        resolution.rules.facts.schema_version = LEGACY_RULE_FACTS_SCHEMA_VERSION;
        resolution.rules.facts.compiler_version = LEGACY_RULE_FACTS_COMPILER_VERSION.to_string();
        resolution
            .validate_for(5)
            .expect("retained v1 rule facts remain compatible");
    }

    #[test]
    fn legacy_execution_does_not_fabricate_resolution_reasons() {
        let mut profile = profile(Vec::new());
        profile.governance_schema_version = 0;
        profile.agent_skills = "Skill: Legacy".to_string();
        profile.skills_revision = content_revision(&profile.agent_skills);
        let resolution =
            resolve_governance(9, &profile, &contract("Legacy task")).expect("legacy resolution");
        assert_eq!(
            resolution.status,
            GovernanceResolutionStatus::LegacyUnavailable
        );
        assert!(resolution.skills.entries.is_empty());
        assert_eq!(resolution.skills.snapshot, "Skill: Legacy");
    }

    #[test]
    fn full_catalog_injects_only_relevant_skills_with_bounded_prompt_growth() {
        let mut catalog = (0..63)
            .map(|index| {
                skill(
                    &format!("documentation-{index}"),
                    "documentation",
                    "contextual",
                    50,
                )
            })
            .collect::<Vec<_>>();
        catalog.push(skill("kubernetes", "infrastructure", "contextual", 90));
        let profile = profile(catalog);
        let started = std::time::Instant::now();
        let resolution =
            resolve_governance(64, &profile, &contract("Diagnose a Kubernetes pod failure"))
                .expect("large catalog resolves");
        assert_eq!(resolution.skills.entries.len(), 64);
        assert_eq!(resolution.skills.selected_skill_ids, vec!["kubernetes"]);
        assert_eq!(resolution.skills.snapshot.matches("Skill ID:").count(), 1);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(100),
            "64-skill selection should remain outside the startup critical path"
        );
    }

    #[test]
    fn concurrent_resolutions_never_share_task_or_skill_state() {
        let handles = (1..=5)
            .map(|task_session_id| {
                std::thread::spawn(move || {
                    let skill_id = format!("task-{task_session_id}-skill");
                    let profile =
                        profile(vec![skill(&skill_id, "infrastructure", "automatic", 90)]);
                    resolve_governance(
                        task_session_id,
                        &profile,
                        &contract("Concurrent worker task"),
                    )
                    .expect("concurrent governance resolves")
                })
            })
            .collect::<Vec<_>>();
        let resolutions = handles
            .into_iter()
            .map(|handle| handle.join().expect("resolution thread"))
            .collect::<Vec<_>>();
        for (index, resolution) in resolutions.iter().enumerate() {
            let task_session_id = index as u64 + 1;
            assert_eq!(resolution.task_session_id, task_session_id);
            assert_eq!(
                resolution.skills.selected_skill_ids,
                vec![format!("task-{task_session_id}-skill")]
            );
        }
    }

    #[test]
    fn authoritative_profiles_reject_renderer_governance_snapshots() {
        let profile = profile(Vec::new());
        let mut contract = contract("New task");
        contract["runtime_inputs"]["selected_skills_snapshot"] = Value::String(String::new());
        assert!(resolve_governance(10, &profile, &contract)
            .expect_err("renderer snapshot must not downgrade backend authority")
            .contains("Legacy renderer governance snapshots"));
    }
}
