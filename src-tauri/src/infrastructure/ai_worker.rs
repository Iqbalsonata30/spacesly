use super::shell_env::inject_shell_env;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Clone, Debug, Deserialize)]
pub struct AiWorkerConfig {
    pub runtime: String,
    pub provider_name: String,
    pub base_url: String,
    pub api_style: String,
    pub api_key: String,
    pub model: String,
    pub opencode_command: String,
    pub opencode_model: String,
    pub opencode_workdir: Option<String>,
    pub opencode_auto_approve: bool,
    pub agent_rules: String,
    pub agent_skills: String,
    pub temperature: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AiWorkerTask {
    pub key: Option<String>,
    pub title: String,
    pub description: String,
    pub labels: Vec<String>,
    pub url: Option<String>,
    pub operator_notes: Option<String>,
    pub previous_output: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AiWorkerChatRequest {
    pub message: String,
    pub terminal_context: Option<String>,
    pub session_context: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AiWorkerStatus {
    pub connected: bool,
    pub provider_name: String,
    pub model: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AiWorkerTaskResult {
    pub summary: String,
    pub evidence: Vec<String>,
    pub details: Vec<String>,
    pub next: Vec<String>,
    pub completion_status: AiWorkerCompletionStatus,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StructuredAiWorkerTaskResult {
    summary: String,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    details: Vec<String>,
    #[serde(default)]
    next: Vec<String>,
    #[serde(alias = "status")]
    completion_status: String,
    #[serde(default)]
    blocked_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AiWorkerCompletionStatus {
    Completed,
    Blocked,
}

#[derive(Clone, Debug, Serialize)]
pub struct AiWorkerChatResult {
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessageResponse,
}

#[derive(Debug, Deserialize)]
struct ChatMessageResponse {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponsesResponse {
    output_text: Option<String>,
    output: Option<Vec<OpenAiOutputItem>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiOutputItem {
    content: Option<Vec<OpenAiContentItem>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiContentItem {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageResponse {
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

pub fn test_ai_worker(config: AiWorkerConfig) -> Result<AiWorkerStatus, String> {
    if config.runtime == "opencode" {
        return test_opencode_worker(config);
    }

    validate_config(&config)?;

    let response = call_model(
        &config,
        "You are Spacesly worker diagnostics. Reply with exactly: connected",
        "Return only the word connected.",
        32,
    )?;

    Ok(AiWorkerStatus {
        connected: true,
        provider_name: provider_label(&config),
        model: config.model,
        message: first_line(&response),
    })
}

pub fn execute_ai_worker_task(
    config: AiWorkerConfig,
    task: AiWorkerTask,
) -> Result<AiWorkerTaskResult, String> {
    if config.runtime == "opencode" {
        return execute_opencode_task(config, task);
    }

    validate_config(&config)?;

    let system_prompt = format!(
        "You are an Agent inside Spacesly, an orchestration app for human and AI agents. You receive Jira-style work cards and produce a concrete execution result. This direct API runtime does not have filesystem, shell, browser, Jira, Kubernetes, Bamboo, or MCP tools. Set completion_status to completed only for reasoning/reporting tasks that require no external side effects. If the work requires changing files, running commands, checking external systems, or using unavailable credentials/tools, set completion_status to blocked and explain what runtime/tool is needed. Return only valid JSON matching the requested schema. Do not wrap it in Markdown.\n\n{}",
        governance_context(&config, true),
    );
    let user_prompt = format!(
        "Task key: {}\nTitle: {}\nURL: {}\nLabels: {}\n\nDescription:\n{}\n\nOperator notes / approvals:\n{}\n\nPrevious Agent output from this card session:\n{}\n\nReturn exactly one JSON object with this schema:\n{{\n  \"completion_status\": \"completed\" | \"blocked\",\n  \"summary\": \"one sentence\",\n  \"evidence\": [\"what was actually verified\"],\n  \"details\": [\"concise notes\"],\n  \"next\": [\"operator follow-up steps, empty if none\"],\n  \"blocked_reason\": \"required when completion_status is blocked, otherwise null\"\n}}",
        task.key.as_deref().unwrap_or("local"),
        task.title,
        task.url.as_deref().unwrap_or("none"),
        if task.labels.is_empty() { "none".to_string() } else { task.labels.join(", ") },
        task.description,
        task.operator_notes.as_deref().unwrap_or("none"),
        task.previous_output.as_deref().unwrap_or("none"),
    );

    let response = call_model(&config, &system_prompt, &user_prompt, 700)?;
    Ok(result_from_structured_response(response, Some(&task)))
}

pub fn chat_ai_worker(
    config: AiWorkerConfig,
    request: AiWorkerChatRequest,
) -> Result<AiWorkerChatResult, String> {
    let message = request.message.trim();
    if message.is_empty() {
        return Err("Chat message is required.".to_string());
    }

    if config.runtime == "opencode" {
        validate_opencode_config(&config)?;
        let prompt = format!(
            "You are the Spacesly workspace chat assistant. Act like a helpful chatbot, not a strict agent executor. Use only the rules below for behavioral guardrails. Prefer the latest user message and recent session context over older board state. If the user asks for a board mutation, you may request it with a final SPACESLY_ACTIONS line, but do not invent task targets. Use session context to keep pronouns and follow-ups like 'it', 'that', and 'run it' grounded in the most recent relevant card. Keep answers concise and practical.\n\nRules:\n{}\n\nSession context:\n{}\n\nWorkspace context:\n{}\n\nUser message:\n{}",
            governance_context(&config, false),
            request.session_context.as_deref().unwrap_or("none"),
            request.terminal_context.as_deref().unwrap_or("none"),
            message,
        );
        let output = opencode_command(&config)
            .args([
                "run",
                "--model",
                config.opencode_model.trim(),
                "--format",
                "default",
            ])
            .arg("--title")
            .arg("Spacesly chat")
            .arg(prompt)
            .output()
            .map_err(|error| format!("Failed to run OpenCode chat: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        if !output.status.success() {
            return Err(format!(
                "OpenCode chat failed: {}",
                if stderr.is_empty() { stdout } else { stderr }
            ));
        }

        let response = if stdout.is_empty() { stderr } else { stdout };
        return Ok(AiWorkerChatResult { message: response });
    }

    validate_config(&config)?;
    let system_prompt = format!(
        "You are the Spacesly workspace chat assistant. Act like a helpful chatbot, not a strict agent executor. Use only the rules below for behavioral guardrails. Prefer the latest user message and recent session context over older board state. If the user asks for a board mutation, you may request it with a final SPACESLY_ACTIONS line, but do not invent task targets. Use session context to keep pronouns and follow-ups like 'it', 'that', and 'run it' grounded in the most recent relevant card. Keep answers concise and practical.\n\nRules:\n{}",
        governance_context(&config, false),
    );
    let user_prompt = format!(
        "Session context:\n{}\n\nWorkspace context:\n{}\n\nUser message:\n{}",
        request.session_context.as_deref().unwrap_or("none"),
        request.terminal_context.as_deref().unwrap_or("none"),
        message,
    );
    let response = call_model(&config, &system_prompt, &user_prompt, 550)?;

    Ok(AiWorkerChatResult { message: response })
}

fn validate_config(config: &AiWorkerConfig) -> Result<(), String> {
    if config.base_url.trim().is_empty() {
        return Err("Agent base URL is required.".to_string());
    }

    if config.api_key.trim().is_empty() {
        return Err("Agent API key is required.".to_string());
    }

    if config.model.trim().is_empty() {
        return Err("Agent model is required.".to_string());
    }

    Ok(())
}

fn validate_opencode_config(config: &AiWorkerConfig) -> Result<(), String> {
    if config.opencode_command.trim().is_empty() {
        return Err("OpenCode command is required.".to_string());
    }

    if config.opencode_model.trim().is_empty() {
        return Err("OpenCode model is required.".to_string());
    }

    Ok(())
}

fn governance_context(config: &AiWorkerConfig, include_skills: bool) -> String {
    let mut sections = Vec::new();
    let rules = config.agent_rules.trim();
    let skills = config.agent_skills.trim();

    if !rules.is_empty() {
        sections.push(if include_skills {
            format!(
                "User-defined Agent rules. These are mandatory operating constraints. Follow every applicable rule exactly. If a requested action conflicts with these rules or system safety, stop and return STATUS: BLOCKED with the conflict:\n{rules}"
            )
        } else {
            format!(
                "User-defined chat rules. These are mandatory operating constraints. Follow every applicable rule exactly. If a requested action conflicts with these rules or safety, explain the conflict briefly instead of taking action:\n{rules}"
            )
        });
    }

    if include_skills && !skills.is_empty() {
        sections.push(format!(
            "User-defined Agent skills/playbooks. Before acting, identify any skill that matches the task, then follow that skill as the required procedure for the matching work. If no skill applies, say so briefly in DETAILS. If a skill cannot be followed because tools/access are missing, return STATUS: BLOCKED:\n{skills}"
        ));
    }

    if sections.is_empty() {
        if include_skills {
            "No additional user-defined rules or skills configured.".to_string()
        } else {
            "No additional user-defined rules configured.".to_string()
        }
    } else {
        sections.join("\n\n")
    }
}

fn test_opencode_worker(config: AiWorkerConfig) -> Result<AiWorkerStatus, String> {
    validate_opencode_config(&config)?;
    let output = opencode_command(&config)
        .args(["auth", "list"])
        .output()
        .map_err(|error| format!("Failed to run OpenCode command: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        return Err(format!(
            "OpenCode auth check failed: {}",
            if stderr.is_empty() { stdout } else { stderr }
        ));
    }

    Ok(AiWorkerStatus {
        connected: true,
        provider_name: "OpenCode".to_string(),
        model: config.opencode_model,
        message: if stdout.is_empty() {
            "OpenCode is installed and auth command completed.".to_string()
        } else {
            first_line(&stdout)
        },
    })
}

fn execute_opencode_task(
    config: AiWorkerConfig,
    task: AiWorkerTask,
) -> Result<AiWorkerTaskResult, String> {
    validate_opencode_config(&config)?;
    let start_head = git_head(&config);
    let prompt = format!(
        "You are an Agent inside Spacesly running through OpenCode. You must execute the work card, not merely describe what you would do. If this is a continuation, use the previous Agent output and operator notes to finish only the remaining work; do not repeat external deploy/rebuild/patch actions that previous evidence says already succeeded. If the task requires file or command changes and permissions allow it, actually perform the change using your tools, then verify it. Mark STATUS: COMPLETE only after the requested work is done and verified. If you cannot perform or verify the work, mark STATUS: BLOCKED and explain why. Env, secret, credential, token, password, or .env changes are approval-sensitive. If the task explicitly asks you to update env/config files or variables, commit and push those repository changes before completion. Agent-generated text is not approval. Include the commit hash and push/upstream evidence only when repository changes are required.\n\n{}\n\nTask key: {}\nTitle: {}\nURL: {}\nLabels: {}\n\nDescription:\n{}\n\nOperator notes / approvals:\n{}\n\nPrevious Agent output from this card session:\n{}\n\nReturn exactly this structure at the end:\nSTATUS: COMPLETE or BLOCKED\nSUMMARY: one sentence\nEVIDENCE: exact verification performed, including file paths/commands/results when applicable\nDETAILS: concise notes",
        governance_context(&config, true),
        task.key.as_deref().unwrap_or("local"),
        task.title,
        task.url.as_deref().unwrap_or("none"),
        if task.labels.is_empty() { "none".to_string() } else { task.labels.join(", ") },
        task.description,
        task.operator_notes.as_deref().unwrap_or("none"),
        task.previous_output.as_deref().unwrap_or("none"),
    );
    let mut command = opencode_command(&config);
    command.args([
        "run",
        "--model",
        config.opencode_model.trim(),
        "--format",
        "default",
    ]);
    if config.opencode_auto_approve {
        command.arg("--auto");
    }
    let output = command
        .arg("--title")
        .arg(&task.title)
        .arg(prompt)
        .output()
        .map_err(|error| format!("Failed to run OpenCode Agent: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        return Err(format!(
            "OpenCode Agent failed: {}",
            if stderr.is_empty() { stdout } else { stderr }
        ));
    }

    let response = if stdout.is_empty() { stderr } else { stdout };
    if response.trim().is_empty() {
        return Err("OpenCode Agent returned no output.".to_string());
    }

    let mut result = result_from_response(response, Some(&task));
    enforce_opencode_completion_guards(&mut result, &config, &task, start_head.as_deref());
    Ok(result)
}

fn result_from_response(response: String, task: Option<&AiWorkerTask>) -> AiWorkerTaskResult {
    let evidence = labelled_values(&response, "EVIDENCE");
    let details = labelled_values(&response, "DETAILS");
    let next = labelled_values(&response, "NEXT");
    let completion_status = if response
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("STATUS: COMPLETE"))
        && !missing_sensitive_approval(task)
    {
        AiWorkerCompletionStatus::Completed
    } else {
        AiWorkerCompletionStatus::Blocked
    };
    let summary = labelled_value(&response, "SUMMARY").unwrap_or_else(|| first_line(&response));
    let blocked_reason = if completion_status == AiWorkerCompletionStatus::Blocked {
        Some(if missing_sensitive_approval(task) {
            completion_guard_reason(task)
        } else {
            labelled_value(&response, "DETAILS")
                .or_else(|| labelled_value(&response, "EVIDENCE"))
                .unwrap_or_else(|| completion_guard_reason(task))
        })
    } else {
        None
    };

    AiWorkerTaskResult {
        summary,
        evidence,
        details,
        next,
        completion_status,
        blocked_reason,
    }
}

fn result_from_structured_response(
    response: String,
    task: Option<&AiWorkerTask>,
) -> AiWorkerTaskResult {
    match parse_structured_result(&response) {
        Ok(mut result) => {
            if result.completion_status == AiWorkerCompletionStatus::Completed
                && missing_sensitive_approval(task)
            {
                block_result(&mut result, completion_guard_reason(task));
            }
            result
        }
        Err(error) => invalid_structured_result(response, error),
    }
}

fn parse_structured_result(response: &str) -> Result<AiWorkerTaskResult, String> {
    let raw = extract_json_object(response)
        .ok_or_else(|| "response did not contain a JSON object".to_string())?;
    let parsed: StructuredAiWorkerTaskResult = serde_json::from_str(raw)
        .map_err(|error| format!("failed to parse JSON result: {error}"))?;
    let completion_status = match parsed.completion_status.trim().to_lowercase().as_str() {
        "completed" | "complete" => AiWorkerCompletionStatus::Completed,
        "blocked" | "block" => AiWorkerCompletionStatus::Blocked,
        other => return Err(format!("invalid completion_status: {other}")),
    };
    let summary = parsed.summary.trim().to_string();
    if summary.is_empty() {
        return Err("summary is required".to_string());
    }
    if completion_status == AiWorkerCompletionStatus::Blocked
        && parsed
            .blocked_reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .is_none()
    {
        return Err("blocked_reason is required when completion_status is blocked".to_string());
    }

    Ok(AiWorkerTaskResult {
        summary,
        evidence: clean_result_lines(parsed.evidence),
        details: clean_result_lines(parsed.details),
        next: clean_result_lines(parsed.next),
        completion_status,
        blocked_reason: parsed
            .blocked_reason
            .map(|reason| reason.trim().to_string())
            .filter(|reason| !reason.is_empty()),
    })
}

fn extract_json_object(response: &str) -> Option<&str> {
    let trimmed = response.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }

    trimmed
        .find('{')
        .and_then(|start| trimmed.rfind('}').map(|end| (start, end)))
        .filter(|(start, end)| start < end)
        .map(|(start, end)| &trimmed[start..=end])
}

fn clean_result_lines(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .take(12)
        .collect()
}

fn invalid_structured_result(response: String, error: String) -> AiWorkerTaskResult {
    let raw = first_line(&response);
    let detail = if raw.is_empty() {
        error.clone()
    } else {
        format!("{error}. First response line: {raw}")
    };

    AiWorkerTaskResult {
        summary: "Agent returned an invalid structured result.".to_string(),
        evidence: Vec::new(),
        details: vec![detail],
        next: vec![
            "Retry the Agent or switch to a runtime that can return the required JSON result."
                .to_string(),
        ],
        completion_status: AiWorkerCompletionStatus::Blocked,
        blocked_reason: Some(error),
    }
}

fn completion_guard_reason(task: Option<&AiWorkerTask>) -> String {
    if missing_sensitive_approval(task) {
        return "Task touches env/secrets/credentials and needs explicit operator approval evidence before it can be marked Done.".to_string();
    }

    "Agent did not provide STATUS: COMPLETE with verification evidence.".to_string()
}

fn missing_sensitive_approval(task: Option<&AiWorkerTask>) -> bool {
    let Some(task) = task else {
        return false;
    };
    if !task_requires_sensitive_approval(task) {
        return false;
    }

    !has_operator_approval(task)
}

fn has_operator_approval(task: &AiWorkerTask) -> bool {
    let notes = task.operator_notes.as_deref().unwrap_or("").to_lowercase();
    notes.contains("approve") || notes.contains("approved") || notes.contains("approval granted")
}

fn task_requires_sensitive_approval(task: &AiWorkerTask) -> bool {
    let text = format!(
        "{}\n{}\n{}",
        task.title,
        task.description,
        task.labels.join(" ")
    )
    .to_lowercase();
    [
        ".env",
        " env",
        "environment variable",
        "secret",
        "credential",
        "api key",
        "token",
        "password",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn enforce_opencode_completion_guards(
    result: &mut AiWorkerTaskResult,
    config: &AiWorkerConfig,
    task: &AiWorkerTask,
    start_head: Option<&str>,
) {
    if result.completion_status != AiWorkerCompletionStatus::Completed {
        return;
    }

    if task_requires_env_update_commit(task) {
        if let Some(reason) = dirty_worktree_reason(config) {
            block_result(result, reason);
            return;
        }

        if let Some(reason) = missing_new_commit_reason(config, start_head) {
            block_result(result, reason);
            return;
        }
    }

    if task_requires_env_update_commit(task) || task_requires_push(task) {
        if let Some(reason) = unpushed_commits_reason(config) {
            block_result(result, reason);
        }
    }
}

fn block_result(result: &mut AiWorkerTaskResult, reason: String) {
    result.completion_status = AiWorkerCompletionStatus::Blocked;
    result.blocked_reason = Some(reason.clone());
    result.summary = reason.clone();
    result.details = vec![reason];
    result.next = vec!["Resolve the blocker, then continue the Agent.".to_string()];
}

fn task_requires_push(task: &AiWorkerTask) -> bool {
    let text = format!("{}\n{}", task.title, task.description).to_lowercase();
    text.contains("push")
        || text.contains("merge request")
        || text.contains("pull request")
        || task_requires_env_update_commit(task)
}

fn task_requires_env_update_commit(task: &AiWorkerTask) -> bool {
    let text = format!(
        "{}\n{}\n{}",
        task.title,
        task.description,
        task.labels.join(" ")
    )
    .to_lowercase();

    let has_update_verb = ["update", "change", "modify", "edit", "add", "remove", "set"]
        .iter()
        .any(|needle| text.contains(needle));

    let has_env_target = [
        ".env",
        "env variable",
        "environment variable",
        "environment config",
        "env config",
        "values.yaml",
        "values yml",
        "helm values",
        "deployment template",
        "deployment-config",
        "deployment config",
        "configmap",
        "secret.yaml",
        "secret yml",
    ]
    .iter()
    .any(|needle| text.contains(needle));

    has_update_verb && has_env_target
}

fn dirty_worktree_reason(config: &AiWorkerConfig) -> Option<String> {
    let status = git_output(config, ["status", "--porcelain"])?;
    if status.trim().is_empty() {
        None
    } else {
        Some("Agent left uncommitted file changes. Review/approve the changes, then commit or retry before marking Done.".to_string())
    }
}

fn unpushed_commits_reason(config: &AiWorkerConfig) -> Option<String> {
    let Some(upstream) = git_output(
        config,
        ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    ) else {
        return Some("Agent completed local repository changes, but the branch has no upstream. Push to a remote branch before marking Jira Done.".to_string());
    };
    if upstream.trim().is_empty() {
        return Some("Agent completed local repository changes, but the branch has no upstream. Push to a remote branch before marking Jira Done.".to_string());
    }

    if git_status(config, ["merge-base", "--is-ancestor", "HEAD", "@{u}"])? {
        None
    } else {
        Some("Agent completed local repository changes, but the latest commit is not pushed to upstream. Push the commit before marking Jira Done.".to_string())
    }
}

fn missing_new_commit_reason(config: &AiWorkerConfig, start_head: Option<&str>) -> Option<String> {
    let Some(start_head) = start_head else {
        return Some("Spacesly could not capture the starting git commit. Commit and push the repository changes before marking Jira Done.".to_string());
    };
    let current_head = git_head(config)?;
    if current_head.trim() == start_head.trim() {
        Some("Agent did not create a new commit for the repository change. Commit and push the Helm/env/template update before marking Jira Done.".to_string())
    } else {
        None
    }
}

fn git_head(config: &AiWorkerConfig) -> Option<String> {
    git_output(config, ["rev-parse", "HEAD"]).map(|value| value.trim().to_string())
}

fn git_output<const N: usize>(config: &AiWorkerConfig, args: [&str; N]) -> Option<String> {
    let mut command = Command::new("git");
    command.args(args);
    command.current_dir(opencode_workdir(config)?);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_status<const N: usize>(config: &AiWorkerConfig, args: [&str; N]) -> Option<bool> {
    let mut command = Command::new("git");
    command.args(args);
    command.current_dir(opencode_workdir(config)?);
    command.output().ok().map(|output| output.status.success())
}

fn opencode_workdir(config: &AiWorkerConfig) -> Option<PathBuf> {
    config
        .opencode_workdir
        .as_deref()
        .map(str::trim)
        .filter(|workdir| !workdir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
}

fn labelled_value(response: &str, label: &str) -> Option<String> {
    let prefix = format!("{label}:");
    response
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix).map(str::trim))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn labelled_values(response: &str, label: &str) -> Vec<String> {
    let prefix = format!("{label}:");
    let mut collecting = false;
    let mut values = Vec::new();

    for raw_line in response.lines() {
        let line = raw_line.trim();
        if let Some(value) = line.strip_prefix(&prefix).map(str::trim) {
            collecting = true;
            if !value.is_empty() {
                values.push(clean_labelled_line(value));
            }
            continue;
        }

        if !collecting {
            continue;
        }

        if let Some((name, _)) = line.split_once(':') {
            if matches!(
                name.trim().to_uppercase().as_str(),
                "STATUS" | "SUMMARY" | "EVIDENCE" | "DETAILS" | "NEXT"
            ) {
                break;
            }
        }

        if !line.is_empty() {
            values.push(clean_labelled_line(line));
        }
    }

    values
        .into_iter()
        .filter(|value| !value.is_empty())
        .take(12)
        .collect()
}

fn clean_labelled_line(value: &str) -> String {
    value
        .trim_start_matches(|ch| ch == '-' || ch == '*')
        .trim()
        .to_string()
}

fn opencode_command(config: &AiWorkerConfig) -> Command {
    let mut command = Command::new(config.opencode_command.trim());
    inject_shell_env(&mut command);
    command.stdin(Stdio::null());

    if let Some(workdir) = config
        .opencode_workdir
        .as_deref()
        .map(str::trim)
        .filter(|workdir| !workdir.is_empty())
    {
        command.current_dir(workdir);
    }

    command
}

fn call_model(
    config: &AiWorkerConfig,
    system_prompt: &str,
    user_prompt: &str,
    max_tokens: u32,
) -> Result<String, String> {
    match config.api_style.as_str() {
        "openai_responses" => call_openai_responses(config, system_prompt, user_prompt, max_tokens),
        "anthropic_messages" => {
            call_anthropic_messages(config, system_prompt, user_prompt, max_tokens)
        }
        _ => call_chat_completion(config, system_prompt, user_prompt, max_tokens),
    }
}

fn call_openai_responses(
    config: &AiWorkerConfig,
    system_prompt: &str,
    user_prompt: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("Failed to create Agent HTTP client: {error}"))?;
    let endpoint = responses_endpoint(&config.base_url);
    let body = serde_json::json!({
        "model": config.model,
        "max_output_tokens": max_tokens,
        "input": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_prompt }
        ]
    });

    let response = client
        .post(endpoint)
        .bearer_auth(config.api_key.trim())
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .map_err(|error| format!("Failed to call Agent. {}", describe_reqwest_error(&error)))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|error| format!("Failed to read Agent response: {error}"))?;

    if !status.is_success() {
        return Err(format!("Agent returned HTTP {status}: {text}"));
    }

    let parsed: OpenAiResponsesResponse = serde_json::from_str(&text)
        .map_err(|error| format!("Failed to parse Agent response: {error}. Body: {text}"))?;
    let content = parsed
        .output_text
        .or_else(|| {
            parsed.output.map(|output| {
                output
                    .into_iter()
                    .flat_map(|item| item.content.unwrap_or_default())
                    .filter(|item| {
                        item.content_type == "output_text" || item.content_type == "text"
                    })
                    .filter_map(|item| item.text)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        })
        .unwrap_or_default()
        .trim()
        .to_string();

    if content.is_empty() {
        Err("Agent returned no message content.".to_string())
    } else {
        Ok(content)
    }
}

fn call_chat_completion(
    config: &AiWorkerConfig,
    system_prompt: &str,
    user_prompt: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("Failed to create Agent HTTP client: {error}"))?;
    let endpoint = chat_endpoint(&config.base_url);
    let body = serde_json::json!({
        "model": config.model,
        "temperature": config.temperature.clamp(0.0, 2.0),
        "max_tokens": max_tokens,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_prompt }
        ]
    });

    let response = client
        .post(endpoint)
        .bearer_auth(config.api_key.trim())
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .map_err(|error| format!("Failed to call Agent. {}", describe_reqwest_error(&error)))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|error| format!("Failed to read Agent response: {error}"))?;

    if !status.is_success() {
        return Err(format!("Agent returned HTTP {status}: {text}"));
    }

    let parsed: ChatCompletionResponse = serde_json::from_str(&text)
        .map_err(|error| format!("Failed to parse Agent response: {error}. Body: {text}"))?;
    parsed
        .choices
        .into_iter()
        .find_map(|choice| choice.message.content)
        .map(|content| content.trim().to_string())
        .filter(|content| !content.is_empty())
        .ok_or_else(|| "Agent returned no message content.".to_string())
}

fn call_anthropic_messages(
    config: &AiWorkerConfig,
    system_prompt: &str,
    user_prompt: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("Failed to create Agent HTTP client: {error}"))?;
    let endpoint = anthropic_endpoint(&config.base_url);
    let body = serde_json::json!({
        "model": config.model,
        "system": system_prompt,
        "temperature": config.temperature.clamp(0.0, 1.0),
        "max_tokens": max_tokens,
        "messages": [
            { "role": "user", "content": user_prompt }
        ]
    });

    let response = client
        .post(endpoint)
        .header("x-api-key", config.api_key.trim())
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .map_err(|error| format!("Failed to call Agent. {}", describe_reqwest_error(&error)))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|error| format!("Failed to read Agent response: {error}"))?;

    if !status.is_success() {
        return Err(format!("Agent returned HTTP {status}: {text}"));
    }

    let parsed: AnthropicMessageResponse = serde_json::from_str(&text)
        .map_err(|error| format!("Failed to parse Agent response: {error}. Body: {text}"))?;
    let content = parsed
        .content
        .into_iter()
        .filter(|block| block.block_type == "text")
        .filter_map(|block| block.text)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if content.is_empty() {
        Err("Agent returned no message content.".to_string())
    } else {
        Ok(content)
    }
}

fn chat_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

fn responses_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/responses") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/responses")
    }
}

