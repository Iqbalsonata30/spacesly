use super::files::WorkspaceRoot;
use super::shell_env::inject_shell_env;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;
use url::Url;

const LSP_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const LSP_MESSAGE_LIMIT: usize = 8 * 1024 * 1024;
const LSP_STDERR_LIMIT: usize = 128 * 1024;

#[derive(Clone, Debug, Deserialize)]
pub struct LspServerConfig {
    pub server_id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub language_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LspDiagnostic {
    pub range: LspRange,
    pub severity: Option<u32>,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<Value>,
    pub data: Option<Value>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct LspDiagnosticReport {
    pub version: Option<i64>,
    pub diagnostics: Vec<LspDiagnostic>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LspHoverResult {
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LspTextEdit {
    pub range: LspRange,
    pub new_text: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LspCompletionRequest {
    pub file_path: String,
    pub position: LspPosition,
    pub trigger_kind: Option<u32>,
    pub trigger_character: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LspCompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub documentation: Option<LspHoverResult>,
    pub kind: Option<u32>,
    pub sort_text: Option<String>,
    pub filter_text: Option<String>,
    pub insert_text: Option<String>,
    pub insert_text_format: Option<u32>,
    pub text_edit: Option<LspTextEdit>,
    pub additional_text_edits: Vec<LspTextEdit>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct LspCompletionResult {
    pub is_incomplete: bool,
    pub items: Vec<LspCompletionItem>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LspCodeActionRequest {
    pub file_path: String,
    pub range: LspRange,
    #[serde(default)]
    pub diagnostics: Vec<LspDiagnostic>,
    #[serde(default)]
    pub only: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LspCodeAction {
    pub title: String,
    pub kind: Option<String>,
    pub is_preferred: Option<bool>,
    pub edits: Vec<LspTextEdit>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LspLocation {
    pub file_path: String,
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct LspDocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: u32,
    pub range: LspRange,
    pub selection_range: LspRange,
    pub depth: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct LspServerStatus {
    pub workspace_id: String,
    pub server_id: String,
    pub language_id: String,
    pub status: String,
}

#[derive(Clone, Default)]
pub struct LspRegistry {
    servers: Arc<Mutex<HashMap<String, Arc<LspClient>>>>,
}

struct LspClient {
    child: Mutex<Child>,
    writer: Arc<Mutex<std::process::ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, mpsc::SyncSender<Result<Value, String>>>>>,
    diagnostics: Arc<Mutex<HashMap<String, LspDiagnosticReport>>>,
    opened_documents: Arc<Mutex<HashMap<String, OpenDocument>>>,
    stderr: Arc<Mutex<String>>,
    next_id: AtomicU64,
    sync_kind: AtomicU8,
    workspace_id: String,
    server_id: String,
    language_id: String,
    root: PathBuf,
    root_uri: String,
}

#[derive(Clone)]
struct OpenDocument {
    version: i64,
    text: String,
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if let Ok(child) = self.child.get_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl LspRegistry {
    pub fn start(
        &self,
        roots: &WorkspaceRoot,
        workspace_id: String,
        config: LspServerConfig,
    ) -> Result<LspServerStatus, String> {
        if config.server_id.trim().is_empty() || config.command.trim().is_empty() {
            return Err("LSP server ID and command are required.".to_string());
        }
        let root = roots.path(&workspace_id)?;
        let root_uri = Url::from_directory_path(&root)
            .map_err(|_| "Failed to create workspace file URI.".to_string())?
            .to_string();
        let mut command = Command::new(config.command.trim());
        inject_shell_env(&mut command);
        command
            .args(&config.args)
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("Failed to start LSP server {}: {error}", config.server_id))?;
        let writer =
            Arc::new(Mutex::new(child.stdin.take().ok_or_else(|| {
                "LSP server stdin was not captured.".to_string()
            })?));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "LSP server stdout was not captured.".to_string())?;
        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(stderr_reader) = child.stderr.take() {
            capture_stderr(stderr_reader, Arc::clone(&stderr));
        }
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let diagnostics = Arc::new(Mutex::new(HashMap::new()));
        let opened_documents = Arc::new(Mutex::new(HashMap::new()));
        start_reader(
            stdout,
            Arc::clone(&writer),
            Arc::clone(&pending),
            Arc::clone(&diagnostics),
            Arc::clone(&opened_documents),
            root_uri.clone(),
        );
        let client = Arc::new(LspClient {
            child: Mutex::new(child),
            writer,
            pending,
            diagnostics,
            opened_documents,
            stderr,
            next_id: AtomicU64::new(1),
            sync_kind: AtomicU8::new(1),
            workspace_id: workspace_id.clone(),
            server_id: config.server_id.clone(),
            language_id: config.language_id.clone(),
            root,
            root_uri: root_uri.clone(),
        });
        let initialize = client.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "workspaceFolders": [{ "uri": client.root_uri, "name": workspace_id }],
                "clientInfo": { "name": "Spacesly", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": {
                    "workspace": {
                        "workspaceFolders": true,
                        "configuration": true,
                        "workspaceEdit": { "documentChanges": true }
                    },
                    "textDocument": {
                        "synchronization": { "didSave": true, "dynamicRegistration": false },
                        "publishDiagnostics": { "versionSupport": true },
                        "hover": { "contentFormat": ["markdown", "plaintext"] },
                        "definition": { "linkSupport": true },
                        "references": { "dynamicRegistration": false },
                        "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                        "completion": {
                            "contextSupport": true,
                            "completionItem": {
                                "documentationFormat": ["markdown", "plaintext"],
                                "snippetSupport": true,
                                "insertReplaceSupport": true
                            },
                            "completionList": {
                                "itemDefaults": ["editRange", "insertTextFormat"]
                            }
                        },
                        "codeAction": {
                            "codeActionLiteralSupport": {
                                "codeActionKind": {
                                    "valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source"]
                                }
                            }
                        }
                    }
                }
            }),
        );
        let initialize = initialize
            .map_err(|error| format!("LSP initialization failed: {error}. {}", client.stderr()))?;
        client
            .sync_kind
            .store(text_document_sync_kind(&initialize), Ordering::Relaxed);
        client.notify("initialized", json!({}))?;
        let key = server_key(&workspace_id, &config.server_id);
        self.servers
            .lock()
            .map_err(|error| error.to_string())?
            .insert(key, Arc::clone(&client));
        Ok(client.status("running"))
    }

    pub fn stop(&self, workspace_id: &str, server_id: &str) -> Result<bool, String> {
        let client = self
            .servers
            .lock()
            .map_err(|error| error.to_string())?
            .remove(&server_key(workspace_id, server_id));
        if let Some(client) = client {
            let _ = client.request("shutdown", Value::Null);
            let _ = client.notify("exit", Value::Null);
            return Ok(true);
        }
        Ok(false)
    }

    pub fn statuses(&self) -> Result<Vec<LspServerStatus>, String> {
        Ok(self
            .servers
            .lock()
            .map_err(|error| error.to_string())?
            .values()
            .map(|client| client.status("running"))
            .collect())
    }

    pub fn stop_all(&self) {
        let keys = self
            .servers
            .lock()
            .ok()
            .map(|servers| {
                servers
                    .values()
                    .map(|client| (client.workspace_id.clone(), client.server_id.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (workspace_id, server_id) in keys {
            let _ = self.stop(&workspace_id, &server_id);
        }
    }

    pub fn sync_document(
        &self,
        workspace_id: &str,
        server_id: &str,
        file_path: &str,
        language_id: &str,
        version: i64,
        text: String,
    ) -> Result<(), String> {
        self.client(workspace_id, server_id)?
            .sync_document(file_path, language_id, version, text)
    }

    pub fn close_document(
        &self,
        workspace_id: &str,
        server_id: &str,
        file_path: &str,
    ) -> Result<(), String> {
        self.client(workspace_id, server_id)?
            .close_document(file_path)
    }

    pub fn diagnostics(
        &self,
        workspace_id: &str,
        server_id: &str,
        file_path: &str,
    ) -> Result<LspDiagnosticReport, String> {
        self.client(workspace_id, server_id)?.diagnostics(file_path)
    }

    pub fn hover(
        &self,
        workspace_id: &str,
        server_id: &str,
        file_path: &str,
        position: LspPosition,
    ) -> Result<Option<LspHoverResult>, String> {
        self.client(workspace_id, server_id)?
            .hover(file_path, position)
    }

    pub fn definition(
        &self,
        workspace_id: &str,
        server_id: &str,
        file_path: &str,
        position: LspPosition,
    ) -> Result<Option<LspLocation>, String> {
        self.client(workspace_id, server_id)?
            .definition(file_path, position)
    }

    pub fn references(
        &self,
        workspace_id: &str,
        server_id: &str,
        file_path: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>, String> {
        self.client(workspace_id, server_id)?
            .references(file_path, position)
    }

    pub fn document_symbols(
        &self,
        workspace_id: &str,
        server_id: &str,
        file_path: &str,
    ) -> Result<Vec<LspDocumentSymbol>, String> {
        self.client(workspace_id, server_id)?
            .document_symbols(file_path)
    }

    pub fn completion(
        &self,
        workspace_id: &str,
        server_id: &str,
        request: LspCompletionRequest,
    ) -> Result<LspCompletionResult, String> {
        self.client(workspace_id, server_id)?.completion(request)
    }

    pub fn code_actions(
        &self,
        workspace_id: &str,
        server_id: &str,
        request: LspCodeActionRequest,
    ) -> Result<Vec<LspCodeAction>, String> {
        self.client(workspace_id, server_id)?.code_actions(request)
    }

    fn client(&self, workspace_id: &str, server_id: &str) -> Result<Arc<LspClient>, String> {
        self.servers
            .lock()
            .map_err(|error| error.to_string())?
            .get(&server_key(workspace_id, server_id))
            .cloned()
            .ok_or_else(|| format!("LSP server {server_id} is not running for this workspace."))
    }
}

impl LspClient {
    fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(1);
        self.pending
            .lock()
            .map_err(|error| error.to_string())?
            .insert(id, tx);
        if let Err(error) = send_message(
            &self.writer,
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        ) {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(error);
        }
        match rx.recv_timeout(LSP_REQUEST_TIMEOUT) {
            Ok(result) => result,
            Err(_) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&id);
                }
                Err(format!("LSP request {method} timed out."))
            }
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        send_message(
            &self.writer,
            &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
        )
    }

    fn sync_document(
        &self,
        file_path: &str,
        language_id: &str,
        version: i64,
        text: String,
    ) -> Result<(), String> {
        let uri = self.file_uri(file_path)?;
        let mut opened = self
            .opened_documents
            .lock()
            .map_err(|error| error.to_string())?;
        let method;
        let params;
        if let Some(previous) = opened.get(&uri) {
            if version <= previous.version {
                return Ok(());
            }
            method = "textDocument/didChange";
            let content_changes = if self.sync_kind.load(Ordering::Relaxed) == 2 {
                vec![incremental_content_change(&previous.text, &text)]
            } else {
                vec![json!({ "text": text.clone() })]
            };
            params = json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": content_changes
            });
        } else {
            method = "textDocument/didOpen";
            params = json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text.clone()
                }
            });
        }
        self.notify(method, params)?;
        opened.insert(uri, OpenDocument { version, text });
        Ok(())
    }

    fn close_document(&self, file_path: &str) -> Result<(), String> {
        let uri = self.file_uri(file_path)?;
        self.opened_documents
            .lock()
            .map_err(|error| error.to_string())?
            .remove(&uri);
        self.diagnostics
            .lock()
            .map_err(|error| error.to_string())?
            .remove(&uri);
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        )
    }

