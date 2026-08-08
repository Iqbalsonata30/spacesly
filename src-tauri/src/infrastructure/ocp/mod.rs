pub mod audit;
mod breaker;
mod client;
pub mod config;
mod errors;
mod preflight;
mod retry;
mod tools;

pub use config::OcpConfigSpec;
pub use config::OcpTimeoutPolicy;
pub use errors::{OcpError, OcpStructuredError};
pub use preflight::PreflightReport;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use super::ai_worker::AiWorkerMcpServer;
use super::mcp::{read_stdout_message, write_proxy_message};
use super::tool_broker::argument_digest;
use audit::{AuditEntry, AuditLog};
use breaker::CircuitBreaker;
use client::{OcpClient, OcpTimeouts};
use config::{
    build_api_server_config, build_in_cluster_config, parse_kubeconfig, ConfigStore, OcpAuthMode,
    ResolvedCluster, CONFIG_SCHEMA_VERSION, LAST_KNOWN_GOOD_FILE,
};
use errors::OcpResult;
use preflight::run_preflight;
use retry::{with_retry, RetryPolicy};
use tools::{execute_tool, tool_metadata, OcpTool};

pub const ENV_MODE: &str = "SPACESLY_OCP_MODE";
pub const ENV_KUBECONFIG: &str = "SPACESLY_OCP_KUBECONFIG";
pub const ENV_CONTEXT: &str = "SPACESLY_OCP_CONTEXT";
pub const ENV_SERVER: &str = "SPACESLY_OCP_SERVER";
pub const ENV_DEFAULT_NAMESPACE: &str = "SPACESLY_OCP_DEFAULT_NAMESPACE";
pub const ENV_CREDENTIALS_FILE: &str = "SPACESLY_OCP_CREDENTIALS_FILE";
pub const ENV_CONNECTOR_DIR: &str = "SPACESLY_OCP_CONNECTOR_DIR";
pub const ENV_APPROVED_OPERATION: &str = "SPACESLY_OCP_APPROVED_OPERATION";
pub const ENV_APPROVED_ARGUMENTS_DIGEST: &str = "SPACESLY_OCP_APPROVED_ARGUMENTS_DIGEST";

const CONNECTOR_ARG: &str = "--spacesly-ocp-connector";
const BREAKER_THRESHOLD: u32 = 3;
const BREAKER_RESET_AFTER_SECS: u64 = 30;
const BREAKER_STATE_FILE: &str = "breaker.json";
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OcpCredentialsFile {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub ca_data: Option<String>,
}

/// Payload for the `ocp_save_draft` Tauri command. Secret values are accepted
/// as optional inputs only; they are persisted by the backend and never
/// returned to the renderer.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OcpConfigInput {
    pub mode: String,
    pub kubeconfig_path: Option<String>,
    pub kubeconfig_context: Option<String>,
    pub server: Option<String>,
    pub default_namespace: Option<String>,
    pub display_name: Option<String>,
    pub environment_label: Option<String>,
    pub token: Option<String>,
    pub ca_pem_base64: Option<String>,
    pub server_id: String,
    /// Optional per-stage timeout overrides. Absent fields fall back to
    /// `OcpTimeoutPolicy::default()` (10s / 30s / 60s).
    #[serde(default)]
    pub timeout_policy: Option<OcpTimeoutPolicy>,
}

/// Which OCP secret types are currently persisted. Never exposes values.
#[derive(Clone, Debug, Serialize)]
pub struct OcpSecretStatus {
    pub token_set: bool,
    pub ca_data_set: bool,
}

pub fn ocp_secret_status(dir: &Path) -> OcpSecretStatus {
    let credentials = read_credentials_file(&dir.join("credentials.json")).unwrap_or_default();
    OcpSecretStatus {
        token_set: credentials
            .token
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false),
        ca_data_set: credentials
            .ca_data
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false),
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::result_large_err)]
pub fn build_ocp_spec(
    mode: &str,
    kubeconfig_path: Option<&str>,
    kubeconfig_context: Option<&str>,
    server: Option<&str>,
    default_namespace: Option<&str>,
    display_name: Option<&str>,
    environment_label: Option<&str>,
    token_set: bool,
    ca_data_set: bool,
    timeout_policy: Option<&OcpTimeoutPolicy>,
) -> Result<OcpConfigSpec, OcpStructuredError> {
    let mode = mode.trim();
    let parsed_mode = OcpAuthMode::parse(mode).ok_or_else(|| {
        OcpStructuredError::from_error(
            &OcpError::config("config_mode", format!("Unknown OCP auth mode '{mode}'.")),
            Some("config"),
        )
    })?;
    let timeout_policy = timeout_policy
        .cloned()
        .unwrap_or_default()
        .validate()
        .map_err(|error| OcpStructuredError::from_error(&error, Some("config")))?;
    let spec = OcpConfigSpec {
        version: CONFIG_SCHEMA_VERSION,
        mode: parsed_mode.as_str().to_string(),
        kubeconfig_path: trim_to_opt(kubeconfig_path),
        kubeconfig_context: trim_to_opt(kubeconfig_context),
        server: trim_to_opt(server),
        ca_data_set,
        token_set,
        default_namespace: trim_to_opt(default_namespace),
        display_name: trim_to_opt(display_name),
        environment_label: trim_to_opt(environment_label),
        timeout_policy,
        preflight_passed: false,
        updated_at_ms: config::now_millis(),
        checksum: String::new(),
    }
    .sealed();
    spec.validate()
        .map_err(|error| OcpStructuredError::from_error(&error, Some("config")))?;
    Ok(spec)
}

