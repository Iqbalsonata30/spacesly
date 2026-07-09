use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::jira_rest;
use super::shell_env::inject_shell_env;

/// Jira issue data returned from a Jira MCP server.
#[derive(Clone, Debug, Serialize)]
pub struct JiraIssue {
    pub key: String,
    pub summary: String,
    pub description: Option<String>,
    pub status: String,
    pub issue_type: String,
    pub url: Option<String>,
    pub labels: Vec<String>,
}

/// Jira agile board exposed by a Jira MCP server.
#[derive(Clone, Debug, Serialize)]
pub struct JiraBoard {
    pub id: String,
    pub name: String,
    pub board_type: String,
}

/// Result of validating a Jira MCP connector.
#[derive(Clone, Debug, Serialize)]
pub struct JiraConnectionStatus {
    pub tool_count: usize,
    pub issue_count: usize,
    pub board_count: usize,
    pub tools: Vec<String>,
}

/// Result of validating a generic MCP connector.
#[derive(Clone, Debug, Serialize)]
pub struct McpConnectionStatus {
    pub tool_count: usize,
    pub tools: Vec<String>,
}

/// Configuration for a Jira MCP stdio connector.
#[derive(Clone, Debug, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Validates any stdio MCP server by initializing it and listing tools.
pub fn test_mcp_connection(server: McpServerConfig) -> Result<McpConnectionStatus, String> {
    let config = JiraMcpConfig {
        server,
        auth: JiraAuthConfig {
            base_url: String::new(),
            auth_mode: "api_token".to_string(),
            username: String::new(),
            api_token: String::new(),
            personal_access_token: String::new(),
            password: String::new(),
        },
        tool_name: "unused".to_string(),
        board_tool_name: default_board_tool_name(),
        board_issues_tool_name: default_board_issues_tool_name(),
        jql: String::new(),
        board_id: None,
        project_key: None,
        board_name: None,
        page_size: default_page_size(),
        max_pages: default_max_pages(),
    };
    let mut client = StdioMcpClient::start(&config)?;
    client.initialize()?;
    let tools = client.list_tools()?;

    Ok(McpConnectionStatus {
        tool_count: tools.len(),
        tools,
    })
}

/// Request sent by the UI when syncing Jira through an MCP server.
#[derive(Clone, Debug, Deserialize)]
pub struct JiraMcpConfig {
    pub server: McpServerConfig,
    pub auth: JiraAuthConfig,
    pub tool_name: String,
    #[serde(default = "default_board_tool_name")]
    pub board_tool_name: String,
    #[serde(default = "default_board_issues_tool_name")]
    pub board_issues_tool_name: String,
    pub jql: String,
    #[serde(default)]
    pub board_id: Option<String>,
    #[serde(default)]
    pub project_key: Option<String>,
    #[serde(default)]
    pub board_name: Option<String>,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    #[serde(default = "default_max_pages")]
    pub max_pages: u32,
}

fn default_page_size() -> u32 {
    25
}

fn default_max_pages() -> u32 {
    1
}

/// Jira credentials supplied by the UI.
#[derive(Clone, Debug, Deserialize)]
pub struct JiraAuthConfig {
    pub base_url: String,
    pub auth_mode: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub api_token: String,
    #[serde(default)]
    pub personal_access_token: String,
    #[serde(default)]
    pub password: String,
}

fn default_board_tool_name() -> String {
    "jira_get_agile_boards".to_string()
}

fn default_board_issues_tool_name() -> String {
    "jira_get_board_issues".to_string()
}

/// Validates a Jira MCP server by listing tools and making small Jira calls.
pub fn test_jira_connection(config: JiraMcpConfig) -> Result<JiraConnectionStatus, String> {
    let mut client = StdioMcpClient::start(&config)?;
    client.initialize()?;

    let tools = client.list_tools()?;
    let search_tool = resolve_tool(&tools, &config.tool_name)?;

    let issues_result = client.call_tool(
        &search_tool,
        json!({
            "jql": config.jql,
            "fields": "key,summary,status,issuetype",
            "limit": 1
        }),
    )?;
    let issue_count = parse_jira_issues(&issues_result)
        .map(|issues| issues.len())
        .unwrap_or(0);

    let board_count = resolve_tool(&tools, &config.board_tool_name)
        .ok()
        .and_then(|board_tool| fetch_boards_with_fallbacks(&mut client, &board_tool, &config).ok())
        .or_else(|| {
            jira_rest::fetch_boards(
                &config.auth,
                config.project_key.as_deref(),
                config.board_name.as_deref(),
            )
            .ok()
        })
        .map(|boards| boards.len())
        .unwrap_or(0);

    Ok(JiraConnectionStatus {
        tool_count: tools.len(),
        issue_count,
        board_count,
        tools,
    })
}