    fn diagnostics(&self, file_path: &str) -> Result<LspDiagnosticReport, String> {
        let uri = self.file_uri(file_path)?;
        Ok(self
            .diagnostics
            .lock()
            .map_err(|error| error.to_string())?
            .get(&uri)
            .cloned()
            .unwrap_or_default())
    }

    fn hover(
        &self,
        file_path: &str,
        position: LspPosition,
    ) -> Result<Option<LspHoverResult>, String> {
        let uri = self.file_uri(file_path)?;
        let value = self.request(
            "textDocument/hover",
            json!({ "textDocument": { "uri": uri }, "position": position }),
        )?;
        Ok(parse_hover(&value))
    }

    fn definition(
        &self,
        file_path: &str,
        position: LspPosition,
    ) -> Result<Option<LspLocation>, String> {
        let uri = self.file_uri(file_path)?;
        let value = self.request(
            "textDocument/definition",
            json!({ "textDocument": { "uri": uri }, "position": position }),
        )?;
        Ok(parse_location(&value, &self.root))
    }

    fn references(
        &self,
        file_path: &str,
        position: LspPosition,
    ) -> Result<Vec<LspLocation>, String> {
        let uri = self.file_uri(file_path)?;
        let value = self.request(
            "textDocument/references",
            json!({
                "textDocument": { "uri": uri },
                "position": position,
                "context": { "includeDeclaration": true }
            }),
        )?;
        Ok(parse_locations(&value, &self.root))
    }

