use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::ErrorKind;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::global_environment::redact_global_environment_values;
use super::jira_rest;
use super::ocp::trusted_resource_operation_identity_from_environment;
#[cfg(test)]
use super::scheduler_store::SubtaskToolAuthority;
use super::scheduler_store::{
    ExternalAssignmentAuthority, ResourceMutationRecord, ResourceMutationReservation,
    ResourceMutationResolution, ResourceMutationState, SchedulerStore,
};
use super::shell_env::inject_shell_env;
use super::tool_broker::ToolBroker;
use crate::domain::resource_idempotency::{
    ResourceExecutionResult, ResourceExecutionStatus, ResourceLookupResult, ResourceLookupStatus,
    ResourceMutationEvidence, ResourceOperationIdentity, ResourceRetryResumeStatus,
};
use crate::domain::task_examination::{
    ConnectorCapabilitySnapshot, ConnectorDiscoveryStatus, DiscoveredToolCapability,
};

const MCP_STDERR_LIMIT: usize = 64 * 1024;
const MCP_MESSAGE_LIMIT: usize = 8 * 1024 * 1024;
const MCP_HEADER_LINE_LIMIT: usize = 8 * 1024;
const MCP_SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MCP_MAX_SESSIONS: usize = 8;
const MCP_CAPABILITY_TOOL_LIMIT: usize = 128;
const MCP_CAPABILITY_ARGUMENT_LIMIT: usize = 64;
const MCP_PROXY_COMMAND_ENV: &str = "SPACESLY_MCP_PROXY_COMMAND";
pub(crate) const MCP_PROXY_AUTHORITY_ENV: &str = "SPACESLY_MCP_PROXY_AUTHORITY";
pub(crate) const MCP_PROXY_AUTHORITY_MODE_ENV: &str = "SPACESLY_MCP_PROXY_AUTHORITY_MODE";
pub(crate) const MCP_PROXY_AUTHORITY_MODE_LEGACY: &str = "legacy";
pub(crate) const MCP_PROXY_AUTHORITY_MODE_REQUIRED: &str = "required";
pub(crate) const MCP_PROXY_CONNECTOR_ID_ENV: &str = "SPACESLY_MCP_PROXY_CONNECTOR_ID";
pub(crate) const MCP_PROXY_CONNECTOR_BINDING_ENV: &str = "SPACESLY_MCP_PROXY_CONNECTOR_BINDING";

enum ProxyAssignmentAuthority {
    Legacy,
    Fenced(ExternalAssignmentAuthority),
}

#[derive(Clone)]
struct PendingResourceMutation {
    authority: ExternalAssignmentAuthority,
    record: ResourceMutationRecord,
    request_id: Value,
}

struct UnansweredMutationResolution {
    resolved: Vec<(Value, ResourceMutationRecord)>,
    error: Option<String>,
}

struct ProxyChildGuard {
    child: Arc<Mutex<Child>>,
    armed: bool,
}

impl ProxyChildGuard {
    fn new(child: Child) -> Self {
        Self {
            child: Arc::new(Mutex::new(child)),
            armed: true,
        }
    }

    fn child(&self) -> Arc<Mutex<Child>> {
        Arc::clone(&self.child)
    }

    fn terminate_and_wait(mut self) -> Result<std::process::ExitStatus, String> {
        let mut child = self.child.lock().map_err(|error| error.to_string())?;
        terminate_proxy_process(&mut child);
        let status = child
            .wait()
            .map_err(|error| format!("Failed to wait for proxied MCP connector: {error}"))?;
        drop(child);
        self.armed = false;
        Ok(status)
    }
}

impl Drop for ProxyChildGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(mut child) = self.child.lock() {
            terminate_proxy_process(&mut child);
            let _ = child.wait();
        }
    }
}

