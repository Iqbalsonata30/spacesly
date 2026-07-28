use super::agent_task_executor::{AgentRuntimeResolver, ResolvedAgentTask};
use crate::domain::task_session::TaskSessionEnvelopeV1;
use crate::infrastructure::ai_worker::{AiWorkerConfig, AiWorkerMcpServer, AiWorkerTask};
use crate::infrastructure::execution_store::ExecutionStore;
use crate::infrastructure::files::WorkspaceRoot;
use crate::infrastructure::provider_registry;
use crate::infrastructure::runtime_profile_store::{AgentRuntimeProfile, RuntimeProfileStore};
use crate::infrastructure::secrets::AppSecretsStore;
use crate::infrastructure::workspace_trust::WorkspaceTrustRegistry;
use std::collections::HashSet;

/// Production resolver for scheduler-owned Agent attempts.
#[derive(Clone)]
pub struct StoredAgentRuntimeResolver {
    profiles: RuntimeProfileStore,
    executions: ExecutionStore,
    secrets: AppSecretsStore,
    workspace_roots: WorkspaceRoot,
    workspace_trust: WorkspaceTrustRegistry,
}

impl StoredAgentRuntimeResolver {
    pub fn new(
        profiles: RuntimeProfileStore,
        executions: ExecutionStore,
        secrets: AppSecretsStore,
        workspace_roots: WorkspaceRoot,
        workspace_trust: WorkspaceTrustRegistry,
    ) -> Self {
        Self {
            profiles,
            executions,
            secrets,
            workspace_roots,
            workspace_trust,
        }
    }
}

