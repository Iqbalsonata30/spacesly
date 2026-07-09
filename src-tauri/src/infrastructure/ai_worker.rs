use super::shell_env::inject_shell_env;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
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
}

#[derive(Clone, Debug, Deserialize)]
pub struct AiWorkerChatRequest {
    pub message: String,
    pub terminal_context: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AiWorkerStatus {
    pub connected: bool,
    pub provider_name: String,
    pub model: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AiWorkerResult {
    pub summary: String,
    pub raw_response: String,
    pub completion_status: String,
    pub blocked_reason: Option<String>,
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
) -> Result<AiWorkerResult, String> {
    if config.runtime == "opencode" {
        return execute_opencode_task(config, task);
    }

    validate_config(&config)?;

    let system_prompt = format!(
        "You are an Agent inside Spacesly, an orchestration app for human and AI agents. You receive Jira-style work cards and produce a concrete execution result. This direct API runtime does not have filesystem, shell, browser, Jira, Kubernetes, Bamboo, or MCP tools. Mark STATUS: COMPLETE only for reasoning/reporting tasks that require no external side effects. If the work requires changing files, running commands, checking external systems, or using unavailable credentials/tools, mark STATUS: BLOCKED and explain what runtime/tool is needed.\n\n{}",
        governance_context(&config),
    );
    let user_prompt = format!(
        "Task key: {}\nTitle: {}\nURL: {}\nLabels: {}\n\nDescription:\n{}\n\nReturn exactly this structure:\nSTATUS: COMPLETE or BLOCKED\nSUMMARY: one sentence\nEVIDENCE: what was actually verified\nDETAILS: concise notes",
        task.key.as_deref().unwrap_or("local"),
        task.title,
        task.url.as_deref().unwrap_or("none"),
        if task.labels.is_empty() { "none".to_string() } else { task.labels.join(", ") },
        task.description,
    );

    let response = call_model(&config, &system_prompt, &user_prompt, 700)?;
    Ok(result_from_response(response))
}

pub fn chat_ai_worker(
    config: AiWorkerConfig,
    request: AiWorkerChatRequest,
) -> Result<AiWorkerResult, String> {
    let message = request.message.trim();
    if message.is_empty() {
        return Err("Chat message is required.".to_string());
    }

    if config.runtime == "opencode" {
        validate_opencode_config(&config)?;
        let prompt = format!(
            "You are the Spacesly workspace Agent chat. You can help the user reason about the workspace and you may request controlled board mutations by appending a final line exactly like: SPACESLY_ACTIONS: [{{\"type\":\"create_task\",\"title\":\"...\",\"description\":\"...\"}}]. Supported action types are create_task, move_card, start_agent, select_card, delete_card, and sync_jira. Use card_id from board context when possible. For move_card target must be todo, ready, in_progress, or done. Do not claim an action happened unless you include it in SPACESLY_ACTIONS. Keep the human-readable response under 120 words.\n\n{}\n\nWorkspace context:\n{}\n\nUser message:\n{}",
            governance_context(&config),
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
        return Ok(AiWorkerResult {
            summary: first_line(&response),
            raw_response: response,
            completion_status: "completed".to_string(),
            blocked_reason: None,
        });
    }

    validate_config(&config)?;
    let system_prompt = format!(
        "You are the Spacesly workspace Agent chat. Help the user reason about tasks, shell output, local commands, Jira work, and Agent execution. You may request controlled Spacesly board mutations by appending a final line exactly like: SPACESLY_ACTIONS: [{{\"type\":\"create_task\",\"title\":\"...\",\"description\":\"...\"}}]. Supported action types are create_task, move_card, start_agent, select_card, delete_card, and sync_jira. Use card_id from board context when possible. For move_card target must be todo, ready, in_progress, or done. Do not claim an action happened unless you include it in SPACESLY_ACTIONS. Keep the human-readable response under 120 words.\n\n{}",
        governance_context(&config),
    );
    let user_prompt = format!(
        "Workspace context:\n{}\n\nUser message:\n{}",
        request.terminal_context.as_deref().unwrap_or("none"),
        message,
    );
    let response = call_model(&config, &system_prompt, &user_prompt, 550)?;

    Ok(AiWorkerResult {
        summary: first_line(&response),
        raw_response: response,
        completion_status: "completed".to_string(),
        blocked_reason: None,
    })
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

fn governance_context(config: &AiWorkerConfig) -> String {
    let mut sections = Vec::new();
    let rules = config.agent_rules.trim();
    let skills = config.agent_skills.trim();

    if !rules.is_empty() {
        sections.push(format!(
            "User-defined Agent rules. These are mandatory and override generic behavior unless they conflict with system safety:\n{rules}"
        ));
    }

    if !skills.is_empty() {
        sections.push(format!(
            "User-defined Agent skills. Apply relevant skills when the task matches their domain:\n{skills}"
        ));
    }

    if sections.is_empty() {
        "No additional user-defined rules or skills configured.".to_string()
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
) -> Result<AiWorkerResult, String> {
    validate_opencode_config(&config)?;
    let prompt = format!(
        "You are an Agent inside Spacesly running through OpenCode. You must execute the work card, not merely describe what you would do. If the task requires file or command changes and permissions allow it, actually perform the change using your tools, then verify it. Mark STATUS: COMPLETE only after the requested work is done and verified. If you cannot perform or verify the work, mark STATUS: BLOCKED and explain why.\n\n{}\n\nTask key: {}\nTitle: {}\nURL: {}\nLabels: {}\n\nDescription:\n{}\n\nReturn exactly this structure at the end:\nSTATUS: COMPLETE or BLOCKED\nSUMMARY: one sentence\nEVIDENCE: exact verification performed, including file paths/commands/results when applicable\nDETAILS: concise notes",
        governance_context(&config),
        task.key.as_deref().unwrap_or("local"),
        task.title,
        task.url.as_deref().unwrap_or("none"),
        if task.labels.is_empty() { "none".to_string() } else { task.labels.join(", ") },
        task.description,
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
        .arg(task.title)
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

    Ok(result_from_response(response))
}

fn result_from_response(response: String) -> AiWorkerResult {
    let completion_status = if response
        .lines()
        .any(|line| line.trim().eq_ignore_ascii_case("STATUS: COMPLETE"))
    {
        "completed"
    } else {
        "blocked"
    };
    let summary = labelled_value(&response, "SUMMARY").unwrap_or_else(|| first_line(&response));
    let blocked_reason = if completion_status == "blocked" {
        Some(
            labelled_value(&response, "DETAILS")
                .or_else(|| labelled_value(&response, "EVIDENCE"))
                .unwrap_or_else(|| {
                    "Agent did not provide STATUS: COMPLETE with verification evidence.".to_string()
                }),
        )
    } else {
        None
    };

    AiWorkerResult {
        summary,
        raw_response: response,
        completion_status: completion_status.to_string(),
        blocked_reason,
    }
}

fn labelled_value(response: &str, label: &str) -> Option<String> {
    let prefix = format!("{label}:");
    response
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix).map(str::trim))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
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