/// Fetches Jira agile boards through the configured MCP server.
pub fn fetch_jira_boards(config: JiraMcpConfig) -> Result<Vec<JiraBoard>, String> {
    jira_rest::fetch_boards(
        &config.auth,
        config.project_key.as_deref(),
        config.board_name.as_deref(),
    )
}

/// Fetches Jira issues through the configured MCP server.
pub fn fetch_jira_issues(config: JiraMcpConfig) -> Result<Vec<JiraIssue>, String> {
    if let Some(board_id) = config
        .board_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
    {
        return jira_rest::fetch_board_issues_paginated(
            &config.auth,
            board_id,
            &config.jql,
            config.page_size,
            config.max_pages,
        );
    }

    let mut client = StdioMcpClient::start(&config)?;

    client.initialize()?;
    let tools = client.list_tools()?;
    let (tool_name, arguments) = if let Some(board_id) = config
        .board_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
    {
        let board_issues_tool = resolve_tool(&tools, &config.board_issues_tool_name)?;
        (
            board_issues_tool,
            json!({
                "board_id": board_id,
                "jql": config.jql,
                "fields": "key,summary,description,status,issuetype",
                "start_at": 0,
                "limit": 50
            }),
        )
    } else {
        let search_tool = resolve_tool(&tools, &config.tool_name)?;
        (
            search_tool,
            json!({
                "jql": config.jql,
                "fields": "key,summary,description,status,issuetype",
                "limit": 50
            }),
        )
    };

    let result = client.call_tool(&tool_name, arguments)?;

    parse_jira_issues(&result)
}

struct StdioMcpClient {
    child: Child,
    stdin: std::process::ChildStdin,
    responses: mpsc::Receiver<Value>,
    stderr: Arc<Mutex<String>>,
    next_id: u64,
}

impl StdioMcpClient {
    fn start(config: &JiraMcpConfig) -> Result<Self, String> {
        if config.server.command.trim().is_empty() {
            return Err("MCP server command is required.".to_string());
        }

        if config.tool_name.trim().is_empty() {
            return Err("Jira MCP tool name is required.".to_string());
        }

        let mut command = Command::new(&config.server.command);
        command
            .args(&config.server.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        inject_shell_env(&mut command);
        command.envs(&config.server.env);

        let mut child = command
            .spawn()
            .map_err(|error| {
                if error.kind() == ErrorKind::NotFound {
                    format!(
                        "MCP command '{}' was not found. Use a full executable path, or enter a full command line in Settings such as 'npx -y <package>'. Original error: {error}",
                        config.server.command
                    )
                } else {
                    format!("Failed to start Jira MCP server: {error}")
                }
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to open Jira MCP stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to open Jira MCP stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Failed to open Jira MCP stderr".to_string())?;
        let (tx, responses) = mpsc::channel();
        let stderr_buffer = Arc::new(Mutex::new(String::new()));

        spawn_stdout_reader(stdout, tx);
        spawn_stderr_reader(stderr, Arc::clone(&stderr_buffer));

        Ok(Self {
            child,
            stdin,
            responses,
            stderr: stderr_buffer,
            next_id: request_seed(),
        })
    }

    fn initialize(&mut self) -> Result<(), String> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "spacesly",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;
        self.notify("notifications/initialized", json!({}))
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments
            }),
        )
    }

    fn list_tools(&mut self) -> Result<Vec<String>, String> {
        let result = self.request("tools/list", json!({}))?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| "MCP server did not return a tools list".to_string())?;

        Ok(tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        self.write_message(&message)?;

        let deadline = Duration::from_secs(45);
        loop {
            let response = self.read_message(deadline)?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }

            if let Some(error) = response.get("error") {
                return Err(format!("Jira MCP request failed: {error}"));
            }

            return response
                .get("result")
                .cloned()
                .ok_or_else(|| "Jira MCP response did not include a result".to_string());
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
    }

    fn write_message(&mut self, message: &Value) -> Result<(), String> {
        let body = serde_json::to_vec(message)
            .map_err(|error| format!("Failed to serialize MCP message: {error}"))?;
        self.stdin
            .write_all(&body)
            .map_err(|error| format!("Failed to write MCP body: {error}"))?;
        self.stdin
            .write_all(b"\n")
            .map_err(|error| format!("Failed to write MCP newline: {error}"))?;
        self.stdin
            .flush()
            .map_err(|error| format!("Failed to flush MCP message: {error}"))
    }

    fn read_message(&mut self, timeout: Duration) -> Result<Value, String> {
        self.responses.recv_timeout(timeout).map_err(|error| {
            let stderr = self
                .stderr
                .lock()
                .map(|buffer| buffer.trim().to_string())
                .unwrap_or_default();
            if stderr.is_empty() {
                format!("Timed out waiting for Jira MCP response after {}s. The MCP server started but did not answer this request. Try narrowing the project/board filter or verify the selected tool arguments. Internal timeout: {error}", timeout.as_secs())
            } else {
                format!(
                    "Timed out waiting for Jira MCP response after {}s. MCP stderr: {}",
                    timeout.as_secs(),
                    stderr
                )
            }
        })
    }
}