impl AgentRuntimeResolver for StoredAgentRuntimeResolver {
    fn resolve(
        &self,
        envelope: &TaskSessionEnvelopeV1,
        runtime_attempt_id: &str,
    ) -> Result<ResolvedAgentTask, String> {
        if runtime_attempt_id.trim().is_empty() {
            return Err("Agent runtime attempt ID is required.".to_string());
        }
        let profile = self
            .profiles
            .get(&envelope.runtime_profile_id)?
            .ok_or_else(|| {
                format!(
                    "Agent runtime profile '{}' was not found.",
                    envelope.runtime_profile_id
                )
            })?;
        validate_profile_binding(envelope, &profile)?;

        let workspace = self
            .workspace_trust
            .require_trusted(&self.workspace_roots, &envelope.workspace_id)?;
        let workspace_revision = self.workspace_roots.revision(&envelope.workspace_id)?;
        if envelope.context_revision.as_deref() != Some(workspace_revision.to_string().as_str()) {
            return Err("Agent workspace revision did not match the envelope.".to_string());
        }

        let execution_run_id = envelope
            .execution_run_id
            .as_deref()
            .ok_or_else(|| "Agent execution run ID is required.".to_string())?;
        let run = self
            .executions
            .get(execution_run_id)?
            .ok_or_else(|| format!("Execution run '{execution_run_id}' was not found."))?;
        if run
            .contract
            .get("workspace_id")
            .and_then(serde_json::Value::as_str)
            != Some(envelope.workspace_id.as_str())
        {
            return Err("Execution contract workspace did not match the envelope.".to_string());
        }

        let (provider_id, model) = profile
            .model
            .split_once('/')
            .ok_or_else(|| "Agent model must use the '<provider>/<model>' form.".to_string())?;
        let provider = provider_registry::profile(provider_id)
            .ok_or_else(|| format!("AI provider '{provider_id}' is not registered."))?;
        if !provider.models.contains(&model) {
            return Err(format!(
                "Model '{model}' is not registered for provider '{provider_id}'."
            ));
        }

        let mcp_servers = envelope
            .connector_ids
            .iter()
            .map(|connector_id| {
                let connector = self.secrets.mcp_connector(connector_id)?;
                let mut command = Vec::with_capacity(connector.args.len() + 1);
                command.push(connector.command);
                command.extend(connector.args);
                Ok(AiWorkerMcpServer {
                    name: connector_id.clone(),
                    secret_id: connector_id.clone(),
                    command,
                    environment: self.secrets.mcp_environment(connector_id)?,
                    proxy_authority: None,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        Ok(ResolvedAgentTask {
            runtime_profile_id: profile.id,
            config: AiWorkerConfig {
                workspace_id: envelope.workspace_id.clone(),
                runtime: profile.runtime,
                provider_name: provider.name.to_string(),
                provider_id: provider.id.to_string(),
                base_url: provider.base_url.to_string(),
                api_style: provider.api_style.as_str().to_string(),
                api_key: self.secrets.ai_api_key(provider.id)?,
                model: model.to_string(),
                opencode_command: profile.opencode_command,
                opencode_model: envelope.model.clone(),
                opencode_workdir: Some(workspace.to_string_lossy().to_string()),
                opencode_auto_approve: false,
                agent_rules: profile.agent_rules,
                agent_skills: profile.agent_skills,
                temperature: profile.temperature,
                restrict_tools: true,
                fenced_tools_only: true,
                isolated_opencode_process: true,
                mcp_servers,
            },
            task: AiWorkerTask {
                execution_contract: Some(run.contract),
                session_key: Some(runtime_attempt_id.to_string()),
            },
        })
    }
}

fn validate_profile_binding(
    envelope: &TaskSessionEnvelopeV1,
    profile: &AgentRuntimeProfile,
) -> Result<(), String> {
    profile.validate()?;
    if profile.id != envelope.runtime_profile_id
        || profile.model != envelope.model
        || profile.prompt_template_version != envelope.prompt_template_version
        || envelope.rules_revision.as_deref() != Some(profile.rules_revision.as_str())
        || envelope.skills_revision.as_deref() != Some(profile.skills_revision.as_str())
    {
        return Err("Agent runtime profile revisions did not match the envelope.".to_string());
    }
    let allowed = profile.connector_ids.iter().collect::<HashSet<_>>();
    if envelope
        .connector_ids
        .iter()
        .any(|connector| !allowed.contains(connector))
    {
        return Err("Agent connector is not allowed by the runtime profile.".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::task_session::TaskSessionKind;

    #[test]
    fn profile_binding_rejects_unapproved_connector() {
        let mut envelope = test_envelope();
        envelope.connector_ids = vec!["jira".to_string(), "github".to_string()];
        assert!(validate_profile_binding(&envelope, &test_profile()).is_err());
    }

    #[test]
    fn profile_binding_requires_exact_revisions() {
        let mut envelope = test_envelope();
        envelope.rules_revision = Some("stale".to_string());
        assert!(validate_profile_binding(&envelope, &test_profile()).is_err());
    }

    fn test_envelope() -> TaskSessionEnvelopeV1 {
        TaskSessionEnvelopeV1 {
            workspace_id: "workspace-personal".to_string(),
            kind: TaskSessionKind::Agent,
            subject_id: None,
            conversation_id: None,
            execution_run_id: Some("run-1".to_string()),
            context_digest: "sha256:digest".to_string(),
            runtime_profile_id: "agent-default".to_string(),
            model: "openai/gpt-5.5".to_string(),
            connector_ids: vec!["jira".to_string()],
            requested_capabilities: vec!["external_tools:jira".to_string()],
            prompt_template_version: "agent-v1".to_string(),
            context_revision: Some("1".to_string()),
            rules_revision: Some("rules-v1".to_string()),
            skills_revision: Some("skills-v1".to_string()),
        }
    }

    fn test_profile() -> AgentRuntimeProfile {
        AgentRuntimeProfile {
            id: "agent-default".to_string(),
            runtime: "opencode".to_string(),
            model: "openai/gpt-5.5".to_string(),
            opencode_command: "opencode".to_string(),
            agent_rules: "Use evidence.".to_string(),
            agent_skills: "Verify changes.".to_string(),
            temperature: 0.2,
            connector_ids: vec!["jira".to_string()],
            prompt_template_version: "agent-v1".to_string(),
            rules_revision: "rules-v1".to_string(),
            skills_revision: "skills-v1".to_string(),
        }
    }
}