    fn document_symbols(&self, file_path: &str) -> Result<Vec<LspDocumentSymbol>, String> {
        let uri = self.file_uri(file_path)?;
        let value = self.request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        )?;
        Ok(parse_document_symbols(&value, &uri))
    }

    fn completion(&self, request: LspCompletionRequest) -> Result<LspCompletionResult, String> {
        let uri = self.file_uri(&request.file_path)?;
        let mut params = json!({
            "textDocument": { "uri": uri },
            "position": request.position
        });
        if let Some(trigger_kind) = request.trigger_kind {
            let mut context = json!({ "triggerKind": trigger_kind });
            if let Some(trigger_character) = request.trigger_character {
                context["triggerCharacter"] = Value::String(trigger_character);
            }
            params["context"] = context;
        }
        let value = self.request("textDocument/completion", params)?;
        Ok(parse_completion(&value))
    }

    fn code_actions(&self, request: LspCodeActionRequest) -> Result<Vec<LspCodeAction>, String> {
        let uri = self.file_uri(&request.file_path)?;
        let mut context = json!({ "diagnostics": request.diagnostics });
        if !request.only.is_empty() {
            context["only"] = json!(request.only);
        }
        let value = self.request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri },
                "range": request.range,
                "context": context
            }),
        )?;
        Ok(parse_code_actions(&value, &uri))
    }

    fn file_uri(&self, file_path: &str) -> Result<String, String> {
        let relative = Path::new(file_path);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err("LSP document path must remain inside the workspace root.".to_string());
        }
        Url::from_file_path(self.root.join(relative))
            .map(|url| url.to_string())
            .map_err(|_| "Failed to create document file URI.".to_string())
    }

    fn status(&self, status: &str) -> LspServerStatus {
        LspServerStatus {
            workspace_id: self.workspace_id.clone(),
            server_id: self.server_id.clone(),
            language_id: self.language_id.clone(),
            status: status.to_string(),
        }
    }

    fn stderr(&self) -> String {
        self.stderr
            .lock()
            .ok()
            .map(|stderr| stderr.trim().to_string())
            .filter(|stderr| !stderr.is_empty())
            .unwrap_or_else(|| "The server produced no stderr output.".to_string())
    }
}