fn trim_to_opt(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Persist a draft spec and its credentials atomically, then return the sealed
/// spec so the caller can register the connector profile.
#[allow(clippy::result_large_err)]
pub fn save_draft(
    input: &OcpConfigInput,
    connector_dir: &Path,
) -> Result<OcpConfigSpec, OcpStructuredError> {
    let existing = read_credentials_file(&connector_dir.join("credentials.json"))
        .map_err(|error| OcpStructuredError::from_error(&error, Some("config")))?;
    let new_token = trim_to_opt(input.token.as_deref());
    let new_ca = trim_to_opt(input.ca_pem_base64.as_deref());
    let token = new_token
        .as_deref()
        .or(existing.token.as_deref())
        .map(str::to_string);
    let ca_pem_base64 = new_ca
        .as_deref()
        .or(existing.ca_data.as_deref())
        .map(str::to_string);
    let ca_data = ca_pem_base64.as_deref().and_then(|encoded| {
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()
    });

    let spec = build_ocp_spec(
        &input.mode,
        input.kubeconfig_path.as_deref(),
        input.kubeconfig_context.as_deref(),
        input.server.as_deref(),
        input.default_namespace.as_deref(),
        input.display_name.as_deref(),
        input.environment_label.as_deref(),
        token.as_deref().is_some(),
        ca_pem_base64.as_deref().is_some(),
        input.timeout_policy.as_ref(),
    )?;

    write_credentials_file(connector_dir, token.as_deref(), ca_data.as_deref())
        .map_err(|error| OcpStructuredError::from_error(&error, Some("config")))?;
    ConfigStore::new(connector_dir.to_path_buf())
        .save_draft(&spec)
        .map_err(|error| OcpStructuredError::from_error(&error, Some("config")))?;
    Ok(spec)
}

/// Build the OPENSHIFT_* environment map that the embedded OCP connector reads
/// at launch. Mirror of `ocp_worker_server`'s expectations.
pub fn ocp_environment(
    spec: &OcpConfigSpec,
    token: Option<&str>,
    ca_pem_base64: Option<&str>,
) -> HashMap<String, String> {
    let mut environment = HashMap::new();
    environment.insert("OPENSHIFT_MODE".to_string(), spec.mode.clone());
    match OcpAuthMode::parse(&spec.mode) {
        Some(OcpAuthMode::ApiServerToken) => {
            if let Some(server) = spec.server.as_deref() {
                environment.insert("OPENSHIFT_SERVER".to_string(), server.to_string());
            }
        }
        Some(OcpAuthMode::Kubeconfig) => {
            if let Some(path) = spec.kubeconfig_path.as_deref() {
                environment.insert("KUBECONFIG".to_string(), path.to_string());
            }
        }
        _ => {}
    }
    if let Some(context) = spec.kubeconfig_context.as_deref() {
        environment.insert("OPENSHIFT_CONTEXT".to_string(), context.to_string());
    }
    if let Some(namespace) = spec.default_namespace.as_deref() {
        environment.insert(
            "OPENSHIFT_DEFAULT_NAMESPACE".to_string(),
            namespace.to_string(),
        );
    }
    if let Some(token) = token.map(str::trim).filter(|value| !value.is_empty()) {
        environment.insert("OPENSHIFT_TOKEN".to_string(), token.to_string());
    }
    if let Some(ca) = ca_pem_base64
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        environment.insert("OPENSHIFT_CA_DATA".to_string(), ca.to_string());
    }
    environment
}

/// Merge newly-supplied secret values with any persisted ones, preferring the
/// new values. Used by preflight (test-only) and save flows so the effective
/// token/CA is resolved exactly once.
pub fn effective_credentials(
    connector_dir: &Path,
    token: Option<&str>,
    ca_pem_base64: Option<&str>,
) -> OcpCredentialsFile {
    let stored = read_credentials_file(&connector_dir.join("credentials.json")).unwrap_or_default();
    OcpCredentialsFile {
        token: trim_to_opt(token).or(stored.token),
        ca_data: trim_to_opt(ca_pem_base64).or(stored.ca_data),
    }
}

/// Rotate persisted credentials and reset preflight state so the connector must
/// be re-tested. Leaves the draft spec's non-secret fields untouched.
#[allow(clippy::result_large_err)]
pub fn rotate_credentials(
    connector_dir: &Path,
    token: Option<&str>,
    ca_pem_base64: Option<&str>,
) -> Result<OcpConfigSpec, OcpStructuredError> {
    let existing = read_credentials_file(&connector_dir.join("credentials.json"))
        .map_err(|error| OcpStructuredError::from_error(&error, Some("config")))?;
    let new_token = trim_to_opt(token);
    let new_ca = trim_to_opt(ca_pem_base64);
    let next_token = new_token.as_deref().or(existing.token.as_deref());
    let next_ca = new_ca.as_deref().or(existing.ca_data.as_deref());
    let ca_data = next_ca.and_then(|encoded| {
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()
    });

    write_credentials_file(connector_dir, next_token, ca_data.as_deref())
        .map_err(|error| OcpStructuredError::from_error(&error, Some("config")))?;

    let store = ConfigStore::new(connector_dir.to_path_buf());
    let mut spec = store.load_draft().ok().flatten().ok_or_else(|| {
        OcpStructuredError::from_error(
            &OcpError::config(
                "config_no_draft",
                "Cannot rotate credentials before a draft configuration is saved.",
            ),
            Some("config"),
        )
    })?;
    spec.preflight_passed = false;
    spec.token_set = next_token.is_some();
    spec.ca_data_set = next_ca.is_some();
    spec.updated_at_ms = config::now_millis();
    let spec = spec.sealed();
    store
        .save_draft(&spec)
        .map_err(|error| OcpStructuredError::from_error(&error, Some("config")))?;
    let _ = fs::remove_file(connector_dir.join(LAST_KNOWN_GOOD_FILE));
    Ok(spec)
}

/// Remove all connector data: draft, last-known-good, credentials, audit log.
pub fn delete_connector(connector_dir: &Path) -> OcpResult<()> {
    for name in [
        "draft.json",
        "last-known-good.json",
        "credentials.json",
        "audit.ndjson",
        BREAKER_STATE_FILE,
    ] {
        let path = connector_dir.join(name);
        if path.exists() {
            fs::remove_file(&path).map_err(|error| {
                OcpError::config(
                    "connector_delete",
                    format!(
                        "Failed to remove OCP connector data '{}': {error}",
                        path.display()
                    ),
                )
            })?;
        }
    }
    Ok(())
}

pub struct OcpConnectorEnv {
    pub mode: OcpAuthMode,
    pub kubeconfig: Option<String>,
    pub context: Option<String>,
    pub server: Option<String>,
    pub default_namespace: Option<String>,
    pub credentials_file: Option<PathBuf>,
    pub connector_dir: Option<PathBuf>,
    pub approved_operation: Option<String>,
    pub approved_arguments_digest: Option<String>,
}

