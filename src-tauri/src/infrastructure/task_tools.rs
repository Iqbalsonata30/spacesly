use serde_json::{json, Value};
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};

use super::files::{
    list_directory, read_file_at_root, write_file_authorized, LineEnding, TextEncoding,
    WorkspaceRoot,
};
use super::git::{
    checkout_workspace_git_branch, commit_workspace_git_changes, merge_workspace_git_branch,
    pull_workspace_git_changes, push_workspace_git_changes, rebase_workspace_git_branch,
    stage_all_workspace_git_files, stage_workspace_git_file, unstage_all_workspace_git_files,
    unstage_workspace_git_file, workspace_git_info, workspace_git_status,
};
use super::mcp::{read_stdout_message, write_proxy_message};
use super::scheduler_store::{SchedulerStore, TaskToolAuthority};
use super::shell::{run_shell_command_cancellable, ShellCommandRequest};

pub(crate) const TASK_TOOLS_AUTHORITY_ENV: &str = "SPACESLY_TASK_TOOLS_AUTHORITY";

pub fn run_task_tools_from_env() -> Result<(), String> {
    let encoded = std::env::var(TASK_TOOLS_AUTHORITY_ENV)
        .map_err(|_| "Task tool authority was not provided.".to_string())?;
    let authority: TaskToolAuthority = serde_json::from_str(&encoded)
        .map_err(|error| format!("Invalid task tool authority: {error}"))?;
    let root = authority
        .workspace_root
        .canonicalize()
        .map_err(|error| format!("Failed to resolve task tool workspace: {error}"))?;
    if root != authority.workspace_root || !root.is_dir() {
        return Err("Task tool workspace root changed or is not a directory.".to_string());
    }
    let workspace_roots = WorkspaceRoot::home()?;
    workspace_roots.set_path(&authority.workspace_id, root)?;

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    while let Some(request) = read_stdout_message(&mut reader)? {
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let response = match request.get("method").and_then(Value::as_str) {
            Some("initialize") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "spacesly-task-tools", "version": env!("CARGO_PKG_VERSION") }
                }
            }),
            Some("tools/list") => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": tool_definitions(&authority) }
            }),
            Some("tools/call") => match call_tool(&authority, &workspace_roots, &request) {
                Ok(value) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "content": [{ "type": "text", "text": value.to_string() }] }
                }),
                Err(error) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{ "type": "text", "text": error }],
                        "isError": true
                    }
                }),
            },
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "Method not found" }
            }),
        };
        write_proxy_message(&mut writer, &response)?;
    }
    Ok(())
}

fn tool_definitions(authority: &TaskToolAuthority) -> Vec<Value> {
    let has = |capability: &str| {
        authority
            .capabilities
            .iter()
            .any(|granted| granted == capability)
    };
    let mut tools = Vec::new();
    if has("workspace_read") {
        tools.push(json!({
            "name": "workspace_read",
            "description": "Read a UTF text file or list a directory inside the assigned workspace.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "list": { "type": "boolean", "default": false }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }));
    }
    if has("workspace_write") {
        tools.push(json!({
            "name": "workspace_write",
            "description": "Create or replace a UTF-8 file inside the assigned workspace. Replacing requires the version returned by workspace_read.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" },
                    "expected_version": { "type": "string" }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }
        }));
    }
    if has("shell") {
        tools.push(json!({
            "name": "shell",
            "description": "Run a cancellable shell command in the assigned workspace.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "workdir": { "type": "string" },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 300 }
                },
                "required": ["command"],
                "additionalProperties": false
            }
        }));
    }
    if has("git") {
        tools.push(json!({
            "name": "git",
            "description": "Run an allowlisted Git workspace operation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "operation": { "type": "string", "enum": ["info", "status", "stage_file", "stage_all", "unstage_file", "unstage_all", "checkout", "pull", "commit", "push", "merge", "rebase"] },
                    "path": { "type": "string" },
                    "branch": { "type": "string" },
                    "message": { "type": "string" }
                },
                "required": ["operation"],
                "additionalProperties": false
            }
        }));
    }
    tools
}

