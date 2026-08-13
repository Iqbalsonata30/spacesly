use serde_json::{json, Value};
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::files::{
    list_directory, read_file_at_root, write_file_authorized, LineEnding, TextEncoding,
    WorkspaceRoot,
};
use super::git::{
    checkout_workspace_git_branch, commit_workspace_git_changes, merge_workspace_git_branch,
    pull_workspace_git_changes, push_workspace_git_changes, rebase_workspace_git_branch,
    repository_root_at, stage_all_workspace_git_files, stage_workspace_git_file,
    unstage_all_workspace_git_files, unstage_workspace_git_file, workspace_git_info,
    workspace_git_status,
};
use super::mcp::{read_stdout_message, write_proxy_message};
use super::scheduler_store::{SchedulerStore, SubtaskToolRisk, TaskToolAuthority};
use super::shell::{run_shell_command_cancellable, ShellCommandRequest};

pub(crate) const TASK_TOOLS_AUTHORITY_ENV: &str = "SPACESLY_TASK_TOOLS_AUTHORITY";
static TASK_TOOL_EVALUATION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
    if let Some(default_repository_root) = authority.default_repository_root.as_ref() {
        let canonical = default_repository_root
            .canonicalize()
            .map_err(|error| format!("Failed to resolve default Git repository: {error}"))?;
        if canonical != *default_repository_root
            || !canonical.starts_with(&root)
            || repository_root_at(&canonical)?.as_deref() != Some(canonical.as_path())
        {
            return Err(
                "Default Git repository is not an exact repository root inside the assigned workspace."
                    .to_string(),
            );
        }
    }
    if authority.bound_branch.as_ref().is_some_and(|branch| {
        branch.trim().is_empty()
            || branch != branch.trim()
            || branch.len() > 255
            || branch.chars().any(char::is_control)
    }) {
        return Err("Task-bound Git branch is invalid.".to_string());
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
                    "path": { "type": "string", "description": "Relative path or absolute path inside the assigned workspace." },
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
                    "path": { "type": "string", "description": "Relative path or absolute path inside the assigned workspace." },
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
                    "workdir": { "type": "string", "description": "Relative path or absolute path inside the assigned workspace." },
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
                    "workdir": { "type": "string", "description": "Repository directory inside the assigned workspace. Omit it when Task Examination resolved an authoritative default repository." },
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
            let path = resolve_workspace_file_path(
                &authority.workspace_root,
                string_argument(arguments, "path")?,
            )?;
            admit_subtask(authority, "workspace_read", SubtaskToolRisk::Read)?;
            if arguments
                .get("list")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                serde_json::to_value(list_directory(
                    roots,
                    authority.workspace_id.clone(),
                    path.clone(),
                )?)
                .map_err(|error| error.to_string())
            } else {
                serde_json::to_value(read_file_at_root(
                    roots,
                    authority.workspace_id.clone(),
                    path,
                )?)
                .map_err(|error| error.to_string())
            }
        }
        "workspace_write" => {
            let path = resolve_workspace_file_path(
                &authority.workspace_root,
                string_argument(arguments, "path")?,
            )?;
            let content = string_argument(arguments, "content")?;
            let expected_version = arguments
                .get("expected_version")
                .and_then(Value::as_str)
                .map(str::to_string);
            authorize(authority, "workspace_write")?;
            admit_subtask(authority, "workspace_write", SubtaskToolRisk::Mutation)?;
            let write_authority = authority.clone();
            serde_json::to_value(write_file_authorized(
                roots,
                authority.workspace_id.clone(),
                path,
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
            admit_subtask(authority, "shell", SubtaskToolRisk::Mutation)?;
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
            let operation = string_argument(arguments, "operation")?;
            admit_subtask(
                authority,
                "git",
                if matches!(operation, "info" | "status") {
                    SubtaskToolRisk::Read
                } else {
                    SubtaskToolRisk::Mutation
                },
            )?;
            call_git(authority, roots, arguments)
        }
        _ => Err("Task tool was not exposed by this server.".to_string()),
    }
}