fn terminate_proxy_process(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

pub fn run_mcp_proxy_from_env() -> Result<(), String> {
    let command_json = std::env::var(MCP_PROXY_COMMAND_ENV)
        .map_err(|_| "MCP proxy connector command was not provided.".to_string())?;
    let command_parts: Vec<String> = serde_json::from_str(&command_json)
        .map_err(|error| format!("Invalid MCP proxy connector command: {error}"))?;
    let authority_mode = optional_unicode_environment(MCP_PROXY_AUTHORITY_MODE_ENV)?;
    let authority_json = optional_unicode_environment(MCP_PROXY_AUTHORITY_ENV)?;
    let authority =
        parse_proxy_assignment_authority(authority_mode.as_deref(), authority_json.as_deref())?;
    let connector_id = required_unicode_environment(MCP_PROXY_CONNECTOR_ID_ENV)?;
    let connector_binding = required_unicode_environment(MCP_PROXY_CONNECTOR_BINDING_ENV)?;
    validate_connector_binding_value(&connector_binding)?;

    let identity_connector_binding = connector_binding.clone();
    run_mcp_proxy_with_io(
        command_parts,
        authority,
        connector_id,
        connector_binding,
        std::io::stdin(),
        std::io::stdout(),
        move |command, tool_name, arguments| {
            trusted_resource_operation_identity_from_environment(command, tool_name, arguments)?
                .map_or_else(
                    || {
                        super::jira::jira_comment_operation_identity(
                            &identity_connector_binding,
                            tool_name,
                            arguments,
                        )
                    },
                    |identity| Ok(Some(identity)),
                )
        },
    )
}

fn run_mcp_proxy_with_io<R, W, F>(
    command_parts: Vec<String>,
    authority: ProxyAssignmentAuthority,
    connector_id: String,
    connector_binding: String,
    client_input: R,
    client_output: W,
    identity_resolver: F,
) -> Result<(), String>
where
    R: std::io::Read + Send + 'static,
    W: Write + Send + 'static,
    F: Fn(&[String], &str, &Value) -> Result<Option<ResourceOperationIdentity>, String>
        + Send
        + Sync
        + 'static,
{
    let (executable, args) = command_parts
        .split_first()
        .ok_or_else(|| "MCP proxy connector command is empty.".to_string())?;
    if executable.trim().is_empty() {
        return Err("MCP proxy connector executable is empty.".to_string());
    }

    let mut command = Command::new(executable);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .args(args)
        .env_remove(MCP_PROXY_COMMAND_ENV)
        .env_remove(MCP_PROXY_AUTHORITY_ENV)
        .env_remove(MCP_PROXY_AUTHORITY_MODE_ENV)
        .env_remove(MCP_PROXY_CONNECTOR_ID_ENV)
        .env_remove(MCP_PROXY_CONNECTOR_BINDING_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to start proxied MCP connector: {error}"))?;
    let upstream_stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Failed to open proxied MCP stdin.".to_string())?;
    let upstream_stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to open proxied MCP stdout.".to_string())?;
    let child = ProxyChildGuard::new(child);
    let exposed_tools = Arc::new(Mutex::new(Vec::<String>::new()));
    let pending_tool_lists = Arc::new(Mutex::new(HashSet::<String>::new()));
    let pending_resource_mutations =
        Arc::new(Mutex::new(HashMap::<String, PendingResourceMutation>::new()));
    let client_stdout = Arc::new(Mutex::new(client_output));
    let request_tools = Arc::clone(&exposed_tools);
    let request_lists = Arc::clone(&pending_tool_lists);
    let request_stdout = Arc::clone(&client_stdout);
    let request_mutations = Arc::clone(&pending_resource_mutations);
    let proxy_stopping = Arc::new(AtomicBool::new(false));
    let request_stopping = Arc::clone(&proxy_stopping);
    let request_activity = Arc::new(Mutex::new(()));
    let request_activity_thread = Arc::clone(&request_activity);
    let request_child = child.child();
    let request_command = command_parts.clone();

    let (request_result_sender, request_result_receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut result = (|| -> Result<(), String> {
            let mut client_reader = BufReader::new(client_input);
            let mut upstream_writer = upstream_stdin;
            while let Some(message) = read_stdout_message(&mut client_reader)? {
                let activity = request_activity_thread
                    .lock()
                    .map_err(|error| error.to_string())?;
                if request_stopping.load(Ordering::SeqCst) {
                    return Ok(());
                }
                let method = message.get("method").and_then(Value::as_str);
                if method == Some("tools/list") {
                    if let Some(id) = message.get("id") {
                        request_lists
                            .lock()
                            .map_err(|error| error.to_string())?
                            .insert(id.to_string());
                    }
                }
                if method == Some("tools/call") {
                    if let Err(error) = validate_proxy_request(
                        &message,
                        &request_tools,
                        &authority,
                        &connector_id,
                        &connector_binding,
                    ) {
                        drop(activity);
                        write_proxy_error(&request_stdout, message.get("id"), &error)?;
                        continue;
                    }
                    let ProxyAssignmentAuthority::Fenced(fenced_authority) = &authority else {
                        drop(activity);
                        write_proxy_message(&mut upstream_writer, &message)?;
                        continue;
                    };
                    let params = message
                        .get("params")
                        .and_then(Value::as_object)
                        .ok_or_else(|| {
                            "MCP tool call did not include object params.".to_string()
                        })?;
                    let tool_name = params.get("name").and_then(Value::as_str).unwrap_or("");
                    let arguments = params.get("arguments").unwrap_or(&Value::Null);
                    let trusted_identity =
                        match identity_resolver(&request_command, tool_name, arguments) {
                            Ok(identity) => identity,
                            Err(error) => {
                                drop(activity);
                                write_proxy_error(&request_stdout, message.get("id"), &error)?;
                                continue;
                            }
                        };
                    if let Some(identity) = trusted_identity {
                        let request_id = match validated_resource_mutation_request_id(&message) {
                            Ok(id) => id,
                            Err(error) => {
                                drop(activity);
                                write_proxy_error(&request_stdout, message.get("id"), &error)?;
                                continue;
                            }
                        };
                        let reservation = match SchedulerStore::reserve_external_resource_mutation(
                            fenced_authority,
                            tool_name,
                            &identity,
                        ) {
                            Ok(reservation) => reservation,
                            Err(error) => {
                                drop(activity);
                                write_proxy_error(&request_stdout, Some(&request_id), &error)?;
                                continue;
                            }
                        };
                        match reservation {
                            ResourceMutationReservation::Blocked(record) => {
                                drop(activity);
                                if record.state == ResourceMutationState::Succeeded
                                    && record.identity.connector == "jira"
                                    && write_succeeded_jira_comment_replay(
                                        &request_stdout,
                                        &request_id,
                                        &record,
                                    )?
                                {
                                    continue;
                                }
                                write_resource_mutation_result(
                                    &request_stdout,
                                    &request_id,
                                    "resource_mutation_blocked",
                                    &record,
                                    "A retained resource mutation fence requires operator review or explicit supersede. Do not retry.",
                                )?;
                                continue;
                            }
                            ResourceMutationReservation::Reserved(record) => {
                                let key = request_id.to_string();
                                let mut pending = request_mutations
                                    .lock()
                                    .map_err(|error| error.to_string())?;
                                if request_stopping.load(Ordering::SeqCst) {
                                    drop(pending);
                                    SchedulerStore::resolve_external_resource_mutation(
                                        fenced_authority,
                                        record.mutation_id,
                                        uncertain_resolution(
                                            "lifecycle",
                                            "proxy_terminated_before_dispatch",
                                        ),
                                    )
                                    .map_err(|error| {
                                        format!(
                                            "Failed to durably fence a mutation reserved during proxy shutdown: {error}"
                                        )
                                    })?;
                                    continue;
                                }
                                if pending.contains_key(&key) {
                                    SchedulerStore::resolve_external_resource_mutation(
                                        fenced_authority,
                                        record.mutation_id,
                                        uncertain_resolution("protocol", "duplicate_request_id"),
                                    )
                                    .map_err(|error| {
                                        format!(
                                            "Failed to durably fence a duplicate resource mutation request ID: {error}"
                                        )
                                    })?;
                                    drop(pending);
                                    drop(activity);
                                    write_proxy_error(
                                        &request_stdout,
                                        Some(&request_id),
                                        "Duplicate in-flight MCP request ID was rejected.",
                                    )?;
                                    continue;
                                }
                                pending.insert(
                                    key,
                                    PendingResourceMutation {
                                        authority: fenced_authority.clone(),
                                        record,
                                        request_id,
                                    },
                                );
                            }
                        }
                    }
                }
                drop(activity);
                if let Err(error) = write_proxy_message(&mut upstream_writer, &message) {
                    if let Some(id) = message.get("id") {
                        if let Some(pending) = request_mutations
                            .lock()
                            .map_err(|lock_error| lock_error.to_string())?
                            .remove(&id.to_string())
                        {
                            let record = SchedulerStore::resolve_external_resource_mutation(
                                &pending.authority,
                                pending.record.mutation_id,
                                uncertain_resolution("transport", "upstream_write_failed"),
                            )
                            .map_err(|resolution_error| {
                                format!(
                                    "Failed to durably mark the unwritten resource mutation uncertain: {resolution_error}. Upstream write failure: {error}"
                                )
                            })?;
                            write_resource_mutation_result(
                                &request_stdout,
                                &pending.request_id,
                                "resource_mutation_uncertain",
                                &record,
                                "The connector request could not be confirmed. Operator review is required. Do not retry.",
                            )?;
                        }
                    }
                    return Err(error);
                }
            }
            Ok(())
        })();
        let unresolved = if result.is_err() {
            request_stopping.store(true, Ordering::SeqCst);
            match resolve_unanswered_resource_mutations(&request_mutations, "client_request_error")
            {
                Ok(resolution) => {
                    if let Some(error) = resolution.error {
                        result = Err(error);
                    }
                    resolution.resolved
                }
                Err(error) => {
                    result = Err(error);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let failed = result.is_err();
        let _ = request_result_sender.send(result);
        if failed {
            if let Ok(mut child) = request_child.lock() {
                terminate_proxy_process(&mut child);
            }
            let _ = write_uncertain_resource_mutation_results(
                Arc::clone(&request_stdout),
                unresolved,
                "The proxy request stream terminated before the connector response was confirmed. Operator review is required. Do not retry.",
            );
        }
    });

    let mut upstream_reader = BufReader::new(upstream_stdout);
    loop {
        let mut message = match read_stdout_message(&mut upstream_reader) {
            Ok(Some(message)) => message,
            Ok(None) => break,
            Err(error) => {
                proxy_stopping.store(true, Ordering::SeqCst);
                let _activity = request_activity.lock().map_err(|error| error.to_string())?;
                let resolution = resolve_unanswered_resource_mutations(
                    &pending_resource_mutations,
                    "upstream_protocol_error",
                )?;
                if let Ok(mut child) = child.child.lock() {
                    terminate_proxy_process(&mut child);
                }
                write_uncertain_resource_mutation_results(
                    Arc::clone(&client_stdout),
                    resolution.resolved,
                    "The connector response was malformed or unreadable. Operator review is required. Do not retry.",
                )?;
                if let Some(resolution_error) = resolution.error {
                    return Err(format!(
                        "{resolution_error} Upstream protocol failure: {error}"
                    ));
                }
                return Err(error);
            }
        };
        if let Some(id) = message.get("id") {
            let is_tool_list = pending_tool_lists
                .lock()
                .map_err(|error| error.to_string())?
                .remove(&id.to_string());
            if is_tool_list {
                let tools = message
                    .get("result")
                    .and_then(|result| result.get("tools"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| tool.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect();
                *exposed_tools.lock().map_err(|error| error.to_string())? = tools;
            }
            if let Some(pending) = pending_resource_mutations
                .lock()
                .map_err(|error| error.to_string())?
                .remove(&id.to_string())
            {
                let resolution = classify_resource_mutation_response(&message, &pending.record);
                if let ResourceMutationResolution::Succeeded(evidence) = &resolution {
                    enrich_resource_mutation_response(&mut message, evidence);
                }
                if let Err(resolution_error) = SchedulerStore::resolve_external_resource_mutation(
                    &pending.authority,
                    pending.record.mutation_id,
                    resolution,
                ) {
                    proxy_stopping.store(true, Ordering::SeqCst);
                    let _activity = request_activity.lock().map_err(|error| error.to_string())?;
                    let unresolved = resolve_unanswered_resource_mutations(
                        &pending_resource_mutations,
                        "ledger_resolution_failed",
                    )?;
                    if let Ok(mut child) = child.child.lock() {
                        terminate_proxy_process(&mut child);
                    }
                    write_uncertain_resource_mutation_results(
                        Arc::clone(&client_stdout),
                        unresolved.resolved,
                        "A connector response could not be committed to the mutation ledger. Operator review is required. Do not retry.",
                    )?;
                    if let Some(error) = unresolved.error {
                        return Err(format!(
                            "The connector response and additional pending mutations could not be committed to the mutation ledger: {resolution_error}. Additional failure: {error}"
                        ));
                    }
                    return Err(format!(
                        "The connector response could not be committed to the mutation ledger: {resolution_error}"
                    ));
                }
            }
        }
        let mut stdout = client_stdout.lock().map_err(|error| error.to_string())?;
        if let Err(error) = write_proxy_message(&mut *stdout, &message) {
            drop(stdout);
            proxy_stopping.store(true, Ordering::SeqCst);
            let _activity = request_activity.lock().map_err(|error| error.to_string())?;
            let resolution = resolve_unanswered_resource_mutations(
                &pending_resource_mutations,
                "client_output_error",
            )?;
            if let Ok(mut child) = child.child.lock() {
                terminate_proxy_process(&mut child);
            }
            write_uncertain_resource_mutation_results(
                Arc::clone(&client_stdout),
                resolution.resolved,
                "The connector response could not be returned to the client. Operator review is required. Do not retry.",
            )?;
            if let Some(resolution_error) = resolution.error {
                return Err(resolution_error);
            }
            return Err(error);
        }
    }
    proxy_stopping.store(true, Ordering::SeqCst);
    let _activity = request_activity.lock().map_err(|error| error.to_string())?;
    let resolution =
        resolve_unanswered_resource_mutations(&pending_resource_mutations, "upstream_eof")?;
    drop(_activity);
    let status = child.terminate_and_wait()?;
    write_uncertain_resource_mutation_results(
        Arc::clone(&client_stdout),
        resolution.resolved,
        "The connector exited without a confirmed mutation response. Operator review is required. Do not retry.",
    )?;
    if let Some(error) = resolution.error {
        return Err(error);
    }
    let request_result = request_result_receiver.try_recv().ok();
    if let Some(Err(error)) = request_result {
        Err(format!("MCP proxy request reader failed: {error}"))
    } else if status.success() {
        Ok(())
    } else {
        Err(format!("Proxied MCP connector exited with {status}."))
    }
}

fn validate_proxy_tool_call(
    message: &Value,
    exposed_tools: &Mutex<Vec<String>>,
) -> Result<super::tool_broker::ToolRisk, String> {
    let params = message
        .get("params")
        .ok_or_else(|| "MCP tool call did not include params.".to_string())?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "MCP tool call did not include a tool name.".to_string())?;
    let arguments = params
        .get("arguments")
        .ok_or_else(|| "MCP tool call did not include arguments.".to_string())?;
    let tools = exposed_tools.lock().map_err(|error| error.to_string())?;
    ToolBroker::validate_mcp_call(name, &tools, arguments)
}

fn validate_proxy_request(
    message: &Value,
    exposed_tools: &Mutex<Vec<String>>,
    authority: &ProxyAssignmentAuthority,
    connector_id: &str,
    connector_binding: &str,
) -> Result<super::tool_broker::ToolRisk, String> {
    let risk = validate_proxy_tool_call(message, exposed_tools)?;
    validate_proxy_assignment_authority(authority, connector_id, connector_binding)?;
    validate_proxy_repair_scope(message, authority)?;
    if let ProxyAssignmentAuthority::Fenced(authority) = authority {
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
                    "Subtask authority does not match its parent connector authority.".to_string(),
                );
            }
            SchedulerStore::admit_subtask_tool_call(
                subtask,
                &authority.capability,
                if risk == super::tool_broker::ToolRisk::Read {
                    crate::infrastructure::scheduler_store::SubtaskToolRisk::Read
                } else {
                    crate::infrastructure::scheduler_store::SubtaskToolRisk::Mutation
                },
            )?;
        }
    }
    Ok(risk)
}

fn validate_proxy_repair_scope(
    message: &Value,
    authority: &ProxyAssignmentAuthority,
) -> Result<(), String> {
    let ProxyAssignmentAuthority::Fenced(authority) = authority else {
        return Ok(());
    };
    if authority.allowed_tools.is_empty() {
        return Ok(());
    }
    let tool_name = message
        .get("params")
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| "MCP tool call did not include a tool name.".to_string())?;
    if authority.allowed_tools.iter().any(|tool| tool == tool_name) {
        Ok(())
    } else {
        Err(format!(
            "MCP capability repair scope does not allow tool '{tool_name}'."
        ))
    }
}

fn validate_proxy_assignment_authority(
    authority: &ProxyAssignmentAuthority,
    connector_id: &str,
    connector_binding: &str,
) -> Result<(), String> {
    let authority = match authority {
        ProxyAssignmentAuthority::Legacy => return Ok(()),
        ProxyAssignmentAuthority::Fenced(authority) => authority,
    };
    if authority.capability != format!("external_tools:{connector_id}")
        || authority.connector_id != connector_id
        || authority.connector_binding_digest != connector_binding
    {
        return Err("MCP proxy authority did not match this connector.".to_string());
    }
    match SchedulerStore::external_authority_is_current(authority)? {
        true => Ok(()),
        false => Err("MCP proxy assignment authority is stale, expired, or ungranted.".to_string()),
    }
}

fn optional_unicode_environment(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!(
            "MCP proxy environment '{name}' was not valid Unicode."
        )),
    }
}

fn required_unicode_environment(name: &str) -> Result<String, String> {
    optional_unicode_environment(name)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("MCP proxy environment '{name}' was required."))
}

fn parse_proxy_assignment_authority(
    mode: Option<&str>,
    authority_json: Option<&str>,
) -> Result<ProxyAssignmentAuthority, String> {
    let mode = mode.unwrap_or(MCP_PROXY_AUTHORITY_MODE_REQUIRED);
    if !matches!(
        mode,
        MCP_PROXY_AUTHORITY_MODE_REQUIRED | MCP_PROXY_AUTHORITY_MODE_LEGACY
    ) {
        return Err(format!("Unknown MCP proxy authority mode '{mode}'."));
    }
    match authority_json {
        Some(value) => serde_json::from_str(value)
            .map(ProxyAssignmentAuthority::Fenced)
            .map_err(|error| format!("Invalid MCP proxy assignment authority: {error}")),
        None if mode == MCP_PROXY_AUTHORITY_MODE_LEGACY => Ok(ProxyAssignmentAuthority::Legacy),
        None => Err("MCP proxy assignment authority was required but not provided.".to_string()),
    }
}

