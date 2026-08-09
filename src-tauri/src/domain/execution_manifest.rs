//! Durable, secret-free evidence describing one Task Session execution attempt.

use crate::domain::governance::{RuleResolutionEntry, SkillResolutionEntry};
use crate::domain::task_session::{TaskMcpConnectorContext, TaskSessionId, TaskSessionKind};
use serde::{Deserialize, Serialize};

pub const EXECUTION_MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionModelConfiguration {
    pub provider_id: String,
    pub api_style: String,
    /// Decimal string to avoid platform-dependent floating-point equality in immutable evidence.
    pub temperature: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionManifestDraft {
    pub kind: TaskSessionKind,
    pub workspace_id: String,
    pub subject_id: Option<String>,
    pub conversation_id: Option<String>,
    pub execution_run_id: Option<String>,
    pub context_digest: String,
    pub context_revision: Option<String>,
    pub runtime: String,
    pub runtime_profile_id: String,
    pub runtime_id: String,
    pub model: String,
    pub model_configuration: ExecutionModelConfiguration,
    pub prompt_template_version: String,
    pub rules_revision: Option<String>,
    pub skills_revision: Option<String>,
    pub rules: Vec<RuleResolutionEntry>,
    pub rules_digest: String,
    pub skills_catalog_revision: Option<String>,
    pub skills: Vec<SkillResolutionEntry>,
    pub connectors: Vec<TaskMcpConnectorContext>,
    pub tool_permission_mode: String,
    pub unknown_fields: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionManifest {
    pub schema_version: u32,
    pub task_session_id: TaskSessionId,
    pub assignment_attempt_id: u64,
    pub assignment_attempt: u32,
    pub worker_id: usize,
    pub fencing_token: u64,
    pub started_at: u64,
    #[serde(flatten)]
    pub execution: ExecutionManifestDraft,
}

/// Read projection joining immutable attempt evidence with Task-Session-owned runtime identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskExecutionManifest {
    #[serde(flatten)]
    pub manifest: ExecutionManifest,
    pub opencode_session_id: Option<String>,
    pub coverage: String,
}

impl ExecutionManifest {
    pub fn validate_for(&self, task_session_id: TaskSessionId) -> Result<(), String> {
        if self.schema_version != EXECUTION_MANIFEST_SCHEMA_VERSION {
            return Err("Execution Manifest schema is not supported.".to_string());
        }
        if self.task_session_id != task_session_id {
            return Err("Execution Manifest belongs to a different Task Session.".to_string());
        }
        for (name, value) in [
            ("workspace_id", self.execution.workspace_id.as_str()),
            ("context_digest", self.execution.context_digest.as_str()),
            ("runtime", self.execution.runtime.as_str()),
            (
                "runtime_profile_id",
                self.execution.runtime_profile_id.as_str(),
            ),
            ("runtime_id", self.execution.runtime_id.as_str()),
            ("model", self.execution.model.as_str()),
            (
                "prompt_template_version",
                self.execution.prompt_template_version.as_str(),
            ),
        ] {
            if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
                return Err(format!("Execution Manifest field '{name}' is invalid."));
            }
        }
        if self.execution.rules.len() > 64
            || self.execution.skills.len() > 64
            || self.execution.connectors.len() > 64
            || self.execution.unknown_fields.len() > 32
        {
            return Err("Execution Manifest metadata exceeds its bounded limits.".to_string());
        }
        Ok(())
    }
}