impl OcpConnectorEnv {
    pub fn from_env() -> Result<Self, String> {
        let mode = required_env(ENV_MODE)?;
        let mode = OcpAuthMode::parse(&mode)
            .ok_or_else(|| format!("OCP connector mode '{mode}' is unknown."))?;
        Ok(Self {
            mode,
            kubeconfig: optional_env(ENV_KUBECONFIG),
            context: optional_env(ENV_CONTEXT),
            server: optional_env(ENV_SERVER),
            default_namespace: optional_env(ENV_DEFAULT_NAMESPACE),
            credentials_file: optional_env(ENV_CREDENTIALS_FILE).map(PathBuf::from),
            connector_dir: optional_env(ENV_CONNECTOR_DIR).map(PathBuf::from),
            approved_operation: optional_env(ENV_APPROVED_OPERATION),
            approved_arguments_digest: optional_env(ENV_APPROVED_ARGUMENTS_DIGEST),
        })
    }
}

pub fn run_ocp_mcp_server() -> Result<(), String> {
    let env = OcpConnectorEnv::from_env()?;
    let cluster = resolve_cluster(&env).map_err(|error| error.to_string())?;
    let timeouts = OcpTimeouts::default();
    let client = OcpClient::build(&cluster, timeouts).map_err(|error| error.to_string())?;
    let secrets = cluster.secret_snapshot();
    let audit = AuditLog::new(
        env.connector_dir
            .clone()
            .unwrap_or_else(default_connector_dir_silent),
    );
    let breaker = CircuitBreaker::new(
        BREAKER_THRESHOLD,
        std::time::Duration::from_secs(BREAKER_RESET_AFTER_SECS),
    );
    let mutation_used = AtomicBool::new(false);
    serve_stdio(
        client,
        &breaker,
        &audit,
        secrets,
        env.approved_operation.as_deref(),
        env.approved_arguments_digest.as_deref(),
        &mutation_used,
        BufReader::new(std::io::stdin()),
        std::io::stdout(),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovedMutation {
    pub operation: String,
    pub arguments_digest: String,
}

/// Reads the most recent structured UI approval from the immutable execution
/// contract. Free-form operator notes are intentionally never treated as approval.
pub fn contract_approved_mutation(contract: &Value) -> Option<ApprovedMutation> {
    let contract_version = contract.get("version")?.as_u64()?;
    contract
        .pointer("/runtime_inputs/approvals")?
        .as_array()?
        .iter()
        .rev()
        .find_map(|approval| {
            if approval.get("decision").and_then(Value::as_str) != Some("approved") {
                return None;
            }
            if approval.get("contract_version").and_then(Value::as_u64) != Some(contract_version) {
                return None;
            }
            let operation = canonical_approval_operation(approval.get("operation")?.as_str()?);
            let arguments_digest = approval.get("arguments_digest")?.as_str()?.trim();
            if operation.is_empty() || arguments_digest.is_empty() {
                return None;
            }
            Some(ApprovedMutation {
                operation: operation.to_string(),
                arguments_digest: arguments_digest.to_string(),
            })
        })
}

fn canonical_approval_operation(value: &str) -> &str {
    let operation = value.trim();
    if operation.starts_with("ocp_") || operation.starts_with("kubernetes_") {
        return operation;
    }
    ["_ocp_", "_kubernetes_"]
        .iter()
        .filter_map(|marker| operation.rfind(marker).map(|index| &operation[index + 1..]))
        .find(|candidate| candidate.starts_with("ocp_") || candidate.starts_with("kubernetes_"))
        .unwrap_or("")
}

fn default_connector_dir_silent() -> PathBuf {
    config::default_connector_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn resolve_cluster(env: &OcpConnectorEnv) -> OcpResult<ResolvedCluster> {
    let credentials = match &env.credentials_file {
        Some(path) => read_credentials_file(path)?,
        None => OcpCredentialsFile::default(),
    };
    match env.mode {
        OcpAuthMode::Kubeconfig => {
            let path = env.kubeconfig.as_deref().ok_or_else(|| {
                OcpError::config(
                    "kubeconfig_required",
                    "Kubeconfig mode requires a kubeconfig path.",
                )
            })?;
            let mut cluster = parse_kubeconfig(Path::new(path), env.context.as_deref())?;
            if cluster.credentials.is_anonymous() {
                if let Some(token) = credentials.token.filter(|value| !value.trim().is_empty()) {
                    cluster.credentials = cluster.credentials.bearer_token(token);
                }
            }
            Ok(cluster)
        }
        OcpAuthMode::ApiServerToken => {
            let server = env.server.as_deref().ok_or_else(|| {
                OcpError::config("server_required", "API server mode requires a server URL.")
            })?;
            let token = credentials.token.unwrap_or_default();
            let ca_data = credentials.ca_data.as_deref().and_then(|encoded| {
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded).ok()
            });
            build_api_server_config(
                server,
                &token,
                ca_data.as_deref(),
                env.default_namespace.as_deref(),
            )
        }
        OcpAuthMode::InCluster => build_in_cluster_config(),
    }
}

fn serve_stdio<R: std::io::BufRead, W: std::io::Write>(
    client: OcpClient,
    breaker: &CircuitBreaker,
    audit: &AuditLog,
    secrets: Vec<String>,
    approved_operation: Option<&str>,
    approved_arguments_digest: Option<&str>,
    mutation_used: &AtomicBool,
    mut reader: R,
    mut writer: W,
) -> Result<(), String> {
    while let Some(message) = read_stdout_message(&mut reader)? {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        match method {
            Some("initialize") => {
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": {
                            "name": "spacesly-ocp",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                });
                write_proxy_message(&mut writer, &response)?;
            }
            Some("notifications/initialized") | Some("initialized") => {}
            Some("ping") => {
                write_proxy_message(
                    &mut writer,
                    &json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
                )?;
            }
            Some("tools/list") => {
                write_proxy_message(
                    &mut writer,
                    &json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tool_metadata() } }),
                )?;
            }
            Some("tools/call") => {
                let started = Instant::now();
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let tool_name = params["name"].as_str().unwrap_or("").to_string();
                let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
                audit.record_best_effort(
                    "tool_started",
                    Some(&tool_name),
                    None,
                    "started",
                    None,
                    0,
                );
                let outcome = call_tool(
                    &client,
                    breaker,
                    &tool_name,
                    &arguments,
                    approved_operation,
                    approved_arguments_digest,
                    mutation_used,
                );
                let latency_ms = started.elapsed().as_millis() as u64;
                match outcome {
                    Ok(value) => {
                        breaker.record_success();
                        persist_breaker_state(audit.dir(), breaker);
                        audit.record_best_effort(
                            "tool_finished",
                            Some(&tool_name),
                            None,
                            "success",
                            None,
                            latency_ms,
                        );
                        write_proxy_message(
                            &mut writer,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "content": [{ "type": "text", "text": output_text(&value) }]
                                }
                            }),
                        )?;
                    }
                    Err(error) => {
                        let secret_slices: Vec<&str> = secrets.iter().map(String::as_str).collect();
                        let redacted = error.redacted(&secret_slices);
                        // Cancellations and permanent failures (config, auth, TLS,
                        // protocol) are not backend overload, so they must not trip
                        // the circuit breaker.
                        if !error.is_cancelled() && !error.is_permanent() {
                            breaker.record_failure();
                        }
                        persist_breaker_state(audit.dir(), breaker);
                        audit.record_best_effort(
                            "tool_failed",
                            Some(&tool_name),
                            None,
                            "failed",
                            Some(&redacted.to_string()),
                            latency_ms,
                        );
                        write_proxy_error(&mut writer, id.as_ref(), &redacted.to_string())?;
                    }
                }
            }
            _ => {
                let error = OcpError::protocol("mcp_method_not_found", "MCP method not found.");
                write_proxy_error(&mut writer, id.as_ref(), &error.message)?;
            }
        }
    }
    Ok(())
}

