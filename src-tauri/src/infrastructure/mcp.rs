use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::jira_rest;
use super::shell_env::inject_shell_env;

const MCP_RESPONSE_QUEUE_CAPACITY: usize = 128;
const MCP_STDERR_LIMIT: usize = 64 * 1024;
const MCP_MESSAGE_LIMIT: usize = 8 * 1024 * 1024;
const MCP_HEADER_LINE_LIMIT: usize = 8 * 1024;
const MCP_SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MCP_MAX_SESSIONS: usize = 8;

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
    let tools = with_mcp_client(&server, |client| client.tools())?;

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
    with_mcp_client(&config.server, |client| {
        let tools = client.tools()?;
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
            .and_then(|board_tool| fetch_boards_with_fallbacks(client, &board_tool, &config).ok())
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

    with_mcp_client(&config.server, |client| {
        let tools = client.tools()?;
        let (tool_name, arguments) = if let Some(board_id) = config
            .board_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        {
            (
                resolve_tool(&tools, &config.board_issues_tool_name)?,
                json!({
                    "board_id": board_id,
                    "jql": config.jql,
                    "fields": "key,summary,description,status,issuetype",
                    "start_at": 0,
                    "limit": 50
                }),
            )
        } else {
            (
                resolve_tool(&tools, &config.tool_name)?,
                json!({
                    "jql": config.jql,
                    "fields": "key,summary,description,status,issuetype",
                    "limit": 50
                }),
            )
        };
        let result = client.call_tool(&tool_name, arguments)?;
        parse_jira_issues(&result)
    })
}

struct StdioMcpClient {
    child: Child,
    stdin: std::process::ChildStdin,
    responses: mpsc::Receiver<Value>,
    stderr: Arc<Mutex<String>>,
    next_id: u64,
    tools: Option<Vec<String>>,
}

impl StdioMcpClient {
    fn start(config: &McpServerConfig) -> Result<Self, String> {
        if config.command.trim().is_empty() {
            return Err("MCP server command is required.".to_string());
        }

        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        inject_shell_env(&mut command);
        command.envs(&config.env);

        let mut child = command
            .spawn()
            .map_err(|error| {
                if error.kind() == ErrorKind::NotFound {
                    format!(
                        "MCP command '{}' was not found. Use a full executable path, or enter a full command line in Settings such as 'npx -y <package>'. Original error: {error}",
                        config.command
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
        let (tx, responses) = mpsc::sync_channel(MCP_RESPONSE_QUEUE_CAPACITY);
        let stderr_buffer = Arc::new(Mutex::new(String::new()));

        spawn_stdout_reader(stdout, tx);
        spawn_stderr_reader(stderr, Arc::clone(&stderr_buffer));

        Ok(Self {
            child,
            stdin,
            responses,
            stderr: stderr_buffer,
            next_id: request_seed(),
            tools: None,
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

    fn tools(&mut self) -> Result<Vec<String>, String> {
        if let Some(tools) = self.tools.as_ref() {
            return Ok(tools.clone());
        }
        let tools = self.list_tools()?;
        self.tools = Some(tools.clone());
        Ok(tools)
    }

    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
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

struct McpSessionEntry {
    client: Arc<Mutex<StdioMcpClient>>,
    last_used: Instant,
}

#[derive(Default)]
struct McpSessionManager {
    sessions: Mutex<HashMap<String, McpSessionEntry>>,
}

static MCP_SESSIONS: OnceLock<McpSessionManager> = OnceLock::new();

fn mcp_session_manager() -> &'static McpSessionManager {
    MCP_SESSIONS.get_or_init(McpSessionManager::default)
}

pub fn close_all_mcp_sessions() {
    let Some(manager) = MCP_SESSIONS.get() else {
        return;
    };
    let sessions = manager.sessions.lock().map(|mut sessions| {
        sessions
            .drain()
            .map(|(_, entry)| entry.client)
            .collect::<Vec<_>>()
    });
    if let Ok(sessions) = sessions {
        drop(sessions);
    }
}

fn with_mcp_client<T, F>(server: &McpServerConfig, operation: F) -> Result<T, String>
where
    F: FnOnce(&mut StdioMcpClient) -> Result<T, String>,
{
    let key = mcp_server_key(server);
    let manager = mcp_session_manager();
    let now = Instant::now();
    let (session, expired) = {
        let mut sessions = manager.sessions.lock().map_err(|error| error.to_string())?;
        let expired = reap_idle_sessions(&mut sessions, now);
        (
            sessions.get(&key).map(|entry| Arc::clone(&entry.client)),
            expired,
        )
    };
    drop(expired);

    let session = if let Some(session) = session {
        session
    } else {
        let mut client = StdioMcpClient::start(server)?;
        client.initialize()?;
        client.tools()?;
        let candidate = Arc::new(Mutex::new(client));
        let (session, evicted) = {
            let mut sessions = manager.sessions.lock().map_err(|error| error.to_string())?;
            if let Some(existing) = sessions.get(&key) {
                (Arc::clone(&existing.client), Vec::new())
            } else {
                let evicted = evict_oldest_idle_session(&mut sessions, now);
                if sessions.len() >= MCP_MAX_SESSIONS {
                    return Err(format!(
                        "MCP session limit reached ({MCP_MAX_SESSIONS}). Close an unused MCP connection before starting another."
                    ));
                }
                sessions.insert(
                    key.clone(),
                    McpSessionEntry {
                        client: Arc::clone(&candidate),
                        last_used: now,
                    },
                );
                (candidate, evicted)
            }
        };
        drop(evicted);
        session
    };

    let mut client = session.lock().map_err(|error| error.to_string())?;
    let result = operation(&mut client);
    let alive = client.is_alive();
    drop(client);

    let mut sessions = manager.sessions.lock().map_err(|error| error.to_string())?;
    if let Some(entry) = sessions.get_mut(&key) {
        if Arc::ptr_eq(&entry.client, &session) {
            if alive && result.is_ok() {
                entry.last_used = Instant::now();
            } else if !alive || result.is_err() {
                sessions.remove(&key);
            }
        }
    }
    result
}

fn reap_idle_sessions(
    sessions: &mut HashMap<String, McpSessionEntry>,
    now: Instant,
) -> Vec<Arc<Mutex<StdioMcpClient>>> {
    let expired: Vec<String> = sessions
        .iter()
        .filter(|(_, entry)| {
            Arc::strong_count(&entry.client) == 1
                && now.duration_since(entry.last_used) >= MCP_SESSION_IDLE_TIMEOUT
        })
        .map(|(key, _)| key.clone())
        .collect();
    expired
        .into_iter()
        .filter_map(|key| sessions.remove(&key).map(|entry| entry.client))
        .collect()
}

fn evict_oldest_idle_session(
    sessions: &mut HashMap<String, McpSessionEntry>,
    now: Instant,
) -> Vec<Arc<Mutex<StdioMcpClient>>> {
    let mut evicted = reap_idle_sessions(sessions, now);
    if sessions.len() < MCP_MAX_SESSIONS {
        return evicted;
    }
    let oldest = sessions
        .iter()
        .filter(|(_, entry)| Arc::strong_count(&entry.client) == 1)
        .min_by_key(|(_, entry)| entry.last_used)
        .map(|(key, _)| key.clone());
    if let Some(key) = oldest {
        if let Some(entry) = sessions.remove(&key) {
            evicted.push(entry.client);
        }
    }
    evicted
}

fn mcp_server_key(server: &McpServerConfig) -> String {
    let mut env: Vec<_> = server.env.iter().collect();
    env.sort_by(|left, right| left.0.cmp(right.0));
    format!(
        "{}\0{}\0{}",
        server.command,
        server.args.join("\0"),
        env.into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("\0")
    )
}

fn spawn_stdout_reader(stdout: std::process::ChildStdout, tx: mpsc::SyncSender<Value>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_stdout_message(&mut reader) {
                Ok(Some(value)) => {
                    let is_response = value.get("id").is_some();
                    if is_response {
                        if tx.send(value).is_err() {
                            break;
                        }
                    } else {
                        let _ = tx.try_send(value);
                    }
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
                append_bounded_text(&mut output, &line, MCP_STDERR_LIMIT);
            }
        }
    });
}

fn read_stdout_message<R: BufRead>(reader: &mut R) -> Result<Option<Value>, String> {
    let Some(line) = read_bounded_line(reader, MCP_MESSAGE_LIMIT)? else {
        return Ok(None);
    };

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
            let Some(header) = read_bounded_line(reader, MCP_HEADER_LINE_LIMIT)? else {
                break;
            };
            if header.trim_end_matches(['\r', '\n']).is_empty() {
                break;
            }
        }
        if length > MCP_MESSAGE_LIMIT {
            return Err(format!(
                "MCP message exceeds the {MCP_MESSAGE_LIMIT}-byte limit."
            ));
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

fn read_bounded_line<R: BufRead>(reader: &mut R, limit: usize) -> Result<Option<String>, String> {
    let mut bytes = Vec::new();
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|error| format!("Failed to read MCP stdout: {error}"))?;
        if buffer.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            return Ok(Some(String::from_utf8_lossy(&bytes).to_string()));
        }

        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let length = newline + 1;
            if bytes.len() + length > limit {
                reader.consume(length);
                return Err(format!("MCP line exceeds the {limit}-byte limit."));
            }
            bytes.extend_from_slice(&buffer[..length]);
            reader.consume(length);
            return Ok(Some(String::from_utf8_lossy(&bytes).to_string()));
        }

        if bytes.len() + buffer.len() > limit {
            let length = buffer.len();
            reader.consume(length);
            drain_line(reader)?;
            return Err(format!("MCP line exceeds the {limit}-byte limit."));
        }

        bytes.extend_from_slice(buffer);
        let length = buffer.len();
        reader.consume(length);
    }
}

fn drain_line<R: BufRead>(reader: &mut R) -> Result<(), String> {
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|error| format!("Failed to drain MCP stdout: {error}"))?;
        if buffer.is_empty() {
            return Ok(());
        }
        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            reader.consume(newline + 1);
            return Ok(());
        }
        let length = buffer.len();
        reader.consume(length);
    }
}