fn server_key(workspace_id: &str, server_id: &str) -> String {
    format!("{workspace_id}:{server_id}")
}

fn text_document_sync_kind(initialize_result: &Value) -> u8 {
    let sync = &initialize_result["capabilities"]["textDocumentSync"];
    sync.as_u64()
        .or_else(|| sync.get("change").and_then(Value::as_u64))
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(1)
}

fn incremental_content_change(previous: &str, current: &str) -> Value {
    let prefix = previous
        .chars()
        .zip(current.chars())
        .take_while(|(left, right)| left == right)
        .map(|(character, _)| character.len_utf8())
        .sum::<usize>();
    let suffix = previous[prefix..]
        .chars()
        .rev()
        .zip(current[prefix..].chars().rev())
        .take_while(|(left, right)| left == right)
        .map(|(character, _)| character.len_utf8())
        .sum::<usize>();
    let previous_end = previous.len() - suffix;
    let current_end = current.len() - suffix;

    json!({
        "range": {
            "start": offset_position(previous, prefix),
            "end": offset_position(previous, previous_end)
        },
        "text": &current[prefix..current_end]
    })
}

fn offset_position(text: &str, offset: usize) -> LspPosition {
    let prefix = &text[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let character = prefix
        .rsplit_once('\n')
        .map(|(_, value)| value)
        .unwrap_or(prefix)
        .encode_utf16()
        .count() as u32;
    LspPosition { line, character }
}

fn send_message(
    writer: &Arc<Mutex<std::process::ChildStdin>>,
    value: &Value,
) -> Result<(), String> {
    let body = serde_json::to_vec(value)
        .map_err(|error| format!("Failed to encode LSP message: {error}"))?;
    let mut writer = writer.lock().map_err(|error| error.to_string())?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())
        .map_err(|error| format!("Failed to write LSP header: {error}"))?;
    writer
        .write_all(&body)
        .and_then(|_| writer.flush())
        .map_err(|error| format!("Failed to write LSP message: {error}"))
}

fn start_reader(
    stdout: std::process::ChildStdout,
    writer: Arc<Mutex<std::process::ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, mpsc::SyncSender<Result<Value, String>>>>>,
    diagnostics: Arc<Mutex<HashMap<String, LspDiagnosticReport>>>,
    opened_documents: Arc<Mutex<HashMap<String, OpenDocument>>>,
    root_uri: String,
) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let message = match read_message(&mut reader) {
                Ok(Some(message)) => message,
                Ok(None) => {
                    fail_pending(&pending, "LSP server closed stdout.");
                    break;
                }
                Err(error) => {
                    fail_pending(&pending, &error);
                    break;
                }
            };
            if let Some(id) = message.get("id").and_then(Value::as_u64) {
                if let Ok(mut pending) = pending.lock() {
                    if let Some(sender) = pending.remove(&id) {
                        let result = if let Some(error) = message.get("error") {
                            Err(format!("LSP error response: {error}"))
                        } else {
                            Ok(message.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let _ = sender.send(result);
                        continue;
                    }
                }
            }
            let method = message.get("method").and_then(Value::as_str);
            if method == Some("textDocument/publishDiagnostics") {
                if let Some(params) = message.get("params") {
                    let uri = params
                        .get("uri")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let values = params
                        .get("diagnostics")
                        .cloned()
                        .and_then(|value| serde_json::from_value::<Vec<LspDiagnostic>>(value).ok())
                        .unwrap_or_default();
                    if let Ok(mut diagnostics) = diagnostics.lock() {
                        let version = params.get("version").and_then(Value::as_i64).or_else(|| {
                            opened_documents
                                .lock()
                                .ok()
                                .and_then(|opened| opened.get(uri).map(|document| document.version))
                        });
                        diagnostics.insert(
                            uri.to_string(),
                            LspDiagnosticReport {
                                version,
                                diagnostics: values,
                            },
                        );
                    }
                }
                continue;
            }
            if let (Some(id), Some(method)) = (message.get("id").cloned(), method) {
                let result = server_request_result(method, message.get("params"), &root_uri);
                let _ = send_message(
                    &writer,
                    &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                );
            }
        }
    });
}