fn call_tool(
    client: &OcpClient,
    breaker: &CircuitBreaker,
    tool_name: &str,
    arguments: &Value,
    approved_operation: Option<&str>,
    approved_arguments_digest: Option<&str>,
    mutation_used: &AtomicBool,
) -> OcpResult<Value> {
    breaker.allow()?;
    let tool = OcpTool::parse(tool_name)
        .ok_or_else(|| OcpError::internal(format!("Unknown OCP tool '{tool_name}'.")))?;
    if tool.is_mutation() {
        let actual_digest = argument_digest(arguments).map_err(|error| {
            OcpError::config(
                "invalid_arguments",
                format!("Could not identify OpenShift operation arguments: {error}"),
            )
        })?;
        if approved_operation != Some(tool.as_str())
            || approved_arguments_digest != Some(actual_digest.as_str())
        {
            // Report approval as a successful MCP tool result. OpenCode retries JSON-RPC
            // errors internally, which can issue the same mutation repeatedly and flood the
            // console. Spacesly recognizes this structured result and pauses the run itself.
            return Ok(json!({
                "status": "approval_required",
                "operation": tool.as_str(),
                "arguments_digest": actual_digest,
                "message": "This action is waiting for an operator decision in Spacesly. Do not retry it."
            }));
        }
        if mutation_used
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(OcpError::config(
                "approval_consumed",
                "This operator approval already authorized one OpenShift mutation. Ask for confirmation again before another change.",
            ));
        }
        // Mutations are intentionally never retried: an ambiguous network response must not
        // repeat a side effect under the same approval.
        return execute_tool(client, tool_name, arguments);
    }
    let cancelled = AtomicBool::new(false);
    with_retry(RetryPolicy::default(), &cancelled, || {
        execute_tool(client, tool_name, arguments)
    })
}

fn output_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()),
    }
}

fn write_proxy_error(
    stdout: &mut impl std::io::Write,
    id: Option<&Value>,
    message: &str,
) -> Result<(), String> {
    let response = json!({
        "jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(Value::Null),
        "error": { "code": -32000, "message": message },
    });
    write_proxy_message(stdout, &response)
}

fn required_env(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("OCP connector environment '{name}' was required."))
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub fn write_credentials_file(
    dir: &Path,
    token: Option<&str>,
    ca_data: Option<&[u8]>,
) -> OcpResult<PathBuf> {
    fs::create_dir_all(dir).map_err(|error| {
        OcpError::config(
            "credentials_dir",
            format!("Failed to create the OCP connector directory: {error}"),
        )
    })?;
    let credentials = OcpCredentialsFile {
        token: token
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        ca_data: ca_data
            .filter(|data| !data.is_empty())
            .map(|data| base64::engine::general_purpose::STANDARD.encode(data)),
    };
    let payload = serde_json::to_string_pretty(&credentials).map_err(|error| {
        OcpError::config(
            "credentials_encode",
            format!("Failed to encode OCP connector credentials: {error}"),
        )
    })?;
    let path = dir.join("credentials.json");
    let temp_path = dir.join("credentials.json.tmp");
    fs::write(&temp_path, payload).map_err(|error| {
        OcpError::config(
            "credentials_write",
            format!("Failed to write OCP connector credentials: {error}"),
        )
    })?;
    set_private_file_permissions(&temp_path)?;
    fs::rename(&temp_path, &path).map_err(|error| {
        OcpError::config(
            "credentials_write",
            format!("Failed to persist OCP connector credentials: {error}"),
        )
    })?;
    Ok(path)
}

pub fn read_credentials_file(path: &Path) -> OcpResult<OcpCredentialsFile> {
    if !path.exists() {
        return Ok(OcpCredentialsFile::default());
    }
    let raw = fs::read_to_string(path).map_err(|error| {
        OcpError::config(
            "credentials_read",
            format!("Failed to read OCP connector credentials: {error}"),
        )
    })?;
    serde_json::from_str(&raw).map_err(|error| {
        OcpError::config(
            "credentials_parse",
            format!("Failed to parse OCP connector credentials: {error}"),
        )
    })
}

pub fn resolve_cluster_from_spec(
    spec: &OcpConfigSpec,
    token: Option<&str>,
    ca_data: Option<&[u8]>,
) -> OcpResult<ResolvedCluster> {
    spec.validate()?;
    match OcpAuthMode::parse(&spec.mode) {
        Some(OcpAuthMode::Kubeconfig) => {
            let path = spec.kubeconfig_path.as_deref().ok_or_else(|| {
                OcpError::config("kubeconfig_required", "Kubeconfig path is required.")
            })?;
            parse_kubeconfig(Path::new(path), spec.kubeconfig_context.as_deref())
        }
        Some(OcpAuthMode::ApiServerToken) => {
            let server = spec.server.as_deref().unwrap_or("");
            build_api_server_config(
                server,
                token.unwrap_or_default(),
                ca_data,
                spec.default_namespace.as_deref(),
            )
        }
        Some(OcpAuthMode::InCluster) => build_in_cluster_config(),
        None => Err(OcpError::config("config_mode", "Unknown OCP auth mode.")),
    }
}

