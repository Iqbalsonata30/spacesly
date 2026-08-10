use super::agent_task_executor::{AgentRuntimeResolver, ResolvedAgentTask};
use crate::domain::governance::{resolve_governance, GovernanceResolutionRecord};
use crate::domain::task_session::TaskSessionEnvelopeV1;
use crate::infrastructure::ai_worker::{AiWorkerConfig, AiWorkerMcpServer, AiWorkerTask};
use crate::infrastructure::execution_store::{ChatConversationSnapshot, ExecutionStore};
use crate::infrastructure::files::WorkspaceRoot;
use crate::infrastructure::mcp::discover_mcp_capability_snapshot;
use crate::infrastructure::ocp;
use crate::infrastructure::provider_registry;
use crate::infrastructure::runtime_profile_store::{AgentRuntimeProfile, RuntimeProfileStore};
use crate::infrastructure::secrets::AppSecretsStore;
use crate::infrastructure::workspace_trust::WorkspaceTrustRegistry;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Production resolver for scheduler-owned Agent attempts.
#[derive(Clone)]
pub struct StoredAgentRuntimeResolver {
    profiles: RuntimeProfileStore,
    executions: ExecutionStore,
    secrets: AppSecretsStore,
    workspace_roots: WorkspaceRoot,
    workspace_trust: WorkspaceTrustRegistry,
}

/// Trusted non-secret identity and owned runtime configuration for one prompt attempt.
pub(crate) struct ResolvedPromptRuntime {
    pub runtime_profile_id: String,
    pub config: AiWorkerConfig,
    pub chat_snapshot: Option<ChatConversationSnapshot>,
    profile: AgentRuntimeProfile,
}

impl StoredAgentRuntimeResolver {
    /// Creates a resolver that borrows singleton stores but resolves per-session runtime state.
    ///
    /// The resolver itself is a shared service; each `resolve` call returns owned runtime config,
    /// MCP connector command/environment snapshots, and task contract data for exactly one
    /// assignment attempt.
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