pub(crate) fn mcp_connector_binding_digest(
    connector_id: &str,
    command: &[String],
    environment: &HashMap<String, String>,
) -> Result<String, String> {
    let connector_id = connector_id.trim();
    if connector_id.is_empty() {
        return Err("MCP connector ID is required for proxy binding.".to_string());
    }
    let canonical_environment = environment.iter().collect::<BTreeMap<_, _>>();
    let encoded = serde_json::to_vec(&(connector_id, command, canonical_environment))
        .map_err(|error| format!("Failed to encode MCP connector binding: {error}"))?;
    Ok(Sha256::digest(encoded)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn validate_connector_binding_value(value: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("MCP proxy connector binding was invalid.".to_string())
    }
}

fn write_proxy_error<W: Write>(
    stdout: &Mutex<W>,
    id: Option<&Value>,
    message: &str,
) -> Result<(), String> {
    let response = json!({
        "jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(Value::Null),
        "error": { "code": -32001, "message": message },
    });
    let mut stdout = stdout.lock().map_err(|error| error.to_string())?;
    write_proxy_message(&mut *stdout, &response)
}

fn validated_resource_mutation_request_id(message: &Value) -> Result<Value, String> {
    match message.get("id") {
        Some(value @ (Value::String(_) | Value::Number(_))) => Ok(value.clone()),
        _ => Err("Resource mutation MCP calls require a string or numeric request ID.".to_string()),
    }
}

fn uncertain_resolution(kind: &str, code: &str) -> ResourceMutationResolution {
    ResourceMutationResolution::Uncertain {
        evidence: None,
        kind: kind.to_string(),
        code: code.to_string(),
    }
}

fn resolve_unanswered_resource_mutations(
    pending: &Mutex<HashMap<String, PendingResourceMutation>>,
    code: &str,
) -> Result<UnansweredMutationResolution, String> {
    let unresolved = pending
        .lock()
        .map_err(|error| error.to_string())?
        .drain()
        .map(|(_, pending)| pending)
        .collect::<Vec<_>>();
    let mut resolved = Vec::with_capacity(unresolved.len());
    let mut first_error = None;
    let mut failed = 0usize;
    for pending in unresolved {
        match SchedulerStore::resolve_external_resource_mutation(
            &pending.authority,
            pending.record.mutation_id,
            uncertain_resolution("transport", code),
        ) {
            Ok(record) => resolved.push((pending.request_id, record)),
            Err(error) => {
                failed = failed.saturating_add(1);
                first_error.get_or_insert(error);
            }
        }
    }
    Ok(UnansweredMutationResolution {
        resolved,
        error: first_error.map(|error| {
            format!(
                "Failed to durably mark {failed} unanswered resource mutation(s) uncertain. First failure: {error}"
            )
        }),
    })
}

fn write_uncertain_resource_mutation_results<W: Write + Send + 'static>(
    stdout: Arc<Mutex<W>>,
    unresolved: Vec<(Value, ResourceMutationRecord)>,
    message: &'static str,
) -> Result<(), String> {
    if unresolved.is_empty() {
        return Ok(());
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = unresolved.into_iter().try_for_each(|(request_id, record)| {
            write_resource_mutation_result(
                &stdout,
                &request_id,
                "resource_mutation_uncertain",
                &record,
                message,
            )
        });
        let _ = sender.send(result);
    });
    match receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("MCP proxy shutdown notification thread terminated unexpectedly.".to_string())
        }
    }
}

fn classify_resource_mutation_response(
    message: &Value,
    expected: &ResourceMutationRecord,
) -> ResourceMutationResolution {
    if expected.identity.connector == "jira"
        && expected.identity.operation == "add_comment"
        && super::jira::trusted_jira_comment_tool(&expected.tool_name)
        && message.get("error").is_none()
    {
        let jira_payload = mcp_text_json_payload(message).unwrap_or_else(|| message.clone());
        if let Some(comment_id) = super::jira::extract_created_jira_comment_id(&jira_payload) {
            let evidence = ResourceMutationEvidence {
                identity: expected.identity.clone(),
                lookup: ResourceLookupResult {
                    status: ResourceLookupStatus::DriftDetected,
                    observed_fingerprint: None,
                    observed_version: None,
                },
                execution: ResourceExecutionResult {
                    status: ResourceExecutionStatus::Executed,
                    resulting_fingerprint: Some(format!(
                        "sha256:{}",
                        expected
                            .identity
                            .mutation_fingerprint
                            .trim_start_matches("sha256:")
                    )),
                    resulting_version: Some(comment_id),
                },
                retry_resume_status: ResourceRetryResumeStatus::FirstExecution,
            };
            return ResourceMutationResolution::Succeeded(evidence);
        }
    }
    if let Some(payload) = mcp_text_json_payload(message) {
        if payload.get("status").and_then(Value::as_str) == Some("approval_required") {
            let identity = payload
                .get("operation_identity")
                .cloned()
                .and_then(|value| serde_json::from_value::<ResourceOperationIdentity>(value).ok());
            return if identity.as_ref() == Some(&expected.identity) {
                ResourceMutationResolution::Failed {
                    evidence: None,
                    kind: "approval".to_string(),
                    code: "approval_required".to_string(),
                }
            } else {
                uncertain_resolution("protocol", "approval_identity_mismatch")
            };
        }
        if let Some(evidence) = payload
            .get("resource_mutation")
            .cloned()
            .and_then(|value| serde_json::from_value::<ResourceMutationEvidence>(value).ok())
        {
            return classify_resource_mutation_evidence(evidence, expected, false, false);
        }
    }
    if let Some(evidence) = message
        .pointer("/error/data/resource_mutation")
        .cloned()
        .and_then(|value| serde_json::from_value::<ResourceMutationEvidence>(value).ok())
    {
        let definitive_error = message
            .pointer("/error/data/kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| {
                matches!(
                    kind,
                    "auth"
                        | "config"
                        | "conflict"
                        | "discovery"
                        | "forbidden"
                        | "invalid_manifest"
                        | "not_found"
                )
            });
        return classify_resource_mutation_evidence(evidence, expected, true, definitive_error);
    }
    uncertain_resolution("protocol", "missing_resource_mutation_evidence")
}

fn enrich_resource_mutation_response(message: &mut Value, evidence: &ResourceMutationEvidence) {
    let Some(result) = message.get_mut("result").and_then(Value::as_object_mut) else {
        return;
    };
    if let Some(content) = result.get_mut("content").and_then(Value::as_array_mut) {
        if content.len() == 1 {
            if let Some(text) = content[0].get_mut("text") {
                if let Some(mut payload) = text
                    .as_str()
                    .and_then(|value| serde_json::from_str::<Value>(value).ok())
                {
                    if let Some(payload) = payload.as_object_mut() {
                        payload.insert(
                            "resource_mutation".to_string(),
                            serde_json::to_value(evidence).unwrap_or(Value::Null),
                        );
                        *text = Value::String(Value::Object(payload.clone()).to_string());
                    }
                }
            }
        }
    }
    result.insert(
        "resource_mutation".to_string(),
        serde_json::to_value(evidence).unwrap_or(Value::Null),
    );
}

fn write_succeeded_jira_comment_replay<W: Write>(
    stdout: &Mutex<W>,
    request_id: &Value,
    record: &ResourceMutationRecord,
) -> Result<bool, String> {
    if record.checkpoint_objective_id.is_some() {
        return Ok(false);
    }
    let Some(mut evidence) = record.evidence.clone() else {
        return Ok(false);
    };
    let Some(comment_id) = evidence.execution.resulting_version.clone() else {
        return Ok(false);
    };
    if !super::jira::canonical_jira_comment_id(&comment_id) {
        return Ok(false);
    }
    evidence.lookup.status = ResourceLookupStatus::AlreadySatisfied;
    evidence.execution.status = ResourceExecutionStatus::Skipped;
    evidence.retry_resume_status = ResourceRetryResumeStatus::AlreadyComplete;
    let payload = json!({
        "commentId": comment_id,
        "resource_mutation": evidence,
        "idempotency_status": "already_complete"
    });
    let response = json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {
            "content": [{ "type": "text", "text": payload.to_string() }],
            "structuredContent": payload,
            "resource_mutation": evidence
        }
    });
    let mut stdout = stdout.lock().map_err(|error| error.to_string())?;
    write_proxy_message(&mut *stdout, &response)?;
    Ok(true)
}

fn classify_resource_mutation_evidence(
    evidence: ResourceMutationEvidence,
    expected: &ResourceMutationRecord,
    response_is_error: bool,
    definitive_error: bool,
) -> ResourceMutationResolution {
    if evidence.validate().is_err() || evidence.identity != expected.identity {
        return uncertain_resolution("protocol", "resource_mutation_identity_mismatch");
    }
    if !response_is_error
        && matches!(
            evidence.execution.status,
            ResourceExecutionStatus::Executed | ResourceExecutionStatus::Skipped
        )
    {
        return ResourceMutationResolution::Succeeded(evidence);
    }
    if definitive_error
        || evidence.execution.status == ResourceExecutionStatus::Conflict
        || matches!(
            evidence.lookup.status,
            ResourceLookupStatus::Incompatible | ResourceLookupStatus::Unavailable
        )
    {
        return ResourceMutationResolution::Failed {
            evidence: Some(evidence),
            kind: "connector".to_string(),
            code: "definitive_rejection".to_string(),
        };
    }
    ResourceMutationResolution::Uncertain {
        evidence: Some(evidence),
        kind: "connector".to_string(),
        code: "ambiguous_mutation_outcome".to_string(),
    }
}

fn mcp_text_json_payload(message: &Value) -> Option<Value> {
    let content = message.pointer("/result/content")?.as_array()?;
    if content.len() != 1 || content[0].get("type").and_then(Value::as_str) != Some("text") {
        return None;
    }
    serde_json::from_str(content[0].get("text")?.as_str()?).ok()
}

fn write_resource_mutation_result<W: Write>(
    stdout: &Mutex<W>,
    id: &Value,
    status: &str,
    record: &ResourceMutationRecord,
    message: &str,
) -> Result<(), String> {
    let payload = json!({
        "status": status,
        "mutation_id": record.mutation_id,
        "operation_key": record.operation_key,
        "ledger_state": if status == "resource_mutation_uncertain" {
            json!("uncertain")
        } else {
            serde_json::to_value(record.state).unwrap_or_else(|_| json!("unknown"))
        },
        "message": message
    });
    let response = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": payload.to_string() }]
        }
    });
    let mut stdout = stdout.lock().map_err(|error| error.to_string())?;
    write_proxy_message(&mut *stdout, &response)
}

pub(crate) fn write_proxy_message(writer: &mut impl Write, message: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, message)
        .map_err(|error| format!("Failed to serialize proxied MCP message: {error}"))?;
    writer
        .write_all(b"\n")
        .and_then(|_| writer.flush())
        .map_err(|error| format!("Failed to write proxied MCP message: {error}"))
}

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
    pub updated_at: Option<String>,
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
    pub tool_metadata: Vec<McpToolMetadata>,
}

/// Result of validating a generic MCP connector.
#[derive(Clone, Debug, Serialize)]
pub struct McpConnectionStatus {
    pub tool_count: usize,
    pub tools: Vec<String>,
    pub tool_metadata: Vec<McpToolMetadata>,
}

#[derive(Clone, Debug, Serialize)]
pub struct McpToolMetadata {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Option<Value>,
}

/// Configuration for a Jira MCP stdio connector.
#[derive(Clone, Debug, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub scope_id: Option<String>,
    #[serde(default)]
    pub secret_id: Option<String>,
}

/// Result of one bounded read-only connector call, including secret-free recovery evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpReadToolResult {
    pub value: Value,
    pub connector_attempts: u8,
    pub session_recovered: bool,
}

/// Validates any stdio MCP server by initializing it and listing tools.
pub fn test_mcp_connection(server: McpServerConfig) -> Result<McpConnectionStatus, String> {
    with_mcp_client(&server, |client| {
        let tool_metadata = client.tool_metadata()?;
        if tool_metadata.is_empty() {
            return Err("MCP server initialized but exposed no tools.".to_string());
        }
        let tools: Vec<String> = tool_metadata.iter().map(|m| m.name.clone()).collect();
        Ok(McpConnectionStatus {
            tool_count: tools.len(),
            tools,
            tool_metadata,
        })
    })
}