#[allow(clippy::result_large_err)]
pub fn run_preflight_connection(
    spec: &OcpConfigSpec,
    token: Option<&str>,
    ca_data: Option<&[u8]>,
    connector_dir: PathBuf,
) -> Result<PreflightReport, OcpStructuredError> {
    let audit = AuditLog::new(connector_dir.clone());
    let correlation_id = format!("pf-{:x}", config::now_millis());
    audit.record_with_context_best_effort(
        "preflight_started",
        "started",
        Some(&format!("mode={}", spec.mode)),
        0,
        &correlation_id,
        "run_preflight",
    );
    let started = Instant::now();
    let cluster = resolve_cluster_from_spec(spec, token, ca_data)
        .map_err(|error| OcpStructuredError::from_error(&error, Some("config")))?;
    let timeouts = OcpTimeouts::from_policy(&spec.timeout_policy);
    let client = OcpClient::build(&cluster, timeouts)
        .map_err(|error| OcpStructuredError::from_error(&error, Some("config")))?;
    let report = run_preflight(&cluster, &client, &spec.timeout_policy);
    let latency_ms = started.elapsed().as_millis() as u64;

    // Emit one audit event per check stage for diagnosability.
    for check in &report.checks {
        let outcome = if check.passed {
            "passed"
        } else if check.required {
            "failed"
        } else {
            "warning"
        };
        audit.record_with_context_best_effort(
            &format!("preflight_stage_{}", check.stage),
            outcome,
            Some(&check.detail),
            check.duration_ms,
            &correlation_id,
            "run_preflight",
        );
    }

    audit.record_with_context_best_effort(
        "preflight_finished",
        if report.passed { "passed" } else { "failed" },
        Some(if report.passed {
            "All required checks passed."
        } else {
            "One or more required checks failed."
        }),
        latency_ms,
        &correlation_id,
        "run_preflight",
    );
    if report.passed {
        let store = ConfigStore::new(connector_dir);
        if let Ok(Some(draft)) = store.load_draft() {
            if draft.checksum == spec.checksum {
                let mut updated = draft;
                updated.preflight_passed = true;
                updated.updated_at_ms = config::now_millis();
                let _ = store.save_draft(&updated.sealed());
            }
        }
        if let Err(error) = store.promote_draft_to_last_known_good() {
            eprintln!("OCP last-known-good promotion failed: {error}");
        }
    }
    Ok(report)
}

#[derive(Clone, Debug, Serialize)]
pub struct OcpConnectorStatus {
    pub config: Option<OcpConfigSpec>,
    pub last_known_good: Option<OcpConfigSpec>,
    pub breaker_state: String,
    pub audit: Vec<AuditEntry>,
}

/// Best-effort persist of the runtime circuit-breaker snapshot so the parent
/// (Tauri) process can surface real breaker health from the MCP subprocess.
fn persist_breaker_state(dir: &Path, breaker: &CircuitBreaker) {
    let snapshot = breaker.snapshot();
    let path = dir.join(BREAKER_STATE_FILE);
    let bytes = match serde_json::to_vec(&snapshot) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("OCP breaker state serialization failed: {error}");
            return;
        }
    };
    if let Err(error) = fs::write(&path, bytes) {
        eprintln!("OCP breaker state persist failed: {error}");
    }
}

/// Read a persisted breaker snapshot, if present. A missing or stale file
/// simply means no subprocess has reported state yet.
fn load_breaker_state(dir: &Path) -> Option<breaker::BreakerSnapshot> {
    let path = dir.join(BREAKER_STATE_FILE);
    let bytes = fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn connector_status(connector_dir: PathBuf) -> OcpConnectorStatus {
    let store = ConfigStore::new(connector_dir);
    let audit = AuditLog::new(store.dir().to_path_buf());
    let breaker_state = load_breaker_state(store.dir())
        .map(|snapshot| {
            breaker::CircuitBreaker::from_snapshot(&snapshot)
                .state()
                .as_str()
                .to_string()
        })
        .unwrap_or_else(|| {
            CircuitBreaker::new(
                BREAKER_THRESHOLD,
                std::time::Duration::from_secs(BREAKER_RESET_AFTER_SECS),
            )
            .state()
            .as_str()
            .to_string()
        });
    OcpConnectorStatus {
        config: store.load_draft().ok().flatten(),
        last_known_good: store.load_last_known_good().ok().flatten(),
        breaker_state,
        audit: audit.entries(50),
    }
}
pub fn ocp_worker_server(
    secret_id: &str,
    env: &HashMap<String, String>,
) -> Result<AiWorkerMcpServer, String> {
    let mode = ocp_mode_from_env(env);
    let connector_dir = config::default_connector_dir().map_err(|error| error.to_string())?;
    let token = env.get("OPENSHIFT_TOKEN").map(String::as_str);
    let ca_data = env
        .get("OPENSHIFT_CA_DATA")
        .and_then(|value| base64::engine::general_purpose::STANDARD.decode(value).ok());
    let credentials_file = write_credentials_file(&connector_dir, token, ca_data.as_deref())
        .map_err(|error| error.to_string())?;

    let mut environment = HashMap::new();
    environment.insert(ENV_MODE.to_string(), mode.as_str().to_string());
    if let Some(server) = env.get("OPENSHIFT_SERVER") {
        if !server.trim().is_empty() {
            environment.insert(ENV_SERVER.to_string(), server.trim().to_string());
        }
    }
    if let Some(kubeconfig) = env.get("KUBECONFIG") {
        if !kubeconfig.trim().is_empty() {
            environment.insert(ENV_KUBECONFIG.to_string(), kubeconfig.trim().to_string());
        }
    }
    if let Some(context) = env.get("OPENSHIFT_CONTEXT") {
        if !context.trim().is_empty() {
            environment.insert(ENV_CONTEXT.to_string(), context.trim().to_string());
        }
    }
    if let Some(namespace) = env.get("OPENSHIFT_DEFAULT_NAMESPACE") {
        if !namespace.trim().is_empty() {
            environment.insert(
                ENV_DEFAULT_NAMESPACE.to_string(),
                namespace.trim().to_string(),
            );
        }
    }
    environment.insert(
        ENV_CREDENTIALS_FILE.to_string(),
        credentials_file.to_string_lossy().to_string(),
    );
    environment.insert(
        ENV_CONNECTOR_DIR.to_string(),
        connector_dir.to_string_lossy().to_string(),
    );

    Ok(AiWorkerMcpServer {
        name: secret_id.to_string(),
        secret_id: secret_id.to_string(),
        command: ocp_worker_command()?,
        environment,
        proxy_authority: None,
    })
}

/// Resolve the argv for the embedded OCP connector subprocess.
pub fn ocp_worker_command() -> Result<Vec<String>, String> {
    Ok(vec![
        std::env::current_exe()
            .map_err(|error| format!("Failed to resolve the Spacesly executable: {error}"))?
            .to_string_lossy()
            .to_string(),
        CONNECTOR_ARG.to_string(),
    ])
}

fn ocp_mode_from_env(env: &HashMap<String, String>) -> OcpAuthMode {
    if env
        .get("OPENSHIFT_SERVER")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        OcpAuthMode::ApiServerToken
    } else if env
        .get("KUBECONFIG")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        OcpAuthMode::Kubeconfig
    } else {
        OcpAuthMode::InCluster
    }
}