    /// Resolves one trusted runtime configuration without resolving a kind-specific prompt input.
    pub(crate) fn resolve_prompt_runtime(
        &self,
        envelope: &TaskSessionEnvelopeV1,
    ) -> Result<ResolvedPromptRuntime, String> {
        let total = crate::infrastructure::performance::span(
            "runtime_context_resolution_ms",
            "agent_runtime",
        );
        let profile_resolution = crate::infrastructure::performance::span(
            "runtime_profile_resolution_ms",
            "agent_runtime",
        );
        let profile = self
            .profiles
            .get(&envelope.runtime_profile_id)?
            .ok_or_else(|| {
                format!(
                    "Agent runtime profile '{}' was not found.",
                    envelope.runtime_profile_id
                )
            })?;
        profile_resolution.finish();
        validate_profile_binding(envelope, &profile)?;

        let workspace_resolution =
            crate::infrastructure::performance::span("workspace_resolution_ms", "workspace");
        let workspace = resolve_profile_workspace(
            &profile,
            &self.workspace_roots,
            &self.workspace_trust,
            &envelope.workspace_id,
        )?;
        workspace_resolution.finish();
        if envelope.kind != crate::domain::task_session::TaskSessionKind::Chat {
            let workspace_validation =
                crate::infrastructure::performance::span("workspace_validation_ms", "workspace");
            let workspace_revision = self.workspace_roots.revision(&envelope.workspace_id)?;
            if envelope.context_revision.as_deref() != Some(workspace_revision.to_string().as_str())
            {
                return Err("Prompt workspace revision did not match the envelope.".to_string());
            }
            workspace_validation.finish();
        }

        let (provider_id, model) = profile
            .model
            .split_once('/')
            .ok_or_else(|| "Agent model must use the '<provider>/<model>' form.".to_string())?;
        let provider = provider_registry::profile(provider_id);

        let connector_resolution =
            crate::infrastructure::performance::span("connector_resolution_ms", "agent_runtime");
        let mcp_servers = envelope
            .connector_ids
            .iter()
            .map(|connector_id| {
                let connector = self.secrets.mcp_connector(connector_id)?;
                let environment = self.secrets.mcp_environment(connector_id)?;
                if ocp::is_ocp_connector_env(&environment) {
                    return ocp::ocp_worker_server(connector_id, &environment);
                }
                let mut command = Vec::with_capacity(connector.args.len() + 1);
                command.push(connector.command);
                command.extend(connector.args);
                Ok(AiWorkerMcpServer {
                    name: connector_id.clone(),
                    secret_id: connector_id.clone(),
                    command,
                    environment,
                    proxy_authority: None,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        connector_resolution.finish();

        let rules_resolution =
            crate::infrastructure::performance::span("rules_resolution_ms", "agent_runtime");
        let agent_rules = profile.agent_rules.clone();
        rules_resolution.finish();
        let skills_resolution =
            crate::infrastructure::performance::span("skills_resolution_ms", "agent_runtime");
        let agent_skills = profile.agent_skills.clone();
        skills_resolution.finish();

        let resolved = ResolvedPromptRuntime {
            runtime_profile_id: profile.id.clone(),
            config: AiWorkerConfig {
                workspace_id: envelope.workspace_id.clone(),
                runtime: profile.runtime.clone(),
                provider_name: provider
                    .map_or(provider_id, |profile| profile.name)
                    .to_string(),
                provider_id: provider_id.to_string(),
                base_url: provider.map_or("", |profile| profile.base_url).to_string(),
                api_style: provider
                    .map(|profile| profile.api_style.as_str())
                    .unwrap_or("openai_chat")
                    .to_string(),
                api_key: self.secrets.ai_api_key(provider_id)?,
                model: model.to_string(),
                opencode_command: profile.opencode_command.clone(),
                opencode_model: envelope.model.clone(),
                opencode_workdir: Some(workspace.to_string_lossy().to_string()),
                opencode_auto_approve: false,
                agent_rules,
                agent_skills,
                governance_schema_version: profile.governance_schema_version,
                skill_catalog: profile.skill_catalog.clone(),
                temperature: profile.temperature,
                restrict_tools: true,
                fenced_tools_only: true,
                isolated_opencode_process: true,
                task_tool_authority: None,
                mcp_servers,
            },
            chat_snapshot: None,
            profile,
        };
        total.finish();
        Ok(resolved)
    }

    /// Resolves an exact backend-owned Chat snapshot from durable conversation state.
    pub(crate) fn resolve_chat_snapshot(
        &self,
        envelope: &TaskSessionEnvelopeV1,
        message_id: &str,
        message_sequence: u64,
        message: &str,
    ) -> Result<ChatConversationSnapshot, String> {
        let conversation_id = envelope
            .conversation_id
            .as_deref()
            .ok_or_else(|| "Chat conversation ID is required.".to_string())?;
        self.executions.resolve_chat_snapshot(
            &envelope.workspace_id,
            conversation_id,
            message_id,
            message_sequence,
            message,
        )
    }

    /// Revalidates that a resolved Chat snapshot remains the current durable model authority.
    pub(crate) fn revalidate_chat_snapshot(
        &self,
        snapshot: &ChatConversationSnapshot,
    ) -> Result<(), String> {
        self.executions.revalidate_chat_snapshot(snapshot)
    }
}

fn resolve_profile_workspace(
    profile: &AgentRuntimeProfile,
    roots: &WorkspaceRoot,
    trust: &WorkspaceTrustRegistry,
    workspace_id: &str,
) -> Result<PathBuf, String> {
    match profile.opencode_workdir.as_deref() {
        Some(path) => trust.require_trusted_path(Path::new(path)),
        None => trust.require_trusted(roots, workspace_id),
    }
}

impl AgentRuntimeResolver for StoredAgentRuntimeResolver {
    fn resolve(
        &self,
        task_session_id: u64,
        envelope: &TaskSessionEnvelopeV1,
        runtime_attempt_id: &str,
        retained_governance: Option<&GovernanceResolutionRecord>,
    ) -> Result<ResolvedAgentTask, String> {
        if runtime_attempt_id.trim().is_empty() {
            return Err("Agent runtime attempt ID is required.".to_string());
        }
        envelope.validate_agent_runtime_ownership()?;
        let mut runtime = self.resolve_prompt_runtime(envelope)?;

        let execution_run_id = envelope
            .execution_run_id
            .as_deref()
            .ok_or_else(|| "Agent execution run ID is required.".to_string())?;
        let conversation_id = envelope
            .conversation_id
            .as_deref()
            .ok_or_else(|| "Agent conversation ID is required.".to_string())?;
        if !self
            .executions
            .conversation_exists(&envelope.workspace_id, conversation_id)?
        {
            return Err("Agent conversation does not belong to this workspace.".to_string());
        }
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

        let governance = match retained_governance {
            Some(resolution) => {
                resolution.validate_for(task_session_id)?;
                resolution.clone()
            }
            None => resolve_governance(task_session_id, &runtime.profile, &run.contract)?,
        };
        runtime.config.agent_rules = governance.rules.snapshot.clone();
        runtime.config.agent_skills = governance.skills.snapshot.clone();

        if let Some(approval) = ocp::contract_approved_mutation(&run.contract) {
            for server in &mut runtime.config.mcp_servers {
                if server.environment.contains_key(ocp::ENV_MODE) {
                    server.environment.insert(
                        ocp::ENV_APPROVED_OPERATION.to_string(),
                        approval.operation.clone(),
                    );
                    server.environment.insert(
                        ocp::ENV_APPROVED_ARGUMENTS_DIGEST.to_string(),
                        approval.arguments_digest.clone(),
                    );
                }
            }
        }

        let connector_capabilities = discover_connector_capabilities(&runtime.config.mcp_servers);

        Ok(ResolvedAgentTask {
            runtime_profile_id: runtime.runtime_profile_id,
            config: runtime.config,
            governance,
            connector_capabilities,
            task: AiWorkerTask {
                execution_contract: Some(run.contract),
                task_examination: None,
                session_key: Some(runtime_attempt_id.to_string()),
                opencode_session_id: None,
            },
        })
    }
}

fn discover_connector_capabilities(
    servers: &[AiWorkerMcpServer],
) -> Vec<crate::domain::task_examination::ConnectorCapabilitySnapshot> {
    let mut snapshots = Vec::with_capacity(servers.len());
    for chunk in servers.chunks(8) {
        let discovered = std::thread::scope(|scope| {
            chunk
                .iter()
                .map(|server| {
                    (
                        server.secret_id.clone(),
                        scope.spawn(|| {
                            discover_mcp_capability_snapshot(
                                &server.secret_id,
                                &server.command,
                                &server.environment,
                            )
                        }),
                    )
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(connector_id, handle)| {
                    handle.join().unwrap_or_else(|_| {
                        crate::domain::task_examination::ConnectorCapabilitySnapshot {
                            connector_id,
                            status: crate::domain::task_examination::ConnectorDiscoveryStatus::Unavailable,
                            tools: Vec::new(),
                            error: Some("MCP capability discovery panicked.".to_string()),
                            warnings: Vec::new(),
                        }
                    })
                })
                .collect::<Vec<_>>()
        });
        snapshots.extend(discovered);
    }
    snapshots.sort_by(|left, right| left.connector_id.cmp(&right.connector_id));
    snapshots
}

fn validate_profile_binding(
    envelope: &TaskSessionEnvelopeV1,
    profile: &AgentRuntimeProfile,
) -> Result<(), String> {
    profile.validate()?;
    profile.validate_content_revisions()?;
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
    use crate::infrastructure::runtime_profile_store::content_revision;
    use tempfile::tempdir;

    #[test]
    fn profile_binding_rejects_unapproved_connector() {
        let mut envelope = test_envelope();
        assert!(validate_profile_binding(&envelope, &test_profile()).is_ok());
        envelope.connector_ids = vec!["jira".to_string(), "github".to_string()];
        assert!(validate_profile_binding(&envelope, &test_profile()).is_err());
    }

    #[test]
    fn profile_binding_requires_exact_revisions() {
        let mut envelope = test_envelope();
        assert!(validate_profile_binding(&envelope, &test_profile()).is_ok());
        envelope.rules_revision = Some("stale".to_string());
        assert!(validate_profile_binding(&envelope, &test_profile()).is_err());
    }

    #[test]
    fn configured_working_directory_is_the_resolved_workspace_authority() {
        let directory = tempdir().unwrap();
        let open_workspace = directory.path().join("open-workspace");
        let configured = directory.path().join("configured");
        std::fs::create_dir_all(&open_workspace).unwrap();
        std::fs::create_dir_all(&configured).unwrap();
        let roots = WorkspaceRoot::home().unwrap();
        roots
            .set_path("workspace-personal", open_workspace)
            .unwrap();
        let trust = WorkspaceTrustRegistry::default();
        trust.trust_path(&configured).unwrap();
        let mut profile = test_profile();
        profile.opencode_workdir = Some(configured.to_string_lossy().to_string());

        assert_eq!(
            resolve_profile_workspace(&profile, &roots, &trust, "workspace-personal").unwrap(),
            configured.canonicalize().unwrap()
        );
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
            rules_revision: Some(content_revision("Use evidence.")),
            skills_revision: Some(content_revision("Verify changes.")),
        }
    }

    fn test_profile() -> AgentRuntimeProfile {
        AgentRuntimeProfile {
            id: "agent-default".to_string(),
            runtime: "opencode".to_string(),
            model: "openai/gpt-5.5".to_string(),
            opencode_command: "opencode".to_string(),
            opencode_workdir: None,
            agent_rules: "Use evidence.".to_string(),
            agent_skills: "Verify changes.".to_string(),
            temperature: 0.2,
            connector_ids: vec!["jira".to_string()],
            prompt_template_version: "agent-v1".to_string(),
            rules_revision: content_revision("Use evidence."),
            skills_revision: content_revision("Verify changes."),
            governance_schema_version: 0,
            skill_catalog: Vec::new(),
        }
    }
}