impl Drop for StdioMcpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_stdout_reader(stdout: std::process::ChildStdout, tx: mpsc::Sender<Value>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_stdout_message(&mut reader) {
                Ok(Some(value)) => {
                    let _ = tx.send(value);
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
    });
}

fn spawn_stderr_reader(stderr: std::process::ChildStderr, buffer: Arc<Mutex<String>>) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(mut output) = buffer.lock() {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(&line);
            }
        }
    });
}

fn read_stdout_message(
    reader: &mut BufReader<std::process::ChildStdout>,
) -> Result<Option<Value>, String> {
    let mut line = String::new();
    let bytes = reader
        .read_line(&mut line)
        .map_err(|error| format!("Failed to read MCP stdout: {error}"))?;
    if bytes == 0 {
        return Ok(None);
    }

    let trimmed = line.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        return Ok(Some(
            json!({ "jsonrpc": "2.0", "method": "_spacesly_empty" }),
        ));
    }

    if let Some(value) = trimmed.strip_prefix("Content-Length:") {
        let length = value
            .trim()
            .parse::<usize>()
            .map_err(|error| format!("Invalid MCP content length: {error}"))?;
        loop {
            line.clear();
            let bytes = reader
                .read_line(&mut line)
                .map_err(|error| format!("Failed to read MCP header: {error}"))?;
            if bytes == 0 || line.trim_end_matches(['\r', '\n']).is_empty() {
                break;
            }
        }
        let mut body = vec![0; length];
        reader
            .read_exact(&mut body)
            .map_err(|error| format!("Failed to read MCP body: {error}"))?;
        return serde_json::from_slice(&body)
            .map(Some)
            .map_err(|error| format!("Failed to parse framed MCP JSON: {error}"));
    }

    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => Ok(Some(value)),
        Err(_) => Ok(Some(
            json!({ "jsonrpc": "2.0", "method": "_spacesly_log", "params": { "line": trimmed } }),
        )),
    }
}

fn request_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(1)
}

fn resolve_tool(tools: &[String], preferred: &str) -> Result<String, String> {
    let preferred = preferred.trim();
    if let Some(tool) = tools.iter().find(|tool| tool.as_str() == preferred) {
        return Ok(tool.clone());
    }
    if let Some(tool) = tools.iter().find(|tool| tool.ends_with(preferred)) {
        return Ok(tool.clone());
    }

    let suffix = preferred.strip_prefix("jira_").unwrap_or(preferred);
    if let Some(tool) = tools.iter().find(|tool| tool.ends_with(suffix)) {
        return Ok(tool.clone());
    }

    Err(format!(
        "Could not find Jira MCP tool '{preferred}'. Available tools: {}",
        tools.join(", ")
    ))
}

fn fetch_boards_with_fallbacks(
    client: &mut StdioMcpClient,
    board_tool: &str,
    config: &JiraMcpConfig,
) -> Result<Vec<JiraBoard>, String> {
    let attempts = board_list_argument_attempts(config);
    let mut last_error = String::new();

    for arguments in attempts {
        match client.call_tool(board_tool, arguments) {
            Ok(result) => match parse_jira_boards(&result) {
                Ok(boards) => return Ok(boards),
                Err(error) => last_error = error,
            },
            Err(error) => last_error = error,
        }
    }

    Err(if last_error.is_empty() {
        "Jira board tool returned no boards".to_string()
    } else {
        last_error
    })
}

fn board_list_arguments(config: &JiraMcpConfig) -> Value {
    let mut arguments = serde_json::Map::new();
    arguments.insert("start_at".to_string(), json!(0));
    arguments.insert("limit".to_string(), json!(50));

    if let Some(project_key) = config
        .project_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        arguments.insert("project_key".to_string(), json!(project_key));
    }

    if let Some(board_name) = config
        .board_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        arguments.insert("board_name".to_string(), json!(board_name));
    }

    Value::Object(arguments)
}