/// Discovers a bounded, secret-free operation inventory for one resolved Agent connector.
pub fn discover_mcp_capability_snapshot(
    connector_id: &str,
    command: &[String],
    environment: &HashMap<String, String>,
) -> ConnectorCapabilitySnapshot {
    let Some((executable, args)) = command.split_first() else {
        return unavailable_capability_snapshot(connector_id, "MCP connector command is empty.");
    };
    let server = McpServerConfig {
        command: executable.clone(),
        args: args.to_vec(),
        env: environment.clone(),
        scope_id: Some(format!("agent-capability:{connector_id}")),
        secret_id: Some(connector_id.to_string()),
    };
    let discovery = test_mcp_connection(server.clone());
    // The OpenCode worker creates its own fenced proxy process. Do not retain a second direct
    // connector process after read-only preflight discovery.
    let _ = close_mcp_session(server);
    match discovery {
        Ok(status) => {
            let discovered_count = status.tool_metadata.len();
            let mut warnings = Vec::new();
            let mut tools = status
                .tool_metadata
                .into_iter()
                .filter_map(|tool| {
                    let name = tool.name.trim();
                    if !canonical_capability_name(name) {
                        return None;
                    }
                    let mut argument_names = tool
                        .input_schema
                        .as_ref()
                        .and_then(|schema| schema.get("properties"))
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|properties| properties.keys())
                        .filter(|name| canonical_capability_name(name))
                        .take(MCP_CAPABILITY_ARGUMENT_LIMIT)
                        .cloned()
                        .collect::<Vec<_>>();
                    argument_names.sort();
                    Some(DiscoveredToolCapability {
                        name: name.to_string(),
                        risk: ToolBroker::risk_for_tool(name).as_str().to_string(),
                        argument_names,
                    })
                })
                .take(MCP_CAPABILITY_TOOL_LIMIT)
                .collect::<Vec<_>>();
            tools.sort_by(|left, right| left.name.cmp(&right.name));
            if tools.len() < discovered_count {
                warnings.push(format!(
                    "Connector '{connector_id}' exposed tools that were invalid or exceeded the bounded inventory limit."
                ));
            }
            if tools.is_empty() {
                return unavailable_capability_snapshot(
                    connector_id,
                    "MCP connector exposed no canonical tools.",
                );
            }
            ConnectorCapabilitySnapshot {
                connector_id: connector_id.to_string(),
                status: ConnectorDiscoveryStatus::Available,
                tools,
                error: None,
                warnings,
            }
        }
        Err(error) => {
            unavailable_capability_snapshot(connector_id, capability_discovery_error(&error))
        }
    }
}

/// Calls one exact read-only tool on a task-selected MCP connector.
///
/// The caller remains responsible for task authority and semantic response validation. This
/// boundary rejects mutation-classified tools and redacts connector environment values from
/// transport diagnostics before returning them.
pub fn call_mcp_read_tool(
    connector_id: &str,
    command: &[String],
    environment: &HashMap<String, String>,
    tool_name: &str,
    arguments: Value,
    request_timeout: Duration,
) -> Result<Value, String> {
    call_mcp_read_tool_bounded(
        connector_id,
        command,
        environment,
        tool_name,
        arguments,
        request_timeout,
        false,
    )
    .map(|result| result.value)
}

/// Calls one exact read-only tool and recreates its connector process at most once after a
/// transport-invalidating failure. Both attempts share the caller's original deadline.
pub fn call_mcp_read_tool_with_recovery(
    connector_id: &str,
    command: &[String],
    environment: &HashMap<String, String>,
    tool_name: &str,
    arguments: Value,
    request_timeout: Duration,
) -> Result<McpReadToolResult, String> {
    call_mcp_read_tool_bounded(
        connector_id,
        command,
        environment,
        tool_name,
        arguments,
        request_timeout,
        true,
    )
}

fn call_mcp_read_tool_bounded(
    connector_id: &str,
    command: &[String],
    environment: &HashMap<String, String>,
    tool_name: &str,
    arguments: Value,
    request_timeout: Duration,
    recover_transport_once: bool,
) -> Result<McpReadToolResult, String> {
    if !canonical_capability_name(connector_id)
        || !canonical_capability_name(tool_name)
        || ToolBroker::risk_for_tool(tool_name)
            != crate::infrastructure::tool_broker::ToolRisk::Read
        || !arguments.is_object()
    {
        return Err("MCP evidence read request is invalid or not read-only.".to_string());
    }
    let Some((executable, args)) = command.split_first() else {
        return Err("MCP evidence connector command is empty.".to_string());
    };
    let server = McpServerConfig {
        command: executable.clone(),
        args: args.to_vec(),
        env: environment.clone(),
        scope_id: Some(format!("agent-evidence:{connector_id}")),
        secret_id: Some(connector_id.to_string()),
    };
    let request_timeout = request_timeout
        .min(Duration::from_secs(45))
        .max(Duration::from_millis(1));
    let deadline = Instant::now() + request_timeout;
    let mut connector_attempts = 0_u8;
    loop {
        connector_attempts = connector_attempts.saturating_add(1);
        let result = with_mcp_client_until(&server, Some(deadline), |client| {
            client.call_tool_with_timeout(
                tool_name,
                arguments.clone(),
                remaining_mcp_timeout(deadline),
            )
        });
        let _ = close_mcp_session(server.clone());
        match result {
            Ok(value) => {
                return Ok(McpReadToolResult {
                    value,
                    connector_attempts,
                    session_recovered: connector_attempts > 1,
                });
            }
            Err(error)
                if recover_transport_once
                    && connector_attempts == 1
                    && Instant::now() < deadline
                    && mcp_error_invalidates_session(&error) =>
            {
                crate::infrastructure::performance::increment(
                    "mcp_read_session_recoveries_total",
                    "mcp",
                    1,
                );
            }
            Err(error) => return Err(error),
        }
    }
}

fn unavailable_capability_snapshot(
    connector_id: &str,
    error: impl Into<String>,
) -> ConnectorCapabilitySnapshot {
    ConnectorCapabilitySnapshot {
        connector_id: connector_id.to_string(),
        status: ConnectorDiscoveryStatus::Unavailable,
        tools: Vec::new(),
        error: Some(error.into()),
        warnings: Vec::new(),
    }
}

fn capability_discovery_error(error: &str) -> &'static str {
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("timed out") {
        "Timed out while discovering MCP tools."
    } else if normalized.contains("not found") {
        "MCP connector command was not found."
    } else if normalized.contains("exposed no tools") {
        "MCP connector exposed no tools."
    } else {
        "MCP tools/list discovery failed."
    }
}

fn canonical_capability_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.contains("..")
        && !value.starts_with('/')
        && !value.ends_with('/')
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/' | b':')
        })
}

/// Request sent by the UI when syncing Jira through an MCP server.
#[derive(Clone, Debug, Deserialize)]
pub struct JiraMcpConfig {
    pub server: McpServerConfig,
    pub auth: JiraAuthConfig,
    #[serde(default)]
    pub secret_id: String,
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

/// Jira connection metadata; credential fields are populated by the backend.
#[derive(Clone, Debug, Deserialize)]
pub struct JiraAuthConfig {
    pub base_url: String,
    pub auth_mode: String,
    #[serde(default)]
    pub username: String,
    #[serde(skip)]
    pub api_token: String,
    #[serde(skip)]
    pub personal_access_token: String,
    #[serde(skip)]
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
        let tool_metadata = client.tool_metadata()?;
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
            tool_metadata,
        })
    })
}

/// Fetches Jira agile boards through the configured MCP server.
pub fn fetch_jira_boards(config: JiraMcpConfig) -> Result<Vec<JiraBoard>, String> {
    let mcp_result = with_mcp_client(&config.server, |client| {
        let tools = client.tools()?;
        let board_tool = resolve_tool(&tools, &config.board_tool_name)?;
        fetch_boards_with_fallbacks(client, &board_tool, &config)
    });

    mcp_result.or_else(|_| {
        jira_rest::fetch_boards(
            &config.auth,
            config.project_key.as_deref(),
            config.board_name.as_deref(),
        )
    })
}

/// Fetches Jira issues through the configured MCP server.
pub fn fetch_jira_issues(config: JiraMcpConfig) -> Result<Vec<JiraIssue>, String> {
    let board_id = config
        .board_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string);

    if let Some(board_id) = board_id {
        let mcp_result = with_mcp_client(&config.server, |client| {
            let tools = client.tools()?;
            let tool_name = resolve_tool(&tools, &config.board_issues_tool_name)?;
            let result = client.call_tool(
                &tool_name,
                json!({
                    "board_id": board_id,
                    "jql": config.jql,
                    "fields": "key,summary,description,status,issuetype,labels,updated",
                    "start_at": 0,
                    "limit": config.page_size
                }),
            )?;
            parse_jira_issues(&result)
        });

        return mcp_result.or_else(|_| {
            jira_rest::fetch_board_issues_paginated(
                &config.auth,
                &board_id,
                &config.jql,
                config.page_size,
                config.max_pages,
            )
        });
    }

    with_mcp_client(&config.server, |client| {
        let tools = client.tools()?;
        let tool_name = resolve_tool(&tools, &config.tool_name)?;
        let arguments = json!({
            "jql": config.jql,
            "fields": "key,summary,description,status,issuetype,labels,updated",
            "limit": config.page_size
        });
        let result = client.call_tool(&tool_name, arguments)?;
        parse_jira_issues(&result)
    })
}

struct StdioMcpClient {
    child: Mutex<Child>,
    stdin: Mutex<std::process::ChildStdin>,
    pending: Arc<Mutex<HashMap<u64, mpsc::SyncSender<Value>>>>,
    stderr: Arc<Mutex<String>>,
    reader_error: Arc<Mutex<Option<String>>>,
    next_id: AtomicU64,
    tool_metadata: Mutex<Option<Vec<McpToolMetadata>>>,
}

impl StdioMcpClient {
    fn start(config: &McpServerConfig) -> Result<Self, String> {
        let spawn_metric = crate::infrastructure::performance::span("mcp_process_spawn_ms", "mcp")
            .with_context(
                "mcp_connection_id",
                config.scope_id.as_deref().unwrap_or("unscoped"),
            );
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
                    format!("Failed to start MCP server: {error}")
                }
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to open MCP stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to open MCP stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Failed to open MCP stderr".to_string())?;
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let stderr_buffer = Arc::new(Mutex::new(String::new()));
        let reader_error = Arc::new(Mutex::new(None));

        spawn_stdout_reader(stdout, Arc::clone(&pending), Arc::clone(&reader_error));
        spawn_stderr_reader(stderr, Arc::clone(&stderr_buffer));