fn server_request_result(method: &str, params: Option<&Value>, root_uri: &str) -> Value {
    match method {
        "workspace/configuration" => {
            let count = params
                .and_then(|params| params.get("items"))
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            Value::Array(vec![Value::Null; count])
        }
        "workspace/workspaceFolders" => json!([{ "uri": root_uri, "name": "workspace" }]),
        _ => Value::Null,
    }
}

fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>, String> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let size = reader
            .read_line(&mut line)
            .map_err(|error| format!("Failed to read LSP header: {error}"))?;
        if size == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| "LSP Content-Length is invalid.".to_string())?,
                );
            }
        }
    }
    let length = content_length.ok_or_else(|| "LSP message omitted Content-Length.".to_string())?;
    if length > LSP_MESSAGE_LIMIT {
        return Err(format!("LSP message exceeded {LSP_MESSAGE_LIMIT} bytes."));
    }
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("Failed to read LSP body: {error}"))?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| format!("LSP server returned invalid JSON: {error}"))
}

fn fail_pending(
    pending: &Arc<Mutex<HashMap<u64, mpsc::SyncSender<Result<Value, String>>>>>,
    error: &str,
) {
    if let Ok(mut pending) = pending.lock() {
        for (_, sender) in pending.drain() {
            let _ = sender.send(Err(error.to_string()));
        }
    }
}

fn capture_stderr(mut reader: impl Read + Send + 'static, stderr: Arc<Mutex<String>>) {
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(size) = reader.read(&mut buffer) {
            if size == 0 {
                break;
            }
            if let Ok(mut stderr) = stderr.lock() {
                let remaining = LSP_STDERR_LIMIT.saturating_sub(stderr.len());
                if remaining > 0 {
                    stderr.push_str(&String::from_utf8_lossy(&buffer[..size.min(remaining)]));
                }
            }
        }
    });
}

fn parse_completion(value: &Value) -> LspCompletionResult {
    let (items, is_incomplete, defaults) = if let Some(items) = value.as_array() {
        (items.as_slice(), false, None)
    } else if let Some(object) = value.as_object() {
        let Some(items) = object.get("items").and_then(Value::as_array) else {
            return LspCompletionResult::default();
        };
        (
            items.as_slice(),
            object
                .get("isIncomplete")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            object.get("itemDefaults"),
        )
    } else {
        return LspCompletionResult::default();
    };

    LspCompletionResult {
        is_incomplete,
        items: items
            .iter()
            .filter_map(|item| parse_completion_item(item, defaults))
            .collect(),
    }
}

fn parse_completion_item(item: &Value, defaults: Option<&Value>) -> Option<LspCompletionItem> {
    let label = item.get("label")?.as_str()?.to_string();
    let insert_text_format = item
        .get("insertTextFormat")
        .or_else(|| defaults.and_then(|value| value.get("insertTextFormat")))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let text_edit = if let Some(text_edit) = item.get("textEdit") {
        Some(parse_completion_text_edit(text_edit)?)
    } else {
        defaults.and_then(|defaults| {
            let range = defaults.get("editRange")?;
            let range = range.get("replace").unwrap_or(range);
            Some(LspTextEdit {
                range: serde_json::from_value(range.clone()).ok()?,
                new_text: item
                    .get("textEditText")
                    .or_else(|| item.get("insertText"))
                    .and_then(Value::as_str)
                    .unwrap_or(&label)
                    .to_string(),
            })
        })
    };
    let additional_text_edits = match item.get("additionalTextEdits") {
        Some(values) => values
            .as_array()?
            .iter()
            .map(parse_text_edit)
            .collect::<Option<Vec<_>>>()?,
        None => Vec::new(),
    };

    Some(LspCompletionItem {
        label,
        detail: string_field(item, "detail"),
        documentation: item.get("documentation").and_then(parse_markup),
        kind: item
            .get("kind")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
        sort_text: string_field(item, "sortText"),
        filter_text: string_field(item, "filterText"),
        insert_text: string_field(item, "insertText"),
        insert_text_format,
        text_edit,
        additional_text_edits,
    })
}

fn parse_completion_text_edit(value: &Value) -> Option<LspTextEdit> {
    let range = value.get("replace").or_else(|| value.get("range"))?;
    Some(LspTextEdit {
        range: serde_json::from_value(range.clone()).ok()?,
        new_text: value.get("newText")?.as_str()?.to_string(),
    })
}

fn parse_text_edit(value: &Value) -> Option<LspTextEdit> {
    Some(LspTextEdit {
        range: serde_json::from_value(value.get("range")?.clone()).ok()?,
        new_text: value.get("newText")?.as_str()?.to_string(),
    })
}

fn parse_code_actions(value: &Value, document_uri: &str) -> Vec<LspCodeAction> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|action| {
            if action.get("command").is_some() || action.get("disabled").is_some() {
                return None;
            }
            let edits = parse_workspace_edit(action.get("edit")?, document_uri)?;
            Some(LspCodeAction {
                title: action.get("title")?.as_str()?.to_string(),
                kind: string_field(action, "kind"),
                is_preferred: action.get("isPreferred").and_then(Value::as_bool),
                edits,
            })
        })
        .collect()
}