fn append_bounded_text(output: &mut String, value: &str, limit: usize) {
    if !output.is_empty() && output.len() < limit {
        output.push('\n');
    }
    let remaining = limit.saturating_sub(output.len());
    if remaining == 0 {
        return;
    }
    let mut length = value.len().min(remaining);
    while length > 0 && !value.is_char_boundary(length) {
        length -= 1;
    }
    output.push_str(&value[..length]);
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

    #[test]
    fn rejects_oversized_mcp_lines_without_consuming_the_next_message() {
        let input = b"123456789\n{\"id\":1,\"result\":{}}\n";
        let mut reader = BufReader::new(std::io::Cursor::new(input));
        let error = read_bounded_line(&mut reader, 8).unwrap_err();
        assert!(error.contains("exceeds"));

        let next = read_stdout_message(&mut reader)
            .expect("next message should remain readable")
            .expect("next message should exist");
        assert_eq!(next.get("id").and_then(Value::as_u64), Some(1));
    }

    #[test]
    fn bounds_mcp_stderr_retention() {
        let mut output = String::new();
        append_bounded_text(
            &mut output,
            &"x".repeat(MCP_STDERR_LIMIT * 2),
            MCP_STDERR_LIMIT,
        );
        append_bounded_text(&mut output, "later diagnostic", MCP_STDERR_LIMIT);
        assert!(output.len() <= MCP_STDERR_LIMIT);
    }

    #[test]
    fn server_key_is_stable_across_environment_order() {
        let mut first_env = HashMap::new();
        first_env.insert("TOKEN".to_string(), "secret".to_string());
        first_env.insert("MODE".to_string(), "test".to_string());
        let mut second_env = HashMap::new();
        second_env.insert("MODE".to_string(), "test".to_string());
        second_env.insert("TOKEN".to_string(), "secret".to_string());
        let first = McpServerConfig {
            command: "server".to_string(),
            args: vec!["--stdio".to_string()],
            env: first_env,
        };
        let second = McpServerConfig {
            command: "server".to_string(),
            args: vec!["--stdio".to_string()],
            env: second_env,
        };

        assert_eq!(mcp_server_key(&first), mcp_server_key(&second));
    }
}