        let client = Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending,
            stderr: stderr_buffer,
            reader_error,
            next_id: AtomicU64::new(request_seed()),
            tool_metadata: Mutex::new(None),
        };
        spawn_metric.finish();
        Ok(client)
    }

    fn initialize(&self) -> Result<(), String> {
        self.initialize_with_timeout(Duration::from_secs(45))
    }

    fn initialize_with_timeout(&self, timeout: Duration) -> Result<(), String> {
        let metric =
            crate::infrastructure::performance::span("mcp_transport_initialization_ms", "mcp");
        self.request_with_timeout(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "spacesly",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
            timeout,
        )?;
        self.notify("notifications/initialized", json!({}))?;
        metric.finish();
        Ok(())
    }

    fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, String> {
        self.call_tool_with_timeout(name, arguments, Duration::from_secs(45))
    }

    fn call_tool_with_timeout(
        &self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        ToolBroker::validate_mcp_call(name, &self.tools_with_timeout(timeout)?, &arguments)?;
        self.request_with_timeout(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments
            }),
            timeout,
        )
    }

    fn list_tool_metadata_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Vec<McpToolMetadata>, String> {
        let discovery = crate::infrastructure::performance::span("mcp_schema_discovery_ms", "mcp");
        let result = self.request_with_timeout("tools/list", json!({}), timeout)?;
        if crate::infrastructure::performance::mode()
            == crate::infrastructure::performance::PerformanceMode::Profiling
        {
            let serialization =
                crate::infrastructure::performance::span("mcp_schema_serialization_ms", "mcp");
            let schema_bytes = serde_json::to_vec(&result)
                .map(|value| value.len())
                .unwrap_or(0);
            serialization.finish();
            crate::infrastructure::performance::increment(
                "mcp_schema_bytes_total",
                "mcp",
                schema_bytes as u64,
            );
        }
        let parsing = crate::infrastructure::performance::span("mcp_schema_parsing_ms", "mcp");
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| "MCP server did not return a tools list".to_string())?;

        let metadata = tools
            .iter()
            .filter_map(|tool| {
                Some(McpToolMetadata {
                    name: tool.get("name")?.as_str()?.to_string(),
                    description: tool
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    input_schema: tool.get("inputSchema").cloned(),
                })
            })
            .collect::<Vec<_>>();
        crate::infrastructure::performance::increment(
            "mcp_tools_discovered_total",
            "mcp",
            metadata.len() as u64,
        );
        parsing.finish();
        discovery.finish();
        Ok(metadata)
    }

    fn tools(&self) -> Result<Vec<String>, String> {
        self.tools_with_timeout(Duration::from_secs(45))
    }

    fn tools_with_timeout(&self, timeout: Duration) -> Result<Vec<String>, String> {
        if let Some(metadata) = self
            .tool_metadata
            .lock()
            .map_err(|error| error.to_string())?
            .clone()
        {
            crate::infrastructure::performance::increment("mcp_schema_cache_hits_total", "mcp", 1);
            return Ok(metadata.into_iter().map(|tool| tool.name).collect());
        }
        crate::infrastructure::performance::increment("mcp_schema_cache_misses_total", "mcp", 1);
        let metadata = self.list_tool_metadata_with_timeout(timeout)?;
        let tools: Vec<String> = metadata.iter().map(|tool| tool.name.clone()).collect();
        *self
            .tool_metadata
            .lock()
            .map_err(|error| error.to_string())? = Some(metadata);
        Ok(tools)
    }

    fn tool_metadata(&self) -> Result<Vec<McpToolMetadata>, String> {
        if let Some(metadata) = self
            .tool_metadata
            .lock()
            .map_err(|error| error.to_string())?
            .clone()
        {
            crate::infrastructure::performance::increment("mcp_schema_cache_hits_total", "mcp", 1);
            return Ok(metadata);
        }
        self.tools()?;
        Ok(self
            .tool_metadata
            .lock()
            .map_err(|error| error.to_string())?
            .clone()
            .unwrap_or_default())
    }

    fn is_alive(&self) -> bool {
        matches!(
            self.child
                .lock()
                .ok()
                .and_then(|mut child| child.try_wait().ok()),
            Some(None)
        )
    }

    fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        let (sender, receiver) = mpsc::sync_channel(1);
        self.pending
            .lock()
            .map_err(|error| error.to_string())?
            .insert(id, sender);
        if let Err(error) = self.write_message(&message) {
            let _ = self.pending.lock().map(|mut pending| pending.remove(&id));
            return Err(error);
        }

        let response = receiver.recv_timeout(timeout).map_err(|error| {
            let _ = self.pending.lock().map(|mut pending| pending.remove(&id));
            let stderr = self
                .stderr
                .lock()
                .map(|buffer| buffer.trim().to_string())
                .unwrap_or_default();
            let reader_error = self
                .reader_error
                .lock()
                .ok()
                .and_then(|error| error.clone());
            if let Some(reader_error) = reader_error {
                reader_error
            } else if stderr.is_empty() {
                format!("Timed out waiting for MCP response after {}s. The MCP server started but did not answer this request. Verify the selected tool arguments. Internal timeout: {error}", timeout.as_secs())
            } else {
                format!("Timed out waiting for MCP response after {}s. MCP stderr: {stderr}", timeout.as_secs())
            }
        })?;

        if let Some(error) = response.get("error") {
            return Err(format!("MCP request failed: {error}"));
        }

        response
            .get("result")
            .cloned()
            .ok_or_else(|| "MCP response did not include a result".to_string())
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
    }

    fn write_message(&self, message: &Value) -> Result<(), String> {
        let body = serde_json::to_vec(message)
            .map_err(|error| format!("Failed to serialize MCP message: {error}"))?;
        let mut stdin = self.stdin.lock().map_err(|error| error.to_string())?;
        stdin
            .write_all(&body)
            .map_err(|error| format!("Failed to write MCP body: {error}"))?;
        stdin
            .write_all(b"\n")
            .map_err(|error| format!("Failed to write MCP newline: {error}"))?;
        stdin
            .flush()
            .map_err(|error| format!("Failed to flush MCP message: {error}"))
    }
}

impl Drop for StdioMcpClient {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct McpSessionEntry {
    client: Arc<StdioMcpClient>,
    last_used: Instant,
}

#[derive(Default)]
struct McpSessionManager {
    sessions: Mutex<HashMap<String, McpSessionEntry>>,
    initializations: Mutex<HashMap<String, Arc<Mutex<()>>>>,
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
    // Clear initialization locks alongside sessions so any in-progress init threads
    // do not block future callers after the app resets its connection state.
    let _ = manager
        .initializations
        .lock()
        .map(|mut inits| inits.clear());
}

pub fn close_mcp_session(server: McpServerConfig) -> Result<bool, String> {
    let manager = mcp_session_manager();
    let key = mcp_server_key(&server);
    let session = manager
        .sessions
        .lock()
        .map_err(|error| error.to_string())?
        .remove(&key)
        .map(|entry| entry.client);
    // Also remove the initialization lock for this key.  If a previous spawn_blocking
    // task timed out while holding the initialization guard, the Mutex remains poisoned
    // (or just locked) and future callers block forever waiting for it.  Removing the
    // entry here lets the next call create a fresh initialization lock.
    let _ = manager
        .initializations
        .lock()
        .map(|mut inits| inits.remove(&key));
    let existed = session.is_some();
    drop(session);
    Ok(existed)
}

fn with_mcp_client<T, F>(server: &McpServerConfig, operation: F) -> Result<T, String>
where
    F: FnOnce(&StdioMcpClient) -> Result<T, String>,
{
    with_mcp_client_until(server, None, operation)
}

fn with_mcp_client_until<T, F>(
    server: &McpServerConfig,
    deadline: Option<Instant>,
    operation: F,
) -> Result<T, String>
where
    F: FnOnce(&StdioMcpClient) -> Result<T, String>,
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
        crate::infrastructure::performance::increment("mcp_session_cache_hits_total", "mcp", 1);
        crate::infrastructure::performance::record_duration(
            "mcp_init_warm_ms",
            "mcp",
            now.elapsed(),
        );
        session
    } else {
        crate::infrastructure::performance::increment("mcp_session_cache_misses_total", "mcp", 1);
        let cold_initialization =
            crate::infrastructure::performance::span("mcp_init_cold_ms", "mcp").with_context(
                "mcp_connection_id",
                server.scope_id.as_deref().unwrap_or("unscoped"),
            );
        let initialization = manager
            .initializations
            .lock()
            .map_err(|error| error.to_string())?
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let initialized = {
            let _initialization_guard = initialization.lock().map_err(|error| error.to_string())?;
            // Remove the map entry while holding the guard so that:
            //   1. Any concurrent waiter queued behind us will see None after we release
            //      the lock and will proceed normally (find the session or start a new init).
            //   2. Stale entries from timed-out init attempts do not accumulate, and
            //      close_mcp_session can install a fresh entry on the next attempt without
            //      racing against an in-progress but abandoned init thread.
            let _ = manager
                .initializations
                .lock()
                .map(|mut inits| inits.remove(&key));
            let existing = {
                let sessions = manager.sessions.lock().map_err(|error| error.to_string())?;
                sessions.get(&key).map(|entry| Arc::clone(&entry.client))
            };
            if let Some(existing) = existing {
                existing
            } else {
                let client = StdioMcpClient::start(server)?;
                if let Some(deadline) = deadline {
                    client.initialize_with_timeout(remaining_mcp_timeout(deadline))?;
                    client.tools_with_timeout(remaining_mcp_timeout(deadline))?;
                } else {
                    client.initialize()?;
                    client.tools()?;
                }
                let candidate = Arc::new(client);
                let evicted = {
                    let mut sessions =
                        manager.sessions.lock().map_err(|error| error.to_string())?;
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
                    evicted
                };
                drop(evicted);
                candidate
            }
        };
        cold_initialization.finish();
        initialized
    };

    let result = operation(&session).map_err(|error| redact_mcp_diagnostic(&error, &server.env));
    let alive = session.is_alive();

    let mut sessions = manager.sessions.lock().map_err(|error| error.to_string())?;
    if let Some(entry) = sessions.get_mut(&key) {
        if Arc::ptr_eq(&entry.client, &session) {
            if alive
                && result
                    .as_ref()
                    .err()
                    .is_none_or(|error| !mcp_error_invalidates_session(error))
            {
                entry.last_used = Instant::now();
            } else {
                sessions.remove(&key);
            }
        }
    }
    result
}

fn remaining_mcp_timeout(deadline: Instant) -> Duration {
    deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(1))
}

fn mcp_error_invalidates_session(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("timed out waiting for mcp response")
        || error.contains("mcp protocol reader failed")
        || error.contains("mcp stdout closed")
        || error.contains("failed to write mcp")
        || error.contains("failed to flush mcp")
}

fn reap_idle_sessions(
    sessions: &mut HashMap<String, McpSessionEntry>,
    now: Instant,
) -> Vec<Arc<StdioMcpClient>> {
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
) -> Vec<Arc<StdioMcpClient>> {
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
        "{}\0{}\0{}\0{}",
        server.scope_id.as_deref().unwrap_or("workspace-personal"),
        server.command,
        server.args.join("\0"),
        env.into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("\0")
    )
}

fn redact_mcp_diagnostic(error: &str, env: &HashMap<String, String>) -> String {
    let redacted = env
        .values()
        .filter(|value| value.len() >= 4)
        .fold(error.to_string(), |message, value| {
            message.replace(value, "[REDACTED]")
        });
    redact_global_environment_values(&redacted)
}

fn spawn_stdout_reader(
    stdout: std::process::ChildStdout,
    pending: Arc<Mutex<HashMap<u64, mpsc::SyncSender<Value>>>>,
    reader_error: Arc<Mutex<Option<String>>>,
) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_stdout_message(&mut reader) {
                Ok(Some(value)) => {
                    dispatch_mcp_response(&pending, value);
                }
                Ok(None) => {
                    fail_pending_requests(
                        &pending,
                        &reader_error,
                        "MCP stdout closed before the pending request completed.".to_string(),
                    );
                    break;
                }
                Err(error) => {
                    fail_pending_requests(
                        &pending,
                        &reader_error,
                        format!("MCP protocol reader failed: {error}"),
                    );
                    break;
                }
            }
        }
    });
}

fn fail_pending_requests(
    pending: &Mutex<HashMap<u64, mpsc::SyncSender<Value>>>,
    reader_error: &Mutex<Option<String>>,
    message: String,
) {
    if let Ok(mut error) = reader_error.lock() {
        *error = Some(message.clone());
    }
    let senders = pending
        .lock()
        .map(|mut pending| pending.drain().collect::<Vec<_>>())
        .unwrap_or_default();
    for (id, sender) in senders {
        let _ = sender.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32000, "message": message }
        }));
    }
}