fn parse_workspace_edit(value: &Value, document_uri: &str) -> Option<Vec<LspTextEdit>> {
    let mut edits = Vec::new();
    if let Some(changes) = value.get("changes") {
        let changes = changes.as_object()?;
        if changes.keys().any(|uri| uri != document_uri) {
            return None;
        }
        for values in changes.values() {
            edits.extend(
                values
                    .as_array()?
                    .iter()
                    .map(parse_text_edit)
                    .collect::<Option<Vec<_>>>()?,
            );
        }
    }
    if let Some(document_changes) = value.get("documentChanges") {
        for change in document_changes.as_array()? {
            if change.get("kind").is_some()
                || change.get("textDocument")?.get("uri")?.as_str()? != document_uri
            {
                return None;
            }
            edits.extend(
                change
                    .get("edits")?
                    .as_array()?
                    .iter()
                    .map(parse_text_edit)
                    .collect::<Option<Vec<_>>>()?,
            );
        }
    }
    (!edits.is_empty()).then_some(edits)
}

fn parse_markup(value: &Value) -> Option<LspHoverResult> {
    if let Some(text) = value.as_str() {
        return Some(LspHoverResult {
            kind: "plaintext".to_string(),
            text: text.to_string(),
        });
    }
    Some(LspHoverResult {
        kind: value
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("plaintext")
            .to_string(),
        text: value.get("value")?.as_str()?.to_string(),
    })
}

fn string_field(value: &Value, name: &str) -> Option<String> {
    value.get(name).and_then(Value::as_str).map(str::to_string)
}