fn call_tool(
    authority: &TaskToolAuthority,
    roots: &WorkspaceRoot,
    request: &Value,
) -> Result<Value, String> {
    let params = request
        .get("params")
        .and_then(Value::as_object)
        .ok_or_else(|| "Task tool call params must be an object.".to_string())?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "Task tool name is required.".to_string())?;
    let arguments = params
        .get("arguments")
        .and_then(Value::as_object)
        .ok_or_else(|| "Task tool arguments must be an object.".to_string())?;
    match name {
        "workspace_read" => {
            authorize(authority, "workspace_read")?;
            let path = string_argument(arguments, "path")?;
            if arguments
                .get("list")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                serde_json::to_value(list_directory(
                    roots,
                    authority.workspace_id.clone(),
                    path.to_string(),
                )?)
                .map_err(|error| error.to_string())
            } else {
                serde_json::to_value(read_file_at_root(
                    roots,
                    authority.workspace_id.clone(),
                    path.to_string(),
                )?)
                .map_err(|error| error.to_string())
            }
        }
        "workspace_write" => {
            let path = string_argument(arguments, "path")?;
            let content = string_argument(arguments, "content")?;
            let expected_version = arguments
                .get("expected_version")
                .and_then(Value::as_str)
                .map(str::to_string);
            authorize(authority, "workspace_write")?;
            let write_authority = authority.clone();
            serde_json::to_value(write_file_authorized(
                roots,
                authority.workspace_id.clone(),
                path.to_string(),
                content.to_string(),
                expected_version,
                None,
                TextEncoding::Utf8,
                LineEnding::Lf,
                true,
                move || authorize(&write_authority, "workspace_write"),
            )?)
            .map_err(|error| error.to_string())
        }
        "shell" => {
            let command = string_argument(arguments, "command")?.to_string();
            let workdir = resolve_workdir(
                &authority.workspace_root,
                arguments.get("workdir").and_then(Value::as_str),
            )?;
            authorize(authority, "shell")?;
            let monitored_authority = authority.clone();
            serde_json::to_value(run_shell_command_cancellable(
                ShellCommandRequest {
                    command,
                    workdir: Some(workdir.to_string_lossy().to_string()),
                    timeout_seconds: arguments.get("timeout_seconds").and_then(Value::as_u64),
                },
                move || {
                    SchedulerStore::task_tool_authority_is_current(&monitored_authority, "shell")
                        .map(|current| !current)
                },
            )?)
            .map_err(|error| error.to_string())
        }
        "git" => {
            authorize(authority, "git")?;
            call_git(authority, roots, arguments)
        }
        _ => Err("Task tool was not exposed by this server.".to_string()),
    }
}