fn dispatch_mcp_response(pending: &Mutex<HashMap<u64, mpsc::SyncSender<Value>>>, value: Value) {
    let Some(id) = value.get("id").and_then(Value::as_u64) else {
        return;
    };
    let sender = pending
        .lock()
        .ok()
        .and_then(|mut pending| pending.remove(&id));
    if let Some(sender) = sender {
        let _ = sender.send(value);
    }
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

pub(crate) fn read_stdout_message<R: BufRead>(reader: &mut R) -> Result<Option<Value>, String> {
    let trimmed = loop {
        let Some(line) = read_bounded_line(reader, MCP_MESSAGE_LIMIT)? else {
            return Ok(None);
        };
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if !trimmed.is_empty() {
            break trimmed.to_string();
        }
    };

    let content_length = trimmed
        .split_once(':')
        .and_then(|(name, value)| name.eq_ignore_ascii_case("content-length").then_some(value));
    if let Some(value) = content_length {
        let length = value
            .trim()
            .parse::<usize>()
            .map_err(|error| format!("Invalid MCP content length: {error}"))?;
        while let Some(header) = read_bounded_line(reader, MCP_HEADER_LINE_LIMIT)? {
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

    serde_json::from_str::<Value>(&trimmed)
        .map(Some)
        .map_err(|error| format!("Invalid JSON on MCP stdout: {error}"))
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
    if preferred.is_empty() {
        return Err("MCP tool name is required.".to_string());
    }
    if let Some(tool) = tools.iter().find(|tool| tool.as_str() == preferred) {
        return Ok(tool.clone());
    }
    if let Some(tool) = resolve_unique_tool_suffix(tools, preferred)? {
        return Ok(tool);
    }

    let suffix = preferred.strip_prefix("jira_").unwrap_or(preferred);
    if suffix != preferred {
        if let Some(tool) = resolve_unique_tool_suffix(tools, suffix)? {
            return Ok(tool);
        }
    }

    Err(format!(
        "Could not find MCP tool '{preferred}'. Available tools: {}",
        tools.join(", ")
    ))
}

fn resolve_unique_tool_suffix(tools: &[String], suffix: &str) -> Result<Option<String>, String> {
    let matches: Vec<&String> = tools.iter().filter(|tool| tool.ends_with(suffix)).collect();
    match matches.as_slice() {
        [] => Ok(None),
        [tool] => Ok(Some((*tool).clone())),
        _ => Err(format!(
            "MCP tool name '{suffix}' is ambiguous. Matching tools: {}",
            matches
                .into_iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn fetch_boards_with_fallbacks(
    client: &StdioMcpClient,
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
        updated_at: text_field(fields, "updated"),
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
    use crate::domain::resource_idempotency::{
        ResourceExecutionResult, ResourceIdentity, ResourceLookupResult, ResourceRetryResumeStatus,
    };
    #[cfg(unix)]
    use std::net::Shutdown;
    #[cfg(unix)]
    use std::os::unix::net::UnixStream;

    #[cfg(unix)]
    #[derive(Clone, Default)]
    struct SharedProxyOutput(Arc<Mutex<Vec<u8>>>);

    #[cfg(unix)]
    impl Write for SharedProxyOutput {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn proxy_test_identity() -> ResourceOperationIdentity {
        ResourceOperationIdentity::new(
            "openshift_kubernetes",
            "scale_deployment",
            ResourceIdentity {
                api_version: "apps/v1".to_string(),
                kind: "Deployment".to_string(),
                namespace: Some("default".to_string()),
                name: "api".to_string(),
            },
            "https://cluster.example:6443",
            &json!({ "replicas": 3 }),
        )
        .expect("test identity")
    }

    fn proxy_test_restart_identity() -> ResourceOperationIdentity {
        ResourceOperationIdentity::new(
            "openshift_kubernetes",
            "restart_deployment",
            ResourceIdentity {
                api_version: "apps/v1".to_string(),
                kind: "Deployment".to_string(),
                namespace: Some("default".to_string()),
                name: "api".to_string(),
            },
            "https://cluster.example:6443",
            &json!({
                "restart_token": "11111111-1111-4111-8111-111111111111"
            }),
        )
        .expect("restart identity")
    }

    fn proxy_test_evidence(
        identity: &ResourceOperationIdentity,
        lookup: ResourceLookupStatus,
        execution: ResourceExecutionStatus,
    ) -> ResourceMutationEvidence {
        ResourceMutationEvidence {
            identity: identity.clone(),
            lookup: ResourceLookupResult {
                status: lookup,
                observed_fingerprint: Some(identity.mutation_fingerprint.clone()),
                observed_version: Some("10".to_string()),
            },
            execution: ResourceExecutionResult {
                status: execution,
                resulting_fingerprint: (execution == ResourceExecutionStatus::Executed)
                    .then(|| identity.mutation_fingerprint.clone()),
                resulting_version: (execution == ResourceExecutionStatus::Executed)
                    .then(|| "11".to_string()),
            },
            retry_resume_status: if execution == ResourceExecutionStatus::Conflict {
                ResourceRetryResumeStatus::Conflict
            } else {
                ResourceRetryResumeStatus::ReconciledAfterDrift
            },
        }
    }

    fn proxy_test_record(identity: &ResourceOperationIdentity) -> ResourceMutationRecord {
        ResourceMutationRecord {
            mutation_id: 7,
            operation_key: identity.key.clone(),
            identity: identity.clone(),
            connector_id: "ocp".to_string(),
            tool_name: "ocp_scale_deployment".to_string(),
            state: ResourceMutationState::Reserved,
            session_id: crate::domain::task_session::TaskSessionId(1),
            attempt_id: 2,
            attempt: 1,
            fencing_token: 3,
            evidence: None,
            failure_kind: None,
            failure_code: None,
            revision: 1,
            reserved_at: 1,
            resolved_at: None,
            superseded_at: None,
            supersede_reason: None,
            checkpoint_objective_id: None,
            checkpoint_tool_call_id: None,
            checkpoint_recorded_at: None,
        }
    }

    #[cfg(unix)]
    fn wait_for_proxy_output(output: &SharedProxyOutput, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if String::from_utf8_lossy(&output.0.lock().unwrap()).contains(expected) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("proxy output did not contain '{expected}' before timeout");
    }

    #[cfg(unix)]
    fn run_resource_proxy_failure_case(
        behavior: &str,
    ) -> (Result<(), String>, ResourceMutationRecord, String) {
        let script = r#"
while IFS= read -r line; do
  case "$line" in
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"ocp_scale_deployment","inputSchema":{"type":"object"}}]}}\n'
      if [ "$1" = "blocked_write" ]; then
        sleep 1
        exec 1>&-
        while :; do :; done
      fi
      ;;
    *'"method":"tools/call"'*)
      case "$1" in
        malformed) printf 'not-json\n'; while :; do :; done ;;
        eof) exit 0 ;;
        eof_alive) exec 1>&-; while :; do :; done ;;
        client_error) while :; do :; done ;;
      esac
      ;;
  esac
done
"#;
        let command = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            script.to_string(),
            "proxy-fixture".to_string(),
            behavior.to_string(),
        ];
        let connector_binding =
            mcp_connector_binding_digest("ocp", &command, &HashMap::new()).expect("binding");
        let directory = tempfile::tempdir().expect("temp directory");
        let store = SchedulerStore::open_at(directory.path().join("scheduler.db"))
            .expect("scheduler store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_with_grants(
                &crate::domain::task_session::TaskRequest::new("proxy-failure-harness"),
                &["external_tools:ocp".to_string()],
                "test-approval",
            )
            .expect("task enqueued");
        let assignment = store
            .claim_next(owner, 1, Duration::from_secs(30), 5)
            .expect("task claimed")
            .expect("assignment");
        let authority = store
            .external_authority(
                assignment.fence,
                "external_tools:ocp",
                "ocp",
                &connector_binding,
            )
            .expect("authority created");
        let identity = proxy_test_identity();
        let resolved_identity = identity.clone();
        let (mut client, proxy_input) = UnixStream::pair().expect("proxy input pair");
        let output = SharedProxyOutput::default();
        let captured_output = output.clone();
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = run_mcp_proxy_with_io(
                command,
                ProxyAssignmentAuthority::Fenced(authority),
                "ocp".to_string(),
                connector_binding,
                proxy_input,
                output,
                move |_, tool_name, _| {
                    Ok((tool_name == "ocp_scale_deployment").then(|| resolved_identity.clone()))
                },
            );
            let _ = result_sender.send(result);
        });

        write_proxy_message(
            &mut client,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .expect("tool list requested");
        wait_for_proxy_output(&captured_output, "ocp_scale_deployment");
        let padding = (behavior == "blocked_write").then(|| "x".repeat(200 * 1024));
        write_proxy_message(
            &mut client,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "ocp_scale_deployment",
                    "arguments": {
                        "name": "api",
                        "namespace": "default",
                        "replicas": 3,
                        "padding": padding
                    }
                }
            }),
        )
        .expect("scale requested");
        if behavior == "client_error" {
            client
                .write_all(b"not-json\n")
                .expect("malformed request sent");
            client.flush().expect("malformed request flushed");
        }
        client
            .shutdown(Shutdown::Write)
            .expect("proxy input closed");

        let result = result_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("proxy failure harness timed out");
        let record = store
            .resource_mutations_for_session(session.id)
            .expect("resource mutations read")
            .into_iter()
            .next()
            .expect("resource mutation recorded");
        let output =
            String::from_utf8(captured_output.0.lock().expect("proxy output lock").clone())
                .expect("proxy output is UTF-8");
        (result, record, output)
    }

    #[cfg(unix)]
    #[test]
    fn resource_proxy_malformed_upstream_response_becomes_uncertain() {
        let (result, record, output) = run_resource_proxy_failure_case("malformed");

        assert!(result
            .expect_err("malformed response fails proxy")
            .contains("Invalid JSON on MCP stdout"));
        assert_eq!(record.state, ResourceMutationState::Uncertain);
        assert_eq!(
            record.failure_code.as_deref(),
            Some("upstream_protocol_error")
        );
        assert!(output.contains("resource_mutation_uncertain"));
    }

    #[cfg(unix)]
    #[test]
    fn resource_proxy_upstream_eof_becomes_uncertain() {
        let (result, record, output) = run_resource_proxy_failure_case("eof");

        result.expect("clean connector exit is handled");
        assert_eq!(record.state, ResourceMutationState::Uncertain);
        assert_eq!(record.failure_code.as_deref(), Some("upstream_eof"));
        assert!(output.contains("resource_mutation_uncertain"));
    }

    #[cfg(unix)]
    #[test]
    fn resource_proxy_reaps_connector_that_remains_alive_after_stdout_eof() {
        let (result, record, output) = run_resource_proxy_failure_case("eof_alive");

        assert!(result
            .expect_err("live connector after EOF is terminated")
            .contains("Proxied MCP connector exited"));
        assert_eq!(record.state, ResourceMutationState::Uncertain);
        assert_eq!(record.failure_code.as_deref(), Some("upstream_eof"));
        assert!(output.contains("resource_mutation_uncertain"));
    }

    #[cfg(unix)]
    #[test]
    fn resource_proxy_shutdown_interrupts_blocked_connector_stdin_write() {
        let (result, record, output) = run_resource_proxy_failure_case("blocked_write");

        assert!(result.is_err());
        assert_eq!(record.state, ResourceMutationState::Uncertain);
        assert!(matches!(
            record.failure_code.as_deref(),
            Some("upstream_eof" | "upstream_write_failed")
        ));
        assert!(output.contains("resource_mutation_uncertain"));
    }

    #[cfg(unix)]
    #[test]
    fn resource_proxy_request_reader_termination_becomes_uncertain() {
        let (result, record, output) = run_resource_proxy_failure_case("client_error");

        assert!(result
            .expect_err("malformed client request fails proxy")
            .contains("MCP proxy request reader failed"));
        assert_eq!(record.state, ResourceMutationState::Uncertain);
        assert_eq!(record.failure_code.as_deref(), Some("client_request_error"));
        assert!(output.contains("resource_mutation_uncertain"));
    }

    #[cfg(unix)]
    #[test]
    fn discovers_secret_free_capabilities_for_unknown_connector() {
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"future_search","description":"token-do-not-retain","inputSchema":{"type":"object","properties":{"query":{"type":"string"}}}},{"name":"future_trigger","inputSchema":{"type":"object","properties":{"item_id":{"type":"string"}}}}]}}\n' "$id"
      ;;
  esac