fn parse_hover(value: &Value) -> Option<LspHoverResult> {
    let contents = value.get("contents")?;
    if let Some(text) = contents.as_str() {
        return Some(LspHoverResult {
            kind: "plaintext".to_string(),
            text: text.to_string(),
        });
    }
    if let Some(object) = contents.as_object() {
        return Some(LspHoverResult {
            kind: object
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("plaintext")
                .to_string(),
            text: object
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }
    let text = contents
        .as_array()?
        .iter()
        .filter_map(|item| {
            item.as_str()
                .or_else(|| item.get("value").and_then(Value::as_str))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(LspHoverResult {
        kind: "markdown".to_string(),
        text,
    })
}

fn parse_location(value: &Value, root: &Path) -> Option<LspLocation> {
    value
        .as_array()
        .map(|locations| {
            locations
                .iter()
                .find_map(|location| parse_location_item(location, root))
        })
        .unwrap_or_else(|| parse_location_item(value, root))
}

fn parse_locations(value: &Value, root: &Path) -> Vec<LspLocation> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|location| parse_location_item(location, root))
        .collect()
}

fn parse_location_item(location: &Value, root: &Path) -> Option<LspLocation> {
    let uri = location
        .get("uri")
        .or_else(|| location.get("targetUri"))?
        .as_str()?;
    let range = location
        .get("range")
        .or_else(|| location.get("targetSelectionRange"))
        .or_else(|| location.get("targetRange"))?;
    let start = range.get("start")?;
    let path = Url::parse(uri).ok()?.to_file_path().ok()?;
    let relative = path
        .strip_prefix(root)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    Some(LspLocation {
        file_path: relative,
        line: start
            .get("line")?
            .as_u64()
            .and_then(|line| u32::try_from(line).ok())?,
        character: start
            .get("character")?
            .as_u64()
            .and_then(|character| u32::try_from(character).ok())?,
    })
}

fn parse_document_symbols(value: &Value, document_uri: &str) -> Vec<LspDocumentSymbol> {
    let mut symbols = Vec::new();
    for value in value.as_array().into_iter().flatten() {
        if value.get("location").is_some() {
            if let Some(symbol) = parse_symbol_information(value, document_uri) {
                symbols.push(symbol);
            }
        } else {
            append_document_symbol(value, 0, &mut symbols);
        }
    }
    symbols
}

fn append_document_symbol(value: &Value, depth: u32, symbols: &mut Vec<LspDocumentSymbol>) {
    let Some(name) = value.get("name").and_then(Value::as_str) else {
        return;
    };
    let Some(kind) = value
        .get("kind")
        .and_then(Value::as_u64)
        .and_then(|kind| u32::try_from(kind).ok())
    else {
        return;
    };
    let Some(range) = value
        .get("range")
        .cloned()
        .and_then(|range| serde_json::from_value(range).ok())
    else {
        return;
    };
    let Some(selection_range) = value
        .get("selectionRange")
        .cloned()
        .and_then(|range| serde_json::from_value(range).ok())
    else {
        return;
    };

    symbols.push(LspDocumentSymbol {
        name: name.to_string(),
        detail: string_field(value, "detail"),
        kind,
        range,
        selection_range,
        depth,
    });
    for child in value
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        append_document_symbol(child, depth + 1, symbols);
    }
}

fn parse_symbol_information(value: &Value, document_uri: &str) -> Option<LspDocumentSymbol> {
    let location = value.get("location")?;
    if location.get("uri")?.as_str()? != document_uri {
        return None;
    }
    let range: LspRange = serde_json::from_value(location.get("range")?.clone()).ok()?;
    Some(LspDocumentSymbol {
        name: value.get("name")?.as_str()?.to_string(),
        detail: None,
        kind: value
            .get("kind")?
            .as_u64()
            .and_then(|kind| u32::try_from(kind).ok())?,
        selection_range: range.clone(),
        range,
        depth: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        incremental_content_change, parse_code_actions, parse_completion, parse_document_symbols,
        parse_hover, parse_locations, read_message, server_request_result, text_document_sync_kind,
        LspDiagnostic,
    };
    use serde_json::json;
    use std::io::Cursor;

    #[test]
    fn reads_case_insensitive_content_length_frames() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let frame = format!("content-length: {}\r\n\r\n{body}", body.len());
        let message = read_message(&mut Cursor::new(frame.into_bytes()))
            .expect("frame should parse")
            .expect("message should exist");
        assert_eq!(message["id"], 1);
    }

    #[test]
    fn answers_workspace_configuration_requests() {
        assert_eq!(
            server_request_result(
                "workspace/configuration",
                Some(&json!({ "items": [{}, {}] })),
                "file:///workspace/"
            ),
            json!([null, null])
        );
    }

    #[test]
    fn normalizes_markup_hover_content() {
        let hover =
            parse_hover(&json!({ "contents": { "kind": "markdown", "value": "**item**" } }))
                .expect("hover should parse");
        assert_eq!(hover.kind, "markdown");
        assert_eq!(hover.text, "**item**");
    }

    #[test]
    fn reads_incremental_sync_capability() {
        assert_eq!(
            text_document_sync_kind(&json!({
                "capabilities": { "textDocumentSync": { "change": 2 } }
            })),
            2
        );
    }

    #[test]
    fn creates_utf16_incremental_document_changes() {
        assert_eq!(
            incremental_content_change("a😀b", "a😀xb"),
            json!({
                "range": {
                    "start": { "line": 0, "character": 3 },
                    "end": { "line": 0, "character": 3 }
                },
                "text": "x"
            })
        );
    }

    #[test]
    fn parses_completion_lists_with_snippets_and_edits() {
        let result = parse_completion(&json!({
            "isIncomplete": true,
            "itemDefaults": {
                "insertTextFormat": 2,
                "editRange": {
                    "insert": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
                    "replace": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 2 } }
                }
            },
            "items": [{
                "label": "println!",
                "textEditText": "println!(\"${1}\");",
                "documentation": { "kind": "markdown", "value": "Print a line." },
                "additionalTextEdits": [{
                    "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                    "newText": "use std::println;\n"
                }]
            }]
        }));

        assert!(result.is_incomplete);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].insert_text_format, Some(2));
        assert_eq!(
            result.items[0]
                .text_edit
                .as_ref()
                .unwrap()
                .range
                .start
                .character,
            0
        );
        assert_eq!(
            result.items[0].text_edit.as_ref().unwrap().new_text,
            "println!(\"${1}\");"
        );
        assert_eq!(result.items[0].additional_text_edits.len(), 1);
        assert_eq!(
            result.items[0].documentation.as_ref().unwrap().kind,
            "markdown"
        );
    }

    #[test]
    fn parses_completion_arrays_and_insert_replace_edits() {
        let result = parse_completion(&json!([{
            "label": "value",
            "insertTextFormat": 1,
            "textEdit": {
                "insert": { "start": { "line": 2, "character": 3 }, "end": { "line": 2, "character": 3 } },
                "replace": { "start": { "line": 2, "character": 1 }, "end": { "line": 2, "character": 3 } },
                "newText": "value"
            }
        }]));

        assert!(!result.is_incomplete);
        assert_eq!(
            result.items[0]
                .text_edit
                .as_ref()
                .unwrap()
                .range
                .start
                .character,
            1
        );
    }

    #[test]
    fn code_actions_keep_only_same_document_text_edits() {
        let uri = "file:///workspace/src/main.rs";
        let actions = parse_code_actions(
            &json!([
                {
                    "title": "Fix import",
                    "kind": "quickfix",
                    "isPreferred": true,
                    "edit": { "changes": { (uri): [{
                        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                        "newText": "use crate::Item;\n"
                    }] } }
                },
                {
                    "title": "Replace name",
                    "edit": { "documentChanges": [{
                        "textDocument": { "uri": uri, "version": 3 },
                        "edits": [{
                            "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 3 } },
                            "newText": "item"
                        }]
                    }] }
                },
                { "title": "Run command", "command": { "title": "Run", "command": "run" } },
                {
                    "title": "Edit then run command",
                    "edit": { "changes": { (uri): [{
                        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                        "newText": "unsafe partial fix"
                    }] } },
                    "command": { "title": "Finish", "command": "finish.fix" }
                },
                {
                    "title": "Touch another file",
                    "edit": { "changes": { "file:///workspace/src/other.rs": [{
                        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                        "newText": "changed"
                    }] } }
                },
                {
                    "title": "Create file",
                    "edit": { "documentChanges": [{ "kind": "create", "uri": "file:///workspace/new.rs" }] }
                }
            ]),
            uri,
        );

        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].title, "Fix import");
        assert_eq!(actions[0].edits.len(), 1);
        assert_eq!(actions[0].is_preferred, Some(true));
        assert_eq!(actions[1].title, "Replace name");
    }

    #[test]
    fn diagnostic_data_survives_deserialization() {
        let diagnostic: LspDiagnostic = serde_json::from_value(json!({
            "range": { "start": { "line": 0, "character": 1 }, "end": { "line": 0, "character": 2 } },
            "severity": 1,
            "message": "problem",
            "source": "test",
            "code": "E1",
            "data": { "fixId": 42 }
        }))
        .expect("diagnostic should parse");

        assert_eq!(diagnostic.data, Some(json!({ "fixId": 42 })));
    }

    #[test]
    fn normalizes_reference_locations_and_location_links() {
        let references = parse_locations(
            &json!([
                {
                    "uri": "file:///workspace/src/main.rs",
                    "range": {
                        "start": { "line": 2, "character": 4 },
                        "end": { "line": 2, "character": 8 }
                    }
                },
                {
                    "targetUri": "file:///workspace/src/lib.rs",
                    "targetRange": {
                        "start": { "line": 5, "character": 0 },
                        "end": { "line": 7, "character": 1 }
                    },
                    "targetSelectionRange": {
                        "start": { "line": 5, "character": 3 },
                        "end": { "line": 5, "character": 7 }
                    }
                },
                {
                    "targetUri": "file:///workspace/src/fallback.rs",
                    "targetRange": {
                        "start": { "line": 8, "character": 1 },
                        "end": { "line": 9, "character": 0 }
                    }
                }
            ]),
            std::path::Path::new("/workspace"),
        );

        assert_eq!(references.len(), 3);
        assert_eq!(references[0].file_path, "src/main.rs");
        assert_eq!(references[0].line, 2);
        assert_eq!(references[1].file_path, "src/lib.rs");
        assert_eq!(references[1].line, 5);
        assert_eq!(references[1].character, 3);
        assert_eq!(references[2].file_path, "src/fallback.rs");
        assert_eq!(references[2].line, 8);
    }

    #[test]
    fn reference_locations_exclude_targets_outside_workspace() {
        let references = parse_locations(
            &json!([
                {
                    "uri": "file:///outside/main.rs",
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 0, "character": 1 }
                    }
                },
                {
                    "uri": "https://example.com/main.rs",
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 0, "character": 1 }
                    }
                },
                { "uri": "file:///workspace/src/malformed.rs" }
            ]),
            std::path::Path::new("/workspace"),
        );

        assert!(references.is_empty());
    }

    #[test]
    fn flattens_hierarchical_document_symbols_with_depth() {
        let range = json!({
            "start": { "line": 1, "character": 0 },
            "end": { "line": 5, "character": 1 }
        });
        let selection_range = json!({
            "start": { "line": 1, "character": 3 },
            "end": { "line": 1, "character": 7 }
        });
        let symbols = parse_document_symbols(
            &json!([{
                "name": "outer",
                "detail": "fn()",
                "kind": 12,
                "range": range,
                "selectionRange": selection_range,
                "children": [{
                    "name": "inner",
                    "kind": 13,
                    "range": range,
                    "selectionRange": selection_range
                }]
            }]),
            "file:///workspace/src/main.rs",
        );

        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "outer");
        assert_eq!(symbols[0].detail.as_deref(), Some("fn()"));
        assert_eq!(symbols[0].depth, 0);
        assert_eq!(symbols[1].name, "inner");
        assert_eq!(symbols[1].depth, 1);
        assert_eq!(symbols[1].selection_range.start.character, 3);
    }

    #[test]
    fn normalizes_flat_symbols_only_for_requested_document() {
        let uri = "file:///workspace/src/main.rs";
        let range = json!({
            "start": { "line": 2, "character": 4 },
            "end": { "line": 2, "character": 9 }
        });
        let symbols = parse_document_symbols(
            &json!([
                {
                    "name": "local",
                    "kind": 13,
                    "location": { "uri": uri, "range": range }
                },
                {
                    "name": "foreign",
                    "kind": 13,
                    "location": {
                        "uri": "file:///workspace/src/other.rs",
                        "range": range
                    }
                }
            ]),
            uri,
        );

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "local");
        assert_eq!(symbols[0].depth, 0);
        assert_eq!(symbols[0].range.start.character, 4);
        assert_eq!(symbols[0].selection_range.start.character, 4);
    }
}