fn admit_subtask(
    authority: &TaskToolAuthority,
    capability: &str,
    risk: SubtaskToolRisk,
) -> Result<(), String> {
    if let Some(subtask) = authority.subtask_authority.as_ref() {
        if subtask.scheduler_database != authority.scheduler_database
            || subtask.scheduler_instance_id != authority.scheduler_instance_id
            || subtask.session_id != authority.session_id
            || subtask.parent_attempt_id != authority.attempt_id
            || subtask.parent_attempt != authority.attempt
            || subtask.parent_owner_id != authority.owner_id
            || subtask.parent_fencing_token != authority.fencing_token
        {
            return Err(
                "Subtask authority does not match its parent workspace authority.".to_string(),
            );
        }
        SchedulerStore::admit_subtask_tool_call(subtask, capability, risk)?;
    }
    Ok(())
}

fn call_git(
    authority: &TaskToolAuthority,
    _roots: &WorkspaceRoot,
    arguments: &serde_json::Map<String, Value>,
) -> Result<Value, String> {
    let workspace = authority.workspace_id.clone();
    let requested_workdir = arguments.get("workdir").and_then(Value::as_str);
    let workdir = match (
        requested_workdir,
        authority.default_repository_root.as_ref(),
    ) {
        (None, Some(default)) => default.clone(),
        (Some(requested), Some(default)) => {
            let resolved = resolve_workdir(&authority.workspace_root, Some(requested))?;
            if &resolved != default {
                return Err(
                    "Git workdir conflicts with the repository resolved by Task Examination."
                        .to_string(),
                );
            }
            resolved
        }
        (requested, None) => resolve_workdir(&authority.workspace_root, requested)?,
    };
    let repo_root = repository_root_at(&workdir)?.ok_or_else(|| {
        "Git workdir is not inside a repository. Provide a repository directory within the assigned workspace using 'workdir'.".to_string()
    })?;
    let repo_root = repo_root
        .canonicalize()
        .map_err(|error| format!("Failed to resolve Git repository root: {error}"))?;
    if !repo_root.starts_with(&authority.workspace_root) {
        return Err("Git repository root escapes the assigned workspace root.".to_string());
    }
    let repo_roots = WorkspaceRoot::scoped(&workspace, &repo_root)?;
    let info = workspace_git_info(&repo_roots, workspace.clone())?;
    authorize(authority, "git")?;
    let operation = string_argument(arguments, "operation")?;
    if let Some(bound_branch) = authority.bound_branch.as_deref() {
        if operation == "checkout" && string_argument(arguments, "branch")? != bound_branch {
            return Err(format!(
                "Git checkout branch conflicts with the task-bound deployment branch '{bound_branch}'."
            ));
        }
        if matches!(
            operation,
            "stage_file"
                | "stage_all"
                | "unstage_file"
                | "unstage_all"
                | "pull"
                | "commit"
                | "push"
                | "merge"
                | "rebase"
        ) && info.current_branch.as_deref() != Some(bound_branch)
        {
            return Err(format!(
                "Git mutation requires the task-bound deployment branch '{bound_branch}', but the repository is on '{}'.",
                info.current_branch.as_deref().unwrap_or("detached HEAD")
            ));
        }
    }
    let value = match operation {
        "info" => serde_json::to_value(info),
        "status" => serde_json::to_value(workspace_git_status(&repo_roots, workspace)?),
        "stage_file" => serde_json::to_value(stage_workspace_git_file(
            &repo_roots,
            workspace,
            validate_relative_path(string_argument(arguments, "path")?)?,
        )?),
        "stage_all" => serde_json::to_value(stage_all_workspace_git_files(&repo_roots, workspace)?),
        "unstage_file" => serde_json::to_value(unstage_workspace_git_file(
            &repo_roots,
            workspace,
            validate_relative_path(string_argument(arguments, "path")?)?,
        )?),
        "unstage_all" => {
            serde_json::to_value(unstage_all_workspace_git_files(&repo_roots, workspace)?)
        }
        "checkout" => serde_json::to_value(checkout_workspace_git_branch(
            &repo_roots,
            workspace,
            string_argument(arguments, "branch")?.to_string(),
        )?),
        "pull" => serde_json::to_value(pull_workspace_git_changes(&repo_roots, workspace)?),
        "commit" => serde_json::to_value(commit_workspace_git_changes(
            &repo_roots,
            workspace,
            string_argument(arguments, "message")?.to_string(),
        )?),
        "push" => serde_json::to_value(push_workspace_git_changes(&repo_roots, workspace)?),
        "merge" => serde_json::to_value(merge_workspace_git_branch(
            &repo_roots,
            workspace,
            string_argument(arguments, "branch")?.to_string(),
        )?),
        "rebase" => serde_json::to_value(rebase_workspace_git_branch(
            &repo_roots,
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

fn resolve_workspace_file_path(root: &Path, path: &str) -> Result<String, String> {
    let expanded;
    let path = Path::new(path);
    let path = if path == Path::new("~") || path.starts_with("~/") {
        let relative = path
            .strip_prefix("~")
            .expect("home-relative path prefix was checked");
        expanded = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not configured for the workspace file path.".to_string())?
            .join(relative);
        expanded.as_path()
    } else {
        path
    };
    if !path.is_absolute() {
        return Ok(path.to_string_lossy().to_string());
    }
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().to_string())
        .map_err(|_| "Workspace file path escapes the assigned workspace.".to_string())
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
    if !path.is_absolute()
        && path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("Workdir must remain inside the assigned workspace.".to_string());
    }
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let resolved = candidate
        .canonicalize()
        .map_err(|error| format!("Failed to resolve workdir: {error}"))?;
    if !resolved.starts_with(root) || !resolved.is_dir() {
        return Err("Workdir escapes the assigned workspace.".to_string());
    }
    Ok(resolved)
}

pub(crate) fn evaluate_task_tool_containment_fixture(
    operation: crate::domain::agent_evaluation::TaskToolContainmentOperation,
    path: &str,
    symlink_to_outside: bool,
) -> crate::domain::agent_evaluation::TaskToolContainmentObservation {
    match evaluate_task_tool_containment_fixture_inner(operation, path, symlink_to_outside) {
        Ok(allowed) => crate::domain::agent_evaluation::TaskToolContainmentObservation {
            allowed,
            fixture_error: false,
        },
        Err(_) => crate::domain::agent_evaluation::TaskToolContainmentObservation {
            allowed: false,
            fixture_error: true,
        },
    }
}

fn evaluate_task_tool_containment_fixture_inner(
    operation: crate::domain::agent_evaluation::TaskToolContainmentOperation,
    path: &str,
    symlink_to_outside: bool,
) -> Result<bool, String> {
    let fixture = TaskToolEvaluationFixture::create()?;
    let workspace = fixture.root.join("workspace");
    let outside = fixture.root.join("outside");
    std::fs::create_dir_all(workspace.join("inside-dir"))
        .map_err(|error| format!("Failed to create evaluation workspace: {error}"))?;
    std::fs::create_dir_all(&outside)
        .map_err(|error| format!("Failed to create evaluation outside root: {error}"))?;
    std::fs::write(workspace.join("inside.txt"), "inside")
        .map_err(|error| format!("Failed to create contained evaluation file: {error}"))?;
    std::fs::write(outside.join("outside.txt"), "outside")
        .map_err(|error| format!("Failed to create outside evaluation file: {error}"))?;
    let workspace = workspace
        .canonicalize()
        .map_err(|error| format!("Failed to resolve evaluation workspace: {error}"))?;
    let outside = outside
        .canonicalize()
        .map_err(|error| format!("Failed to resolve evaluation outside root: {error}"))?;
    let path = task_tool_evaluation_path(path, &workspace, &outside);
    let path = configure_task_tool_symlink(path, &workspace, &outside, symlink_to_outside)?;
    match operation {
        crate::domain::agent_evaluation::TaskToolContainmentOperation::WorkspaceRead => {
            let roots = WorkspaceRoot::scoped("evaluation-workspace", &workspace)?;
            Ok(resolve_workspace_file_path(&workspace, &path)
                .and_then(|path| {
                    read_file_at_root(&roots, "evaluation-workspace".to_string(), path).map(|_| ())
                })
                .is_ok())
        }
        crate::domain::agent_evaluation::TaskToolContainmentOperation::ShellWorkdir => {
            Ok(resolve_workdir(&workspace, Some(&path)).is_ok())
        }
        crate::domain::agent_evaluation::TaskToolContainmentOperation::GitFile => {
            Ok(validate_relative_path(&path).is_ok())
        }
    }
}

struct TaskToolEvaluationFixture {
    root: PathBuf,
}

impl TaskToolEvaluationFixture {
    fn create() -> Result<Self, String> {
        for _ in 0..128 {
            let sequence = TASK_TOOL_EVALUATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "spacesly-agent-task-tool-evaluation-{}-{sequence}",
                std::process::id()
            ));
            match std::fs::create_dir(&root) {
                Ok(()) => return Ok(Self { root }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "Failed to create task-tool evaluation root: {error}"
                    ));
                }
            }
        }
        Err("Could not allocate a unique task-tool evaluation root.".to_string())
    }
}