fn call_git(
    authority: &TaskToolAuthority,
    roots: &WorkspaceRoot,
    arguments: &serde_json::Map<String, Value>,
) -> Result<Value, String> {
    let workspace = authority.workspace_id.clone();
    let info = workspace_git_info(roots, workspace.clone())?;
    let repo_root = info
        .repo_root
        .as_deref()
        .ok_or_else(|| "Assigned workspace is not a Git repository.".to_string())?;
    let repo_root = PathBuf::from(repo_root)
        .canonicalize()
        .map_err(|error| format!("Failed to resolve Git repository root: {error}"))?;
    if repo_root != authority.workspace_root {
        return Err("Git repository root escapes the assigned workspace root.".to_string());
    }
    authorize(authority, "git")?;
    let value = match string_argument(arguments, "operation")? {
        "info" => serde_json::to_value(info),
        "status" => serde_json::to_value(workspace_git_status(roots, workspace)?),
        "stage_file" => serde_json::to_value(stage_workspace_git_file(
            roots,
            workspace,
            validate_relative_path(string_argument(arguments, "path")?)?,
        )?),
        "stage_all" => serde_json::to_value(stage_all_workspace_git_files(roots, workspace)?),
        "unstage_file" => serde_json::to_value(unstage_workspace_git_file(
            roots,
            workspace,
            validate_relative_path(string_argument(arguments, "path")?)?,
        )?),
        "unstage_all" => serde_json::to_value(unstage_all_workspace_git_files(roots, workspace)?),
        "checkout" => serde_json::to_value(checkout_workspace_git_branch(
            roots,
            workspace,
            string_argument(arguments, "branch")?.to_string(),
        )?),
        "pull" => serde_json::to_value(pull_workspace_git_changes(roots, workspace)?),
        "commit" => serde_json::to_value(commit_workspace_git_changes(
            roots,
            workspace,
            string_argument(arguments, "message")?.to_string(),
        )?),
        "push" => serde_json::to_value(push_workspace_git_changes(roots, workspace)?),
        "merge" => serde_json::to_value(merge_workspace_git_branch(
            roots,
            workspace,
            string_argument(arguments, "branch")?.to_string(),
        )?),
        "rebase" => serde_json::to_value(rebase_workspace_git_branch(
            roots,
            workspace,
            string_argument(arguments, "branch")?.to_string(),
        )?),
        _ => return Err("Git operation is not allowlisted.".to_string()),
    };
    value.map_err(|error| error.to_string())
}

fn validate_relative_path(path: &str) -> Result<String, String> {
    let path = Path::new(path);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("Git file path must remain inside the assigned workspace.".to_string());
    }
    Ok(path.to_string_lossy().to_string())
}

fn authorize(authority: &TaskToolAuthority, capability: &str) -> Result<(), String> {
    match SchedulerStore::task_tool_authority_is_current(authority, capability)? {
        true => Ok(()),
        false => Err(format!(
            "Task tool assignment is stale, cancelled, expired, or lacks capability '{capability}'."
        )),
    }
}

fn string_argument<'a>(
    arguments: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<&'a str, String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("Task tool argument '{name}' is required."))
}

fn resolve_workdir(root: &Path, relative: Option<&str>) -> Result<PathBuf, String> {
    let relative = relative.unwrap_or("");
    let path = Path::new(relative);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("Shell workdir must remain inside the assigned workspace.".to_string());
    }
    let resolved = root
        .join(path)
        .canonicalize()
        .map_err(|error| format!("Failed to resolve shell workdir: {error}"))?;
    if !resolved.starts_with(root) || !resolved.is_dir() {
        return Err("Shell workdir escapes the assigned workspace.".to_string());
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_workdir_rejects_path_escape() {
        let root = std::env::temp_dir().canonicalize().expect("temp root");
        assert!(resolve_workdir(&root, Some("../outside")).is_err());
        assert!(resolve_workdir(&root, Some("/tmp")).is_err());
    }

    #[test]
    fn git_file_path_rejects_escape() {
        assert!(validate_relative_path("../outside").is_err());
        assert!(validate_relative_path("/tmp/outside").is_err());
        assert_eq!(
            validate_relative_path("src/main.rs").unwrap(),
            "src/main.rs"
        );
    }

    #[test]
    fn capability_controls_exposed_tools() {
        let authority = TaskToolAuthority {
            scheduler_database: PathBuf::from("scheduler.db"),
            scheduler_instance_id: "instance".to_string(),
            session_id: crate::domain::task_session::TaskSessionId(1),
            attempt_id: 1,
            attempt: 1,
            owner_id: 1,
            fencing_token: 1,
            workspace_id: "workspace".to_string(),
            workspace_root: PathBuf::from("/workspace"),
            capabilities: vec!["workspace_read".to_string()],
        };
        let names = tool_definitions(&authority)
            .into_iter()
            .filter_map(|tool| tool["name"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert_eq!(names, ["workspace_read"]);
    }
}