fn board_list_argument_attempts(config: &JiraMcpConfig) -> Vec<Value> {
    let mut attempts = vec![board_list_arguments(config)];

    attempts.push(json!({ "limit": 50 }));
    attempts.push(json!({ "max_results": 50 }));
    attempts.push(json!({}));

    if let Some(project_key) = config
        .project_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        attempts.insert(0, json!({ "project_key": project_key, "limit": 50 }));
    }

    if let Some(board_name) = config
        .board_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        attempts.insert(0, json!({ "board_name": board_name, "limit": 50 }));
    }

    attempts
}

fn parse_jira_issues(result: &Value) -> Result<Vec<JiraIssue>, String> {
    let payloads = extract_text_payloads(result);
    let mut issues = Vec::new();

    for payload in payloads {
        if let Ok(value) = serde_json::from_str::<Value>(&payload) {
            collect_issues(&value, &mut issues);
        }
    }

    if issues.is_empty() {
        collect_issues(result, &mut issues);
    }

    if issues.is_empty() {
        return Err("Jira MCP response did not contain any issues".to_string());
    }

    Ok(issues)
}

fn parse_jira_boards(result: &Value) -> Result<Vec<JiraBoard>, String> {
    let payloads = extract_text_payloads(result);
    let mut boards = Vec::new();

    for payload in payloads {
        if let Ok(value) = serde_json::from_str::<Value>(&payload) {
            collect_boards(&value, &mut boards);
        }
    }

    if boards.is_empty() {
        collect_boards(result, &mut boards);
    }

    if boards.is_empty() {
        return Err("Jira MCP response did not contain any boards".to_string());
    }

    Ok(boards)
}

fn extract_text_payloads(result: &Value) -> Vec<String> {
    result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("text").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn collect_issues(value: &Value, issues: &mut Vec<JiraIssue>) {
    if let Some(array) = value.as_array() {
        for item in array {
            if let Some(issue) = parse_issue(item) {
                issues.push(issue);
            }
        }
        return;
    }

    for key in ["issues", "results", "values"] {
        if let Some(array) = value.get(key).and_then(Value::as_array) {
            collect_issues(&Value::Array(array.clone()), issues);
        }
    }
}

fn collect_boards(value: &Value, boards: &mut Vec<JiraBoard>) {
    if let Some(array) = value.as_array() {
        for item in array {
            if let Some(board) = parse_board(item) {
                boards.push(board);
            }
        }
        return;
    }

    for key in ["boards", "values", "results"] {
        if let Some(array) = value.get(key).and_then(Value::as_array) {
            collect_boards(&Value::Array(array.clone()), boards);
        }
    }
}

fn parse_issue(value: &Value) -> Option<JiraIssue> {
    let fields = value.get("fields").unwrap_or(value);
    let key = text_field(value, "key")?;
    let summary = text_field(fields, "summary").unwrap_or_else(|| key.clone());
    let description = text_field(fields, "description");
    let status = nested_name(fields, "status").unwrap_or_else(|| "Unknown".to_string());
    let issue_type = nested_name(fields, "issuetype")
        .or_else(|| nested_name(fields, "issue_type"))
        .unwrap_or_else(|| "Task".to_string());

    Some(JiraIssue {
        key,
        summary,
        description,
        status,
        issue_type,
        url: None,
        labels: labels_field(fields),
    })
}

fn labels_field(value: &Value) -> Vec<String> {
    value
        .get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_board(value: &Value) -> Option<JiraBoard> {
    let id = text_field(value, "id")?;
    let name = text_field(value, "name").unwrap_or_else(|| format!("Jira board {id}"));
    let board_type = text_field(value, "type")
        .or_else(|| text_field(value, "board_type"))
        .unwrap_or_else(|| "board".to_string());

    Some(JiraBoard {
        id,
        name,
        board_type,
    })
}

fn nested_name(value: &Value, key: &str) -> Option<String> {
    let nested = value.get(key)?;
    text_value(nested).or_else(|| nested.get("name").and_then(text_value))
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(text_value)
}

fn text_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_issues_from_mcp_text_payload() {
        let result = json!({
            "content": [{
                "type": "text",
                "text": r#"{"issues":[{"key":"SPC-1","fields":{"summary":"Connect Jira","status":{"name":"To Do"},"issuetype":{"name":"Story"}}}]}"#
            }]
        });

        let issues = parse_jira_issues(&result).unwrap();

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].key, "SPC-1");
        assert_eq!(issues[0].summary, "Connect Jira");
        assert_eq!(issues[0].status, "To Do");
    }
}