fn anthropic_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/messages") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/messages")
    }
}

fn provider_label(config: &AiWorkerConfig) -> String {
    let label = config.provider_name.trim();
    if label.is_empty() {
        "Agent provider".to_string()
    } else {
        label.to_string()
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(text)
        .trim()
        .chars()
        .take(240)
        .collect()
}

fn describe_reqwest_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "The request timed out after 30 seconds. Check the model provider, VPN/proxy, or use a faster model.".to_string()
    } else if error.is_connect() {
        "Could not connect to the model provider. Check base URL, VPN/proxy, or network access."
            .to_string()
    } else {
        error.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_governance(rules: &str, skills: &str) -> AiWorkerConfig {
        AiWorkerConfig {
            runtime: "api".to_string(),
            provider_name: "OpenAI".to_string(),
            base_url: "https://example.invalid".to_string(),
            api_style: "openai_chat".to_string(),
            api_key: "token".to_string(),
            model: "gpt-5.5".to_string(),
            opencode_command: "opencode".to_string(),
            opencode_model: "openai/gpt-5.5".to_string(),
            opencode_workdir: None,
            opencode_auto_approve: false,
            agent_rules: rules.to_string(),
            agent_skills: skills.to_string(),
            temperature: 0.2,
        }
    }

    #[test]
    fn governance_context_marks_rules_as_mandatory() {
        let context = governance_context(&config_with_governance("Never guess.", ""), true);

        assert!(context.contains("mandatory operating constraints"));
        assert!(context.contains("Never guess."));
    }

    #[test]
    fn governance_context_marks_skills_as_required_procedures() {
        let context = governance_context(&config_with_governance("", "Skill: Deploy safely"), true);

        assert!(context.contains("required procedure"));
        assert!(context.contains("Skill: Deploy safely"));
    }

    #[test]
    fn chat_governance_context_excludes_skills() {
        let context = governance_context(
            &config_with_governance("Never guess.", "Skill: Deploy safely"),
            false,
        );

        assert!(context.contains("Never guess."));
        assert!(!context.contains("Skill: Deploy safely"));
    }

    #[test]
    fn task_result_serializes_completion_status_for_ipc() {
        let value = serde_json::to_value(AiWorkerTaskResult {
            summary: "Done".to_string(),
            evidence: vec!["Checked output".to_string()],
            details: Vec::new(),
            next: Vec::new(),
            completion_status: AiWorkerCompletionStatus::Completed,
            blocked_reason: None,
        })
        .expect("serialize task result");

        assert_eq!(value["completion_status"].as_str(), Some("completed"));
    }

    #[test]
    fn structured_result_parses_completed_json() {
        let result = result_from_structured_response(
            r#"{
              "completion_status": "completed",
              "summary": "Queue summarized.",
              "evidence": ["Read visible board state"],
              "details": ["No external tools required"],
              "next": [],
              "blocked_reason": null
            }"#
            .to_string(),
            None,
        );

        assert_eq!(
            result.completion_status,
            AiWorkerCompletionStatus::Completed
        );
        assert_eq!(result.summary, "Queue summarized.");
        assert_eq!(result.evidence, vec!["Read visible board state"]);
        assert_eq!(result.blocked_reason, None);
    }

    #[test]
    fn structured_result_blocks_invalid_json() {
        let result = result_from_structured_response("STATUS: COMPLETE".to_string(), None);

        assert_eq!(result.completion_status, AiWorkerCompletionStatus::Blocked);
        assert_eq!(
            result.summary,
            "Agent returned an invalid structured result."
        );
        assert!(result
            .blocked_reason
            .as_deref()
            .unwrap_or_default()
            .contains("response did not contain a JSON object"));
    }

    #[test]
    fn structured_result_requires_blocked_reason() {
        let result = result_from_structured_response(
            r#"{
              "completion_status": "blocked",
              "summary": "Cannot access Jira.",
              "evidence": [],
              "details": [],
              "next": [],
              "blocked_reason": null
            }"#
            .to_string(),
            None,
        );

        assert_eq!(result.completion_status, AiWorkerCompletionStatus::Blocked);
        assert!(result
            .blocked_reason
            .as_deref()
            .unwrap_or_default()
            .contains("blocked_reason is required"));
    }

    #[test]
    fn structured_result_keeps_sensitive_task_blocked_without_approval() {
        let task = AiWorkerTask {
            key: Some("SEC-1".to_string()),
            title: "Update API token".to_string(),
            description: "Change secret token handling.".to_string(),
            labels: Vec::new(),
            url: None,
            operator_notes: None,
            previous_output: None,
        };

        let result = result_from_structured_response(
            r#"{
              "completion_status": "completed",
              "summary": "Updated token handling.",
              "evidence": ["Reasoned about the change"],
              "details": [],
              "next": [],
              "blocked_reason": null
            }"#
            .to_string(),
            Some(&task),
        );

        assert_eq!(result.completion_status, AiWorkerCompletionStatus::Blocked);
        assert!(result
            .blocked_reason
            .as_deref()
            .unwrap_or_default()
            .contains("needs explicit operator approval"));
    }

    #[test]
    fn env_update_tasks_require_commit_and_push() {
        let task = AiWorkerTask {
            key: Some("QCASH-1".to_string()),
            title: "Update env variable in qcash-deployment Helm template".to_string(),
            description: "Change values.yaml for the prerelease chart.".to_string(),
            labels: vec!["deployment".to_string()],
            url: None,
            operator_notes: Some("approval granted".to_string()),
            previous_output: None,
        };

        assert!(task_requires_env_update_commit(&task));
        assert!(task_requires_push(&task));
    }

    #[test]
    fn redeploy_only_tasks_do_not_require_commit_or_push() {
        let task = AiWorkerTask {
            key: Some("QCASH-2".to_string()),
            title: "Redeploy service".to_string(),
            description: "Redeploy qcash-deployment after the release is available.".to_string(),
            labels: vec!["deployment".to_string()],
            url: None,
            operator_notes: None,
            previous_output: None,
        };

        assert!(!task_requires_env_update_commit(&task));
        assert!(!task_requires_push(&task));
    }

    #[test]
    fn non_repo_chat_task_does_not_require_push() {
        let task = AiWorkerTask {
            key: Some("QCASH-2".to_string()),
            title: "Explain current queue".to_string(),
            description: "Summarize the visible board state.".to_string(),
            labels: Vec::new(),
            url: None,
            operator_notes: None,
            previous_output: None,
        };

        assert!(!task_requires_env_update_commit(&task));
        assert!(!task_requires_push(&task));
    }
}