done
"#;
        let connector_id = format!("future-system-{}", request_seed());
        let command = vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()];
        let environment = HashMap::new();
        let snapshot = discover_mcp_capability_snapshot(&connector_id, &command, &environment);

        assert_eq!(snapshot.status, ConnectorDiscoveryStatus::Available);
        assert_eq!(snapshot.tools.len(), 2);
        assert_eq!(snapshot.tools[0].name, "future_search");
        assert_eq!(snapshot.tools[0].risk, "read");
        assert_eq!(snapshot.tools[0].argument_names, vec!["query"]);
        assert_eq!(snapshot.tools[1].risk, "mutation");
        assert!(!serde_json::to_string(&snapshot)
            .expect("snapshot JSON")
            .contains("do-not-retain"));

        close_mcp_session(McpServerConfig {
            command: command[0].clone(),
            args: command[1..].to_vec(),
            env: environment,
            scope_id: Some(format!("agent-capability:{connector_id}")),
            secret_id: Some(connector_id),
        })
        .expect("capability session closes");
    }

    #[cfg(unix)]
    #[test]
    fn calls_one_exact_read_only_mcp_evidence_tool() {
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"bamboo_get_build","inputSchema":{"type":"object","properties":{"result_key":{"type":"string"}}}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"{\"buildResultKey\":\"PAYROLL-DEPLOY-42\",\"buildState\":\"Successful\"}"}]}}\n' "$id"
      ;;
  esac
done
"#;
        let connector_id = format!("bamboo-evidence-{}", request_seed());
        let command = vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()];
        let result = call_mcp_read_tool(
            &connector_id,
            &command,
            &HashMap::new(),
            "bamboo_get_build",
            json!({"result_key": "PAYROLL-DEPLOY-42"}),
            Duration::from_secs(5),
        )
        .expect("read-only evidence call succeeds");
        assert_eq!(
            result.pointer("/content/0/type").and_then(Value::as_str),
            Some("text")
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_only_evidence_recreates_failed_connector_session_once() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let marker = directory.path().join("first-session-failed");
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"confluence_get_page","inputSchema":{"type":"object","properties":{"page_id":{"type":"string"}}}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      if [ ! -f "$RECOVERY_MARKER" ]; then
        : > "$RECOVERY_MARKER"
        exit 0
      fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"{\\"id\\":\\"1997894022\\",\\"title\\":\\"Deployment SOP\\"}"}]}}\n' "$id"
      ;;
  esac
done
"#;
        let connector_id = format!("confluence-recovery-{}", request_seed());
        let command = vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()];
        let environment = HashMap::from([(
            "RECOVERY_MARKER".to_string(),
            marker.to_string_lossy().into_owned(),
        )]);

        let result = call_mcp_read_tool_with_recovery(
            &connector_id,
            &command,
            &environment,
            "confluence_get_page",
            json!({"page_id": "1997894022"}),
            Duration::from_secs(5),
        )
        .expect("read-only connector session recovered");

        assert_eq!(result.connector_attempts, 2);
        assert!(result.session_recovered);
        assert_eq!(
            result
                .value
                .pointer("/content/0/type")
                .and_then(Value::as_str),
            Some("text")
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_only_evidence_does_not_recreate_session_for_provider_error() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let attempts = directory.path().join("connector-attempts");
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"confluence_get_page","inputSchema":{"type":"object","properties":{"page_id":{"type":"string"}}}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf 'x\n' >> "$ATTEMPT_MARKER"
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"invalid request %s"}}\n' "$id" "$CONNECTOR_SECRET"
      ;;
  esac
done
"#;
        let connector_id = format!("confluence-provider-error-{}", request_seed());
        let command = vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()];
        let environment = HashMap::from([
            (
                "ATTEMPT_MARKER".to_string(),
                attempts.to_string_lossy().into_owned(),
            ),
            (
                "CONNECTOR_SECRET".to_string(),
                "connector-secret-must-not-leak".to_string(),
            ),
        ]);

        let error = call_mcp_read_tool_with_recovery(
            &connector_id,
            &command,
            &environment,
            "confluence_get_page",
            json!({"page_id": "1997894022"}),
            Duration::from_secs(5),
        )
        .expect_err("provider validation errors are not transport retries");

        assert_eq!(
            std::fs::read_to_string(attempts).expect("attempt marker read"),
            "x\n"
        );
        assert!(!error.contains("connector-secret-must-not-leak"));
        assert!(error.contains("[REDACTED]"));
    }

    #[cfg(unix)]
    #[test]
    fn mcp_evidence_timeout_bounds_cold_initialization() {
        let script = r#"
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) sleep 2 ;;
  esac
done
"#;
        let connector_id = format!("slow-evidence-{}", request_seed());
        let command = vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()];
        let started = Instant::now();
        let error = call_mcp_read_tool(
            &connector_id,
            &command,
            &HashMap::new(),
            "bamboo_get_build",
            json!({"result_key": "PAYROLL-DEPLOY-42"}),
            Duration::from_millis(100),
        )
        .expect_err("cold initialization must respect the evidence deadline");

        assert!(error.contains("Timed out waiting for MCP response"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn mcp_evidence_boundary_rejects_mutation_tools_before_spawn() {
        let error = call_mcp_read_tool(
            "bamboo",
            &["/does/not/exist".to_string()],
            &HashMap::new(),
            "bamboo_trigger_build",
            json!({"plan_key": "PAYROLL-DEPLOY"}),
            Duration::from_secs(5),
        )
        .expect_err("mutation tool must be rejected before connector spawn");
        assert!(error.contains("not read-only"));
    }

    #[cfg(unix)]
    #[test]
    fn initializes_and_caches_a_new_mcp_session_without_deadlocking() {
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"mock_tool"}]}}\n' "$id"
      ;;
  esac