impl Drop for TaskToolEvaluationFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn task_tool_evaluation_path(path: &str, workspace: &Path, outside: &Path) -> String {
    path.replace("{{workspace}}", &workspace.to_string_lossy())
        .replace("{{outside}}", &outside.to_string_lossy())
}

#[cfg(unix)]
fn configure_task_tool_symlink(
    path: String,
    workspace: &Path,
    outside: &Path,
    enabled: bool,
) -> Result<String, String> {
    if enabled {
        std::os::unix::fs::symlink(outside, workspace.join("outside-link"))
            .map_err(|error| format!("Failed to create evaluation symlink: {error}"))?;
    }
    Ok(path)
}

#[cfg(not(unix))]
fn configure_task_tool_symlink(
    path: String,
    _workspace: &Path,
    outside: &Path,
    enabled: bool,
) -> Result<String, String> {
    if enabled {
        Ok(path.replacen("outside-link", &outside.to_string_lossy(), 1))
    } else {
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::task_session::{
        TaskRequest, TaskSessionEnvelope, TaskSessionEnvelopeV1, TaskSessionKind,
    };
    use crate::infrastructure::scheduler_store::SubtaskToolAuthority;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    #[test]
    fn fenced_shell_tool_returns_response_after_file_creation() {
        let directory = tempdir().expect("test directory");
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace directory");
        let root = workspace.canonicalize().expect("workspace root");
        let store = SchedulerStore::open_at(directory.path().join("scheduler.db"))
            .expect("scheduler store");
        let owner = store.register_owner().expect("scheduler owner");
        store
            .enqueue_with_grants(
                &TaskRequest {
                    label: "create-file".to_string(),
                    payload: serde_json::to_string(&TaskSessionEnvelope::V1(
                        TaskSessionEnvelopeV1 {
                            workspace_id: "workspace-test".to_string(),
                            kind: TaskSessionKind::Agent,
                            subject_id: Some("card-1".to_string()),
                            conversation_id: Some("conversation-1".to_string()),
                            execution_run_id: Some("run-1".to_string()),
                            context_digest: "digest".to_string(),
                            runtime_profile_id: "profile-1".to_string(),
                            model: "openai/test".to_string(),
                            connector_ids: Vec::new(),
                            requested_capabilities: vec!["shell".to_string()],
                            prompt_template_version: "agent-v1".to_string(),
                            context_revision: Some("1".to_string()),
                            rules_revision: Some("rules".to_string()),
                            skills_revision: Some("skills".to_string()),
                        },
                    ))
                    .expect("task envelope"),
                },
                &["shell".to_string()],
                "test",
            )
            .expect("task enqueued");
        let assignment = store
            .claim_next(owner, 1, Duration::from_secs(30), 1)
            .expect("claim succeeds")
            .expect("assignment");
        let authority = store
            .task_tool_authority(
                assignment.fence,
                "workspace-test",
                root.clone(),
                &["shell".to_string()],
            )
            .expect("tool authority");
        let roots = WorkspaceRoot::home().expect("workspace roots");
        roots
            .set_path("workspace-test", root.clone())
            .expect("workspace registered");
        let started = Instant::now();
        let response = call_tool(
            &authority,
            &roots,
            &json!({
                "params": {
                    "name": "shell",
                    "arguments": {
                        "command": "printf 'created' > created.txt && test -f created.txt",
                        "workdir": root,
                        "timeout_seconds": 5
                    }
                }
            }),
        )
        .expect("shell tool response");

        assert_eq!(response["exit_code"], 0);
        assert_eq!(
            std::fs::read_to_string(workspace.join("created.txt")).unwrap(),
            "created"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn shell_workdir_accepts_absolute_paths_only_inside_assigned_workspace() {
        let directory = tempdir().expect("workspace");
        let root = directory.path().canonicalize().expect("workspace root");
        let child = root.join("child");
        std::fs::create_dir(&child).expect("child directory");

        assert_eq!(resolve_workdir(&root, None).unwrap(), root);
        assert_eq!(resolve_workdir(&root, Some("child")).unwrap(), child);
        assert_eq!(
            resolve_workdir(&root, Some(root.to_string_lossy().as_ref())).unwrap(),
            root
        );
        assert_eq!(
            resolve_workdir(&root, Some(child.to_string_lossy().as_ref())).unwrap(),
            child
        );
        assert!(resolve_workdir(&root, Some("../outside")).is_err());
        assert!(
            resolve_workdir(&root, Some(std::env::temp_dir().to_string_lossy().as_ref())).is_err()
        );
    }

    #[test]
    fn workspace_file_path_accepts_absolute_paths_only_inside_assigned_workspace() {
        let directory = tempdir().expect("workspace");
        let root = directory.path().canonicalize().expect("workspace root");

        assert_eq!(
            resolve_workspace_file_path(&root, "iqbalsonata.txt").unwrap(),
            "iqbalsonata.txt"
        );
        assert_eq!(
            resolve_workspace_file_path(&root, root.join("iqbalsonata.txt").to_str().unwrap())
                .unwrap(),
            "iqbalsonata.txt"
        );
        assert!(resolve_workspace_file_path(&root, "/tmp/iqbalsonata.txt").is_err());
    }

    #[test]
    fn shell_workdir_resolution_has_no_cross_workspace_state() {
        let first = tempdir().expect("first workspace");
        let second = tempdir().expect("second workspace");
        let first_root = first.path().canonicalize().expect("first root");
        let second_root = second.path().canonicalize().expect("second root");

        assert_eq!(
            resolve_workdir(&first_root, Some(first_root.to_string_lossy().as_ref())).unwrap(),
            first_root
        );
        assert_eq!(
            resolve_workdir(&second_root, Some(second_root.to_string_lossy().as_ref())).unwrap(),
            second_root
        );
        assert!(
            resolve_workdir(&first_root, Some(second_root.to_string_lossy().as_ref())).is_err()
        );
        assert!(
            resolve_workdir(&second_root, Some(first_root.to_string_lossy().as_ref())).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_workdir_rejects_symlink_escape_after_canonicalization() {
        let workspace = tempdir().expect("workspace");
        let outside = tempdir().expect("outside");
        let root = workspace.path().canonicalize().expect("workspace root");
        let link = root.join("outside-link");
        std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");

        assert!(resolve_workdir(&root, Some("outside-link")).is_err());
        assert!(resolve_workdir(&root, Some(link.to_string_lossy().as_ref())).is_err());
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
    fn git_tool_operates_on_nested_repository_within_assigned_workspace() {
        let directory = tempdir().expect("test directory");
        let workspace = directory.path().join("workspace");
        let repository = workspace.join("BRI").join("qcash-deployment");
        std::fs::create_dir_all(&repository).expect("repository directory");
        let git = crate::infrastructure::git::git_executable().expect("git executable");
        let initialized = std::process::Command::new(git)
            .args(["init", "--quiet"])
            .current_dir(&repository)
            .status()
            .expect("git init runs");
        assert!(initialized.success());

        let root = workspace.canonicalize().expect("workspace root");
        let store = SchedulerStore::open_at(directory.path().join("scheduler.db"))
            .expect("scheduler store");
        let owner = store.register_owner().expect("scheduler owner");
        store
            .enqueue_with_grants(
                &TaskRequest {
                    label: "nested-git".to_string(),
                    payload: serde_json::to_string(&TaskSessionEnvelope::V1(
                        TaskSessionEnvelopeV1 {
                            workspace_id: "workspace-test".to_string(),
                            kind: TaskSessionKind::Agent,
                            subject_id: Some("card-git".to_string()),
                            conversation_id: Some("conversation-git".to_string()),
                            execution_run_id: Some("run-git".to_string()),
                            context_digest: "digest".to_string(),
                            runtime_profile_id: "profile-git".to_string(),
                            model: "openai/test".to_string(),
                            connector_ids: Vec::new(),
                            requested_capabilities: vec!["git".to_string()],
                            prompt_template_version: "agent-v1".to_string(),
                            context_revision: Some("1".to_string()),
                            rules_revision: Some("rules".to_string()),
                            skills_revision: Some("skills".to_string()),
                        },
                    ))
                    .expect("task envelope"),
                },
                &["git".to_string()],
                "test",
            )
            .expect("task enqueued");
        let assignment = store
            .claim_next(owner, 1, Duration::from_secs(30), 1)
            .expect("claim succeeds")
            .expect("assignment");
        let mut authority = store
            .task_tool_authority(
                assignment.fence,
                "workspace-test",
                root.clone(),
                &["git".to_string()],
            )
            .expect("tool authority");
        let roots = WorkspaceRoot::scoped("workspace-test", &root).expect("workspace registered");

        let response = call_tool(
            &authority,
            &roots,
            &json!({
                "params": {
                    "name": "git",
                    "arguments": {
                        "operation": "info",
                        "workdir": "BRI/qcash-deployment"
                    }
                }
            }),
        )
        .expect("nested repository inspected");

        assert_eq!(response["is_git_repo"], true);
        assert_eq!(
            response["repo_root"],
            repository
                .canonicalize()
                .expect("repository root")
                .to_string_lossy()
                .as_ref()
        );
        let missing_workdir = call_tool(
            &authority,
            &roots,
            &json!({
                "params": {
                    "name": "git",
                    "arguments": { "operation": "info" }
                }
            }),
        )
        .expect_err("workspace root itself is not a repository");
        assert!(missing_workdir.contains("Provide a repository directory"));

        authority.default_repository_root =
            Some(repository.canonicalize().expect("default repository root"));
        let default_response = call_tool(
            &authority,
            &roots,
            &json!({
                "params": {
                    "name": "git",
                    "arguments": { "operation": "info" }
                }
            }),
        )
        .expect("resolved default repository inspected");
        assert_eq!(default_response["is_git_repo"], true);
        assert_eq!(default_response["repo_root"], response["repo_root"]);

        let conflicting = call_tool(
            &authority,
            &roots,
            &json!({
                "params": {
                    "name": "git",
                    "arguments": { "operation": "info", "workdir": "." }
                }
            }),
        )
        .expect_err("resolved repository cannot be overridden");
        assert!(conflicting.contains("conflicts with the repository resolved"));

        authority.bound_branch = Some("prerelease".to_string());
        let wrong_checkout = call_tool(
            &authority,
            &roots,
            &json!({
                "params": {
                    "name": "git",
                    "arguments": {
                        "operation": "checkout",
                        "branch": "main"
                    }
                }
            }),
        )
        .expect_err("different deployment branch rejected");
        assert!(wrong_checkout.contains("task-bound deployment branch 'prerelease'"));
        let wrong_current_branch = call_tool(
            &authority,
            &roots,
            &json!({
                "params": {
                    "name": "git",
                    "arguments": { "operation": "stage_all" }
                }
            }),
        )
        .expect_err("mutation on wrong branch rejected");
        assert!(wrong_current_branch.contains("requires the task-bound deployment branch"));
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
            default_repository_root: None,
            bound_branch: None,
            capabilities: vec!["workspace_read".to_string()],
            subtask_authority: None,
        };
        let names = tool_definitions(&authority)
            .into_iter()
            .filter_map(|tool| tool["name"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert_eq!(names, ["workspace_read"]);
    }

    #[test]
    fn workspace_tool_rejects_an_unbound_subtask_fence_before_file_access() {
        let directory = tempdir().expect("test directory");
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace directory");
        std::fs::write(workspace.join("evidence.txt"), "safe").expect("fixture writes");
        let root = workspace.canonicalize().expect("workspace root");
        let database = directory.path().join("scheduler.db");
        let store = SchedulerStore::open_at(database.clone()).expect("scheduler store");
        let owner = store.register_owner().expect("scheduler owner");
        let session = store
            .enqueue_with_grants(
                &TaskRequest::from_envelope(
                    "subtask-workspace",
                    &TaskSessionEnvelope::V1(TaskSessionEnvelopeV1 {
                        workspace_id: "workspace-test".to_string(),
                        kind: TaskSessionKind::Agent,
                        subject_id: None,
                        conversation_id: Some("subtask-workspace".to_string()),
                        execution_run_id: Some("subtask-workspace-run".to_string()),
                        context_digest: "digest".to_string(),
                        runtime_profile_id: "profile".to_string(),
                        model: "openai/test".to_string(),
                        connector_ids: Vec::new(),
                        requested_capabilities: vec!["workspace_read".to_string()],
                        prompt_template_version: "agent-v1".to_string(),
                        context_revision: None,
                        rules_revision: None,
                        skills_revision: None,
                    }),
                )
                .expect("request encodes"),
                &["workspace_read".to_string()],
                "test",
            )
            .expect("task enqueued");
        let assignment = store
            .claim_next(owner, 1, Duration::from_secs(30), 1)
            .expect("claim succeeds")
            .expect("assignment exists");
        let mut authority = store
            .task_tool_authority(
                assignment.fence,
                "workspace-test",
                root.clone(),
                &["workspace_read".to_string()],
            )
            .expect("parent authority");
        authority.subtask_authority = Some(SubtaskToolAuthority {
            scheduler_database: database,
            scheduler_instance_id: store.instance_id().to_string(),
            session_id: session.id,
            parent_attempt_id: assignment.fence.attempt_id,
            parent_attempt: assignment.fence.attempt,
            parent_owner_id: assignment.fence.owner_id,
            parent_fencing_token: assignment.fence.fencing_token,
            subtask_id: 999,
            subtask_attempt_id: 999,
            subtask_attempt: 1,
            subtask_fencing_token: 1,
            authority_id: 999,
            authority_fencing_token: 1,
            objective_id: "unbound-objective".to_string(),
            capabilities: vec!["workspace_read".to_string()],
            allowed_connector_tools: std::collections::BTreeMap::new(),
            lease_expires_at: i64::MAX as u64,
        });
        let roots = WorkspaceRoot::scoped("workspace-test", &root).expect("workspace registered");
        let error = call_tool(
            &authority,
            &roots,
            &json!({
                "params": {
                    "name": "workspace_read",
                    "arguments": { "path": "evidence.txt" }
                }
            }),
        )
        .expect_err("unbound subtask must fail before file access");
        assert!(error.contains("Subtask tool authority is stale"));
    }
}