pub fn is_ocp_connector_env(env: &HashMap<String, String>) -> bool {
    env.contains_key("OPENSHIFT_MODE")
        || env.contains_key("OPENSHIFT_SERVER")
        || env.contains_key("OPENSHIFT_TOKEN")
        || env.contains_key("KUBECONFIG")
        || env.contains_key("OPENSHIFT_CA_DATA")
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> OcpResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        OcpError::config(
            "credentials_perms",
            format!("Failed to set file permissions: {error}"),
        )
    })
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> OcpResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::ocp::tools::OcpTool;

    #[test]
    fn mode_detection_prioritizes_api_server_then_kubeconfig() {
        let mut env = HashMap::new();
        assert_eq!(ocp_mode_from_env(&env), OcpAuthMode::InCluster);
        env.insert("OPENSHIFT_SERVER".to_string(), "https://x:6443".to_string());
        assert_eq!(ocp_mode_from_env(&env), OcpAuthMode::ApiServerToken);
        env.insert("KUBECONFIG".to_string(), "/tmp/kube".to_string());
        assert_eq!(ocp_mode_from_env(&env), OcpAuthMode::ApiServerToken);
        env.remove("OPENSHIFT_SERVER");
        assert_eq!(ocp_mode_from_env(&env), OcpAuthMode::Kubeconfig);
    }

    #[test]
    fn connector_env_detection_covers_ocp_keys() {
        let mut env = HashMap::new();
        assert!(!is_ocp_connector_env(&env));
        env.insert("KUBECONFIG".to_string(), "/tmp/kube".to_string());
        assert!(is_ocp_connector_env(&env));
        env.clear();
        env.insert("OPENSHIFT_MODE".to_string(), "in_cluster".to_string());
        assert!(is_ocp_connector_env(&env));
    }

    #[test]
    fn credentials_file_round_trips_and_is_mode_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_credentials_file(dir.path(), Some("s3cret"), Some(b"ca")).unwrap();
        let loaded = read_credentials_file(&path).unwrap();
        assert_eq!(loaded.token.as_deref(), Some("s3cret"));
        assert!(loaded.ca_data.is_some(), "CA data should be stored");
        // CA is stored base64-encoded; raw bytes must not appear unencoded.
        let raw = fs::read_to_string(&path).unwrap();
        // "ca" raw bytes should NOT appear verbatim (it is base64-encoded to "Y2E=").
        assert!(
            !raw.contains("\"ca\""),
            "CA PEM should be base64-encoded, not raw"
        );
        // The file must have mode 0600.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "credentials file must be mode 0600");
        }
    }

    #[test]
    fn missing_credentials_file_reads_as_empty() {
        let loaded = read_credentials_file(Path::new("/nonexistent/credentials.json")).unwrap();
        assert!(loaded.token.is_none());
    }

    #[test]
    fn spec_draft_and_last_known_good_constants_are_stable() {
        use config::{CONFIG_SCHEMA_VERSION, DRAFT_FILE, LAST_KNOWN_GOOD_FILE};
        assert_eq!(CONFIG_SCHEMA_VERSION, 1);
        assert_eq!(DRAFT_FILE, "draft.json");
        assert_eq!(LAST_KNOWN_GOOD_FILE, "last-known-good.json");
    }

    #[test]
    fn save_draft_updates_status_and_environment_without_leaking_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let input = OcpConfigInput {
            mode: "api_server_token".to_string(),
            kubeconfig_path: None,
            kubeconfig_context: None,
            server: Some("https://api.cluster.example:6443".to_string()),
            default_namespace: Some("prod".to_string()),
            display_name: Some("Production".to_string()),
            environment_label: Some("production".to_string()),
            token: Some("s3cr3t-token".to_string()),
            ca_pem_base64: Some(base64::engine::general_purpose::STANDARD.encode("ca-bytes")),
            server_id: "mcp-ocp".to_string(),
            timeout_policy: None,
        };
        let spec = save_draft(&input, dir.path()).unwrap();
        assert!(spec.token_set);
        assert!(spec.ca_data_set);
        assert!(!spec.preflight_passed);

        let secrets = ocp_secret_status(dir.path());
        assert!(secrets.token_set);
        assert!(secrets.ca_data_set);

        let status = connector_status(dir.path().to_path_buf());
        let draft = status.config.expect("draft should be persisted");
        assert_eq!(draft.display_name.as_deref(), Some("Production"));

        let env = ocp_environment(&spec, Some("s3cr3t-token"), Some("c2EtYnl0ZXM="));
        assert_eq!(
            env.get("OPENSHIFT_SERVER").map(String::as_str),
            Some("https://api.cluster.example:6443")
        );
        assert_eq!(
            env.get("OPENSHIFT_TOKEN").map(String::as_str),
            Some("s3cr3t-token")
        );
        assert_eq!(
            env.get("OPENSHIFT_DEFAULT_NAMESPACE").map(String::as_str),
            Some("prod")
        );
        assert!(is_ocp_connector_env(&env));
    }

    #[test]
    fn save_draft_roundtrips_custom_timeout_policy() {
        let dir = tempfile::tempdir().unwrap();
        let input = OcpConfigInput {
            mode: "api_server_token".to_string(),
            kubeconfig_path: None,
            kubeconfig_context: None,
            server: Some("https://api.cluster.example:6443".to_string()),
            default_namespace: None,
            display_name: None,
            environment_label: None,
            token: Some("tok".to_string()),
            ca_pem_base64: None,
            server_id: "mcp-ocp".to_string(),
            timeout_policy: Some(OcpTimeoutPolicy {
                connect_secs: Some(5),
                request_secs: Some(20),
                preflight_secs: Some(45),
            }),
        };
        let spec = save_draft(&input, dir.path()).unwrap();
        assert_eq!(spec.timeout_policy.connect_secs, Some(5));
        assert_eq!(spec.timeout_policy.request_secs, Some(20));
        assert_eq!(spec.timeout_policy.preflight_secs, Some(45));

        let status = connector_status(dir.path().to_path_buf());
        let draft = status.config.expect("draft should be persisted");
        assert_eq!(draft.timeout_policy.connect_secs, Some(5));
        assert_eq!(draft.timeout_policy.request_secs, Some(20));
        assert_eq!(draft.timeout_policy.preflight_secs, Some(45));
    }

    #[test]
    fn save_draft_defaults_timeout_policy_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let input = OcpConfigInput {
            mode: "api_server_token".to_string(),
            kubeconfig_path: None,
            kubeconfig_context: None,
            server: Some("https://api.cluster.example:6443".to_string()),
            default_namespace: None,
            display_name: None,
            environment_label: None,
            token: Some("tok".to_string()),
            ca_pem_base64: None,
            server_id: "mcp-ocp".to_string(),
            timeout_policy: None,
        };
        let spec = save_draft(&input, dir.path()).unwrap();
        assert_eq!(spec.timeout_policy.connect_secs, Some(10));
        assert_eq!(spec.timeout_policy.request_secs, Some(30));
        assert_eq!(spec.timeout_policy.preflight_secs, Some(60));
    }

    #[test]
    fn build_ocp_spec_rejects_zero_timeouts() {
        let err = build_ocp_spec(
            "api_server_token",
            None,
            None,
            Some("https://api.cluster.example:6443"),
            None,
            None,
            None,
            true,
            false,
            Some(&OcpTimeoutPolicy {
                connect_secs: Some(0),
                request_secs: Some(30),
                preflight_secs: Some(60),
            }),
        )
        .unwrap_err();
        assert_eq!(err.code, "config_timeout_zero");
    }

    #[test]
    fn connector_status_reads_persisted_breaker_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let breaker = CircuitBreaker::new(1, std::time::Duration::from_secs(60));
        breaker.record_failure();
        persist_breaker_state(dir.path(), &breaker);

        let status = connector_status(dir.path().to_path_buf());
        assert_eq!(status.breaker_state, "open");
    }

    #[test]
    fn connector_status_falls_back_to_closed_without_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let status = connector_status(dir.path().to_path_buf());
        assert_eq!(status.breaker_state, "closed");
    }

    #[test]
    fn rotate_credentials_resets_preflight_and_keeps_non_secret_fields() {
        let dir = tempfile::tempdir().unwrap();
        let input = OcpConfigInput {
            mode: "kubeconfig".to_string(),
            kubeconfig_path: Some("/home/user/.kube/config".to_string()),
            kubeconfig_context: None,
            server: None,
            default_namespace: None,
            display_name: Some("Staging".to_string()),
            environment_label: Some("staging".to_string()),
            token: None,
            ca_pem_base64: None,
            server_id: "mcp-ocp".to_string(),
            timeout_policy: None,
        };
        let saved = save_draft(&input, dir.path()).unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        let mut promoted = saved.clone();
        promoted.preflight_passed = true;
        store.save_draft(&promoted.sealed()).unwrap();

        let rotated = rotate_credentials(dir.path(), Some("rotated-token"), None).unwrap();
        assert!(
            !rotated.preflight_passed,
            "rotation must reset preflight state"
        );
        assert_eq!(rotated.display_name.as_deref(), Some("Staging"));
        assert_eq!(
            rotated.kubeconfig_path.as_deref(),
            Some("/home/user/.kube/config")
        );
        assert!(store.load_last_known_good().unwrap().is_none());
    }

    #[test]
    fn delete_connector_removes_all_connector_files() {
        let dir = tempfile::tempdir().unwrap();
        let input = OcpConfigInput {
            mode: "api_server_token".to_string(),
            kubeconfig_path: None,
            kubeconfig_context: None,
            server: Some("https://api.cluster.example:6443".to_string()),
            default_namespace: None,
            display_name: None,
            environment_label: None,
            token: Some("s3cr3t".to_string()),
            ca_pem_base64: None,
            server_id: "mcp-ocp".to_string(),
            timeout_policy: None,
        };
        save_draft(&input, dir.path()).unwrap();
        assert!(dir.path().join("draft.json").exists());
        assert!(dir.path().join("credentials.json").exists());

        delete_connector(dir.path()).unwrap();
        assert!(!dir.path().join("draft.json").exists());
        assert!(!dir.path().join("last-known-good.json").exists());
        assert!(!dir.path().join("credentials.json").exists());
        assert!(!dir.path().join("audit.ndjson").exists());
    }

    // ── MCP protocol server (serve_stdio) ────────────────────────────────────

    /// Feed raw newline-delimited JSON-RPC requests through the stdio server and
    /// return the parsed responses. Never touches the network: the client is
    /// pointed at a loopback address and only protocol-only paths are exercised.
    fn serve_protocol(input: &str) -> Vec<Value> {
        let cluster =
            build_api_server_config("https://127.0.0.1:1", "test-token", None, None).unwrap();
        let client = OcpClient::build(&cluster, OcpTimeouts::default()).unwrap();
        let breaker = CircuitBreaker::new(
            BREAKER_THRESHOLD,
            std::time::Duration::from_secs(BREAKER_RESET_AFTER_SECS),
        );
        let audit_dir = tempfile::tempdir().unwrap();
        let audit = AuditLog::new(audit_dir.path().to_path_buf());
        let mut output = Vec::new();
        serve_stdio(
            client,
            &breaker,
            &audit,
            vec![],
            None,
            None,
            &AtomicBool::new(false),
            std::io::Cursor::new(input.as_bytes()),
            &mut output,
        )
        .unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn mcp_protocol_initialize_responds_with_capabilities() {
        let responses = serve_protocol(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
        );
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], json!(1));
        assert_eq!(
            responses[0]["result"]["protocolVersion"],
            json!("2024-11-05")
        );
        assert_eq!(
            responses[0]["result"]["serverInfo"]["name"],
            json!("spacesly-ocp")
        );
        assert_eq!(
            responses[0]["result"]["capabilities"]["tools"]["listChanged"],
            json!(false)
        );
    }

    #[test]
    fn mcp_protocol_initialized_notification_produces_no_response() {
        let responses =
            serve_protocol("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n");
        assert!(responses.is_empty());
    }

    #[test]
    fn mcp_protocol_ping_responds_with_empty_result() {
        let responses = serve_protocol("{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n");
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], json!(7));
        assert_eq!(responses[0]["result"], json!({}));
    }

    #[test]
    fn mcp_protocol_tools_list_exposes_all_ocp_tools() {
        let responses =
            serve_protocol("{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n");
        assert_eq!(responses.len(), 1);
        let tools = responses[0]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), OcpTool::all().len());
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        for expected in OcpTool::all().iter().map(|tool| tool.as_str()) {
            assert!(names.contains(&expected), "missing tool '{expected}'");
        }
    }

    #[test]
    fn mcp_protocol_unknown_tool_returns_jsonrpc_error() {
        let responses = serve_protocol(
            "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":{\"name\":\"ocp_frobnicate\",\"arguments\":{}}}\n",
        );
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], json!(5));
        assert_eq!(responses[0]["error"]["code"], json!(-32000));
        assert!(responses[0]["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("Unknown OCP tool"));
    }

    #[test]
    fn mcp_protocol_mutation_requires_explicit_approval() {
        let responses = serve_protocol(
            "{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"tools/call\",\"params\":{\"name\":\"ocp_restart_deployment\",\"arguments\":{\"namespace\":\"default\",\"name\":\"api\"}}}\n",
        );
        let content = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        let approval: Value = serde_json::from_str(content).unwrap();
        assert_eq!(approval["status"], json!("approval_required"));
        assert_eq!(approval["operation"], json!("ocp_restart_deployment"));
        assert_eq!(
            approval["message"]
                .as_str()
                .unwrap()
                .matches("retry")
                .count(),
            1
        );

        let responses = serve_protocol(
            "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{\"name\":\"kubernetes_resources_delete\",\"arguments\":{\"api_version\":\"v1\",\"kind\":\"ConfigMap\",\"namespace\":\"default\",\"name\":\"temporary\"}}}\n",
        );
        let content = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        let approval: Value = serde_json::from_str(content).unwrap();
        assert_eq!(approval["status"], json!("approval_required"));
        assert_eq!(approval["operation"], json!("kubernetes_resources_delete"));
    }

    #[test]
    fn execution_contract_requires_structured_ui_approval() {
        assert!(contract_approved_mutation(&json!({
            "runtime_inputs": { "operator_notes": "approved" }
        }))
        .is_none());
        assert!(contract_approved_mutation(&json!({
            "runtime_inputs": { "approvals": [{
                "operation": "jira_transition_issue",
                "arguments_digest": "sha256:not-ocp",
                "decision": "approved"
            }] }
        }))
        .is_none());
        assert_eq!(
            contract_approved_mutation(&json!({
                "runtime_inputs": { "approvals": [{
                    "operation": "ocp_restart_deployment",
                    "arguments_digest": "abc123",
                    "decision": "approved",
                    "decided_at": "2026-08-08T00:00:00Z",
                    "contract_version": 2
                }] }
            , "version": 2 })),
            Some(ApprovedMutation {
                operation: "ocp_restart_deployment".to_string(),
                arguments_digest: "abc123".to_string(),
            })
        );
        assert_eq!(
            contract_approved_mutation(&json!({
                "runtime_inputs": { "approvals": [{
                    "operation": "spacesly-mcp-test_kubernetes_resources_create",
                    "arguments_digest": "ghi789",
                    "decision": "approved",
                    "contract_version": 4
                }] },
                "version": 4
            })),
            Some(ApprovedMutation {
                operation: "kubernetes_resources_create".to_string(),
                arguments_digest: "ghi789".to_string(),
            })
        );
        assert_eq!(
            contract_approved_mutation(&json!({
                "runtime_inputs": { "approvals": [{
                    "operation": "kubernetes_resources_patch",
                    "arguments_digest": "def456",
                    "decision": "approved",
                    "contract_version": 2
                }] },
                "version": 2
            })),
            Some(ApprovedMutation {
                operation: "kubernetes_resources_patch".to_string(),
                arguments_digest: "def456".to_string(),
            })
        );
        assert!(contract_approved_mutation(&json!({
            "version": 3,
            "runtime_inputs": { "approvals": [{
                "operation": "ocp_restart_deployment",
                "arguments_digest": "abc123",
                "decision": "approved",
                "contract_version": 2
            }] }
        }))
        .is_none());
    }

    #[test]
    fn mcp_protocol_unknown_method_returns_jsonrpc_error() {
        let responses =
            serve_protocol("{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/unknown\"}\n");
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], json!(9));
        assert_eq!(responses[0]["error"]["code"], json!(-32000));
        assert_eq!(
            responses[0]["error"]["message"].as_str().unwrap(),
            "MCP method not found."
        );
    }

    #[test]
    fn mcp_protocol_multiple_requests_are_answered_in_order() {
        let responses = serve_protocol(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\
             {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n\
             {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n",
        );
        let ids: Vec<i64> = responses
            .iter()
            .map(|response| response["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn resolve_cluster_api_server_mode_reads_credentials_file() {
        let dir = tempfile::tempdir().unwrap();
        write_credentials_file(dir.path(), Some("s3cr3t-token"), Some(b"ca-bytes")).unwrap();
        let env = OcpConnectorEnv {
            mode: OcpAuthMode::ApiServerToken,
            kubeconfig: None,
            context: None,
            server: Some("https://api.cluster.example:6443".to_string()),
            default_namespace: Some("prod".to_string()),
            credentials_file: Some(dir.path().join("credentials.json")),
            connector_dir: None,
            approved_operation: None,
            approved_arguments_digest: None,
        };
        let cluster = resolve_cluster(&env).unwrap();
        assert_eq!(cluster.server, "https://api.cluster.example:6443");
        assert_eq!(
            cluster.credentials.bearer_token.as_deref(),
            Some("s3cr3t-token")
        );
        assert_eq!(cluster.default_namespace.as_deref(), Some("prod"));
    }
}