done
"#;
        let server = McpServerConfig {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            env: HashMap::new(),
            scope_id: Some(format!("deadlock-regression-{}", request_seed())),
            secret_id: None,
        };
        let cleanup = server.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = sender.send(test_mcp_connection(server));
        });

        let status = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("new MCP session initialization deadlocked")
            .unwrap();
        assert_eq!(status.tools, vec!["mock_tool"]);
        close_mcp_session(cleanup).unwrap();
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "repeatable performance harness; run explicitly with --ignored --nocapture"]
    fn performance_baseline_mcp_cold_and_warm() {
        crate::infrastructure::performance::reset();
        crate::infrastructure::performance::set_mode(
            crate::infrastructure::performance::PerformanceMode::Profiling,
        );
        let script = r#"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"mock_tool","inputSchema":{"type":"object"}}]}}\n' "$id"
      ;;
  esac
done
"#;
        let server = McpServerConfig {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            env: HashMap::new(),
            scope_id: Some(format!("performance-baseline-{}", request_seed())),
            secret_id: None,
        };
        test_mcp_connection(server.clone()).expect("cold MCP initialization");
        test_mcp_connection(server.clone()).expect("warm MCP initialization");
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::infrastructure::performance::snapshot())
                .expect("benchmark snapshot encoded")
        );
        close_mcp_session(server).expect("benchmark MCP session closed");
    }

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
    fn accepts_case_insensitive_content_length_and_skips_blank_lines() {
        let body = br#"{"id":1,"result":{}}"#;
        let input = format!(
            "\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            std::str::from_utf8(body).unwrap()
        );
        let mut reader = BufReader::new(std::io::Cursor::new(input));

        let message = read_stdout_message(&mut reader)
            .expect("framed message should parse")
            .expect("framed message should exist");
        assert_eq!(message["id"], 1);
    }

    #[test]
    fn rejects_non_json_stdout_instead_of_treating_it_as_a_response() {
        let mut reader = BufReader::new(std::io::Cursor::new("server log\n"));
        let error = read_stdout_message(&mut reader).expect_err("invalid stdout should fail");
        assert!(error.contains("Invalid JSON on MCP stdout"));
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
            scope_id: None,
            secret_id: None,
        };
        let second = McpServerConfig {
            command: "server".to_string(),
            args: vec!["--stdio".to_string()],
            env: second_env,
            scope_id: None,
            secret_id: None,
        };

        assert_eq!(mcp_server_key(&first), mcp_server_key(&second));

        let mut other_scope = first.clone();
        other_scope.scope_id = Some("workspace-other".to_string());
        assert_ne!(mcp_server_key(&first), mcp_server_key(&other_scope));
    }

    #[test]
    fn redacts_environment_values_from_mcp_diagnostics() {
        let env = HashMap::from([
            ("TOKEN".to_string(), "super-secret-token".to_string()),
            ("SHORT".to_string(), "abc".to_string()),
        ]);

        let diagnostic = redact_mcp_diagnostic("MCP stderr: super-secret-token and abc", &env);

        assert!(!diagnostic.contains("super-secret-token"));
        assert!(diagnostic.contains("[REDACTED]"));
        assert!(diagnostic.contains("abc"));
    }

    #[test]
    fn routes_out_of_order_responses_to_the_matching_request() {
        let pending = Mutex::new(HashMap::new());
        let (first_sender, first_receiver) = mpsc::sync_channel(1);
        let (second_sender, second_receiver) = mpsc::sync_channel(1);
        pending.lock().unwrap().insert(1, first_sender);
        pending.lock().unwrap().insert(2, second_sender);

        dispatch_mcp_response(&pending, json!({ "id": 2, "result": "second" }));
        dispatch_mcp_response(&pending, json!({ "id": 1, "result": "first" }));

        assert_eq!(first_receiver.recv().unwrap()["result"], "first");
        assert_eq!(second_receiver.recv().unwrap()["result"], "second");
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn rejects_ambiguous_tool_suffix_matches() {
        let tools = vec!["jira_search".to_string(), "legacy_search".to_string()];
        let error = resolve_tool(&tools, "search").expect_err("suffix should be ambiguous");
        assert!(error.contains("ambiguous"));
        assert!(error.contains("jira_search"));
        assert!(error.contains("legacy_search"));
    }

    #[test]
    fn proxy_rejects_tool_calls_before_upstream_dispatch() {
        let tools = Mutex::new(vec!["jira_search".to_string()]);
        let allowed = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "jira_search", "arguments": { "jql": "project = APP" } }
        });
        assert!(validate_proxy_tool_call(&allowed, &tools).is_ok());

        let denied = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "jira_delete", "arguments": {} }
        });
        assert!(validate_proxy_tool_call(&denied, &tools).is_err());
    }

    #[test]
    fn proxy_rechecks_durable_assignment_authority_before_each_tool_call() {
        let directory = tempfile::tempdir().expect("temp directory");
        let store = SchedulerStore::open_at(directory.path().join("scheduler.db"))
            .expect("scheduler store opens");
        let owner = store.register_owner().expect("owner registered");
        let session = store
            .enqueue_with_grants(
                &crate::domain::task_session::TaskRequest::new("fenced-mcp"),
                &[
                    "external_tools:jira-a".to_string(),
                    "external_tools:jira-b".to_string(),
                ],
                "test-approval",
            )
            .expect("task enqueued");
        let assignment = store
            .claim_next(owner, 1, Duration::from_secs(30), 5)
            .expect("task claimed")
            .expect("assignment");
        let connector_command = vec!["jira-mcp".to_string(), "--stdio".to_string()];
        let connector_environment = HashMap::from([("JIRA_URL".to_string(), "jira-a".to_string())]);
        let connector_binding =
            mcp_connector_binding_digest("jira-a", &connector_command, &connector_environment)
                .expect("connector bound");
        let authority = store
            .external_authority(
                assignment.fence,
                "external_tools:jira-a",
                "jira-a",
                &connector_binding,
            )
            .expect("authority created");
        let tools = Mutex::new(vec!["jira_search".to_string()]);
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "jira_search", "arguments": { "jql": "project = APP" } }
        });

        assert!(validate_proxy_request(
            &request,
            &tools,
            &ProxyAssignmentAuthority::Fenced(authority.clone()),
            "jira-a",
            &connector_binding,
        )
        .is_ok());

        let mut unbound_subtask = authority.clone();
        unbound_subtask.subtask_authority = Some(SubtaskToolAuthority {
            scheduler_database: directory.path().join("scheduler.db"),
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
            capabilities: vec!["external_tools:jira-a".to_string()],
            lease_expires_at: i64::MAX as u64,
        });
        let mut mismatched_parent = unbound_subtask.clone();
        mismatched_parent
            .subtask_authority
            .as_mut()
            .expect("nested authority")
            .parent_fencing_token += 1;
        let error = validate_proxy_request(
            &request,
            &tools,
            &ProxyAssignmentAuthority::Fenced(mismatched_parent),
            "jira-a",
            &connector_binding,
        )
        .expect_err("mismatched nested parent must fail before connector forwarding");
        assert!(error.contains("does not match its parent connector authority"));
        let error = validate_proxy_request(
            &request,
            &tools,
            &ProxyAssignmentAuthority::Fenced(unbound_subtask),
            "jira-a",
            &connector_binding,
        )
        .expect_err("unbound subtask must fail before connector forwarding");
        assert!(error.contains("Subtask tool authority is stale"));

        let mut repair_scoped = authority.clone();
        repair_scoped.allowed_tools = vec!["jira_read_issue".to_string()];
        let error = validate_proxy_request(
            &request,
            &tools,
            &ProxyAssignmentAuthority::Fenced(repair_scoped),
            "jira-a",
            &connector_binding,
        )
        .expect_err("repair scope must reject a different exposed tool");
        assert!(error.contains("capability repair scope"));

        let mut ungranted = authority.clone();
        ungranted.capability = "external_tools:other".to_string();
        assert!(validate_proxy_request(
            &request,
            &tools,
            &ProxyAssignmentAuthority::Fenced(ungranted),
            "jira-a",
            &connector_binding,
        )
        .is_err());

        let other_environment = HashMap::from([("JIRA_URL".to_string(), "jira-b".to_string())]);
        let other_binding =
            mcp_connector_binding_digest("jira-b", &connector_command, &other_environment)
                .expect("other connector bound");
        assert!(validate_proxy_request(
            &request,
            &tools,
            &ProxyAssignmentAuthority::Fenced(authority.clone()),
            "jira-b",
            &other_binding,
        )
        .is_err());

        let mut stale = authority.clone();
        stale.fencing_token = stale.fencing_token.saturating_add(1);
        assert!(validate_proxy_request(
            &request,
            &tools,
            &ProxyAssignmentAuthority::Fenced(stale),
            "jira-a",
            &connector_binding,
        )
        .is_err());

        store
            .cancel(session.id)
            .expect("task cancellation requested");
        assert!(validate_proxy_request(
            &request,
            &tools,
            &ProxyAssignmentAuthority::Fenced(authority),
            "jira-a",
            &connector_binding,
        )
        .is_err());
    }

    #[test]
    fn proxy_authority_defaults_to_required_and_legacy_is_explicit() {
        assert!(parse_proxy_assignment_authority(None, None).is_err());
        assert!(matches!(
            parse_proxy_assignment_authority(Some(MCP_PROXY_AUTHORITY_MODE_LEGACY), None),
            Ok(ProxyAssignmentAuthority::Legacy)
        ));
        assert!(parse_proxy_assignment_authority(Some("unknown"), None).is_err());
    }

    #[test]
    fn proxy_serializes_json_lines_for_stdio_compatibility() {
        let mut output = Vec::new();
        write_proxy_message(&mut output, &json!({ "id": 1, "result": {} })).unwrap();

        assert_eq!(output.last(), Some(&b'\n'));
        assert_eq!(serde_json::from_slice::<Value>(&output).unwrap()["id"], 1);
    }

    #[test]
    fn proxy_classifies_valid_resource_mutation_success_before_forwarding() {
        let identity = proxy_test_identity();
        let evidence = proxy_test_evidence(
            &identity,
            ResourceLookupStatus::DriftDetected,
            ResourceExecutionStatus::Executed,
        );
        let response = json!({
            "id": 1,
            "result": { "content": [{
                "type": "text",
                "text": json!({ "resource_mutation": evidence }).to_string()
            }]}
        });
        assert!(matches!(
            classify_resource_mutation_response(&response, &proxy_test_record(&identity)),
            ResourceMutationResolution::Succeeded(_)
        ));
    }

    #[test]
    fn proxy_classifies_and_replays_confirmed_jira_comment_without_body_persistence() {
        let identity = crate::infrastructure::jira::jira_comment_operation_identity(
            &"a".repeat(64),
            "jira_add_comment",
            &json!({"issue_key": "OPS-42", "comment": "private completion detail"}),
        )
        .unwrap()
        .unwrap();
        let mut record = proxy_test_record(&identity);
        record.connector_id = "corporate-jira".to_string();
        record.tool_name = "jira_add_comment".to_string();
        let response = json!({
            "id": 1,
            "result": { "content": [{
                "type": "text",
                "text": json!({ "commentId": "10042" }).to_string()
            }]}
        });
        let ResourceMutationResolution::Succeeded(evidence) =
            classify_resource_mutation_response(&response, &record)
        else {
            panic!("Jira comment response must be a confirmed success");
        };
        assert_eq!(
            evidence.execution.resulting_version.as_deref(),
            Some("10042")
        );
        assert!(!serde_json::to_string(&evidence)
            .unwrap()
            .contains("private completion detail"));

        record.state = ResourceMutationState::Succeeded;
        record.evidence = Some(evidence);
        let output = Mutex::new(Vec::new());
        assert!(write_succeeded_jira_comment_replay(&output, &json!(7), &record).unwrap());
        let replay: Value = serde_json::from_slice(&output.into_inner().unwrap()).unwrap();
        assert_eq!(replay["id"], 7);
        assert_eq!(replay["result"]["structuredContent"]["commentId"], "10042");
        assert_eq!(
            replay["result"]["structuredContent"]["idempotency_status"],
            "already_complete"
        );
    }

    #[test]
    fn proxy_classifies_valid_restart_mutation_success_before_forwarding() {
        let identity = proxy_test_restart_identity();
        let evidence = proxy_test_evidence(
            &identity,
            ResourceLookupStatus::DriftDetected,
            ResourceExecutionStatus::Executed,
        );
        let response = json!({
            "id": 1,
            "result": { "content": [{
                "type": "text",
                "text": json!({ "resource_mutation": evidence }).to_string()
            }]}
        });
        let mut record = proxy_test_record(&identity);
        record.tool_name = "ocp_restart_deployment".to_string();

        assert!(matches!(
            classify_resource_mutation_response(&response, &record),
            ResourceMutationResolution::Succeeded(_)
        ));
    }

    #[test]
    fn proxy_releases_approval_and_conflict_but_retains_ambiguous_outcomes() {
        let identity = proxy_test_identity();
        let record = proxy_test_record(&identity);
        let approval = json!({
            "id": 1,
            "result": { "content": [{
                "type": "text",
                "text": json!({
                    "status": "approval_required",
                    "operation_identity": identity
                }).to_string()
            }]}
        });
        assert!(matches!(
            classify_resource_mutation_response(&approval, &record),
            ResourceMutationResolution::Failed { code, .. } if code == "approval_required"
        ));

        let conflict_evidence = proxy_test_evidence(
            &record.identity,
            ResourceLookupStatus::DriftDetected,
            ResourceExecutionStatus::Conflict,
        );
        let conflict = json!({
            "id": 1,
            "error": { "data": { "resource_mutation": conflict_evidence } }
        });
        assert!(matches!(
            classify_resource_mutation_response(&conflict, &record),
            ResourceMutationResolution::Failed { .. }
        ));
        let blocked_evidence = proxy_test_evidence(
            &record.identity,
            ResourceLookupStatus::DriftDetected,
            ResourceExecutionStatus::Blocked,
        );
        let forbidden = json!({
            "id": 1,
            "error": {
                "data": {
                    "kind": "forbidden",
                    "resource_mutation": blocked_evidence
                }
            }
        });
        assert!(matches!(
            classify_resource_mutation_response(&forbidden, &record),
            ResourceMutationResolution::Failed { .. }
        ));
        let ambiguous_evidence = proxy_test_evidence(
            &record.identity,
            ResourceLookupStatus::DriftDetected,
            ResourceExecutionStatus::Blocked,
        );
        let transport = json!({
            "id": 1,
            "error": {
                "data": {
                    "kind": "connect",
                    "resource_mutation": ambiguous_evidence
                }
            }
        });
        assert!(matches!(
            classify_resource_mutation_response(&transport, &record),
            ResourceMutationResolution::Uncertain { .. }
        ));
        assert!(matches!(
            classify_resource_mutation_response(&json!({ "id": 1, "result": {} }), &record),
            ResourceMutationResolution::Uncertain { code, .. }
                if code == "missing_resource_mutation_evidence"
        ));
    }

    #[test]
    fn proxy_rejects_mismatched_identity_and_invalid_mutation_request_ids() {
        let identity = proxy_test_identity();
        let mut other = proxy_test_identity();
        other.resource.name = "other".to_string();
        let evidence = proxy_test_evidence(
            &other,
            ResourceLookupStatus::DriftDetected,
            ResourceExecutionStatus::Executed,
        );
        let response = json!({
            "id": 1,
            "result": { "content": [{
                "type": "text",
                "text": json!({ "resource_mutation": evidence }).to_string()
            }]}
        });
        assert!(matches!(
            classify_resource_mutation_response(&response, &proxy_test_record(&identity)),
            ResourceMutationResolution::Uncertain { .. }
        ));
        assert!(validated_resource_mutation_request_id(&json!({ "id": 1 })).is_ok());
        assert!(validated_resource_mutation_request_id(&json!({ "id": "call" })).is_ok());
        assert!(validated_resource_mutation_request_id(&json!({ "id": null })).is_err());
        assert!(validated_resource_mutation_request_id(&json!({})).is_err());
    }

    #[test]
    fn only_transport_failures_invalidate_reusable_mcp_sessions() {
        assert!(!mcp_error_invalidates_session(
            "MCP request failed: tool rejected the arguments"
        ));
        assert!(!mcp_error_invalidates_session(
            "Failed to parse Jira issues from the tool result"
        ));
        assert!(mcp_error_invalidates_session(
            "Timed out waiting for MCP response after 45s"
        ));
        assert!(mcp_error_invalidates_session(
            "MCP protocol reader failed: invalid JSON"
        ));
    }
}
