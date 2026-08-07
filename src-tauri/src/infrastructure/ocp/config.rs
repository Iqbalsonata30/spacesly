use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use yaml_rust2::YamlLoader;

use super::errors::{OcpError, OcpResult};

pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const DRAFT_FILE: &str = "draft.json";
pub const LAST_KNOWN_GOOD_FILE: &str = "last-known-good.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcpAuthMode {
    Kubeconfig,
    ApiServerToken,
    InCluster,
}

impl OcpAuthMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kubeconfig => "kubeconfig",
            Self::ApiServerToken => "api_server_token",
            Self::InCluster => "in_cluster",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "kubeconfig" => Some(Self::Kubeconfig),
            "api_server_token" => Some(Self::ApiServerToken),
            "in_cluster" => Some(Self::InCluster),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClientCert {
    pub certificate: Vec<u8>,
    pub key: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum CaMaterial {
    Data(Vec<u8>),
    File(PathBuf),
}

#[derive(Clone, Debug, Default)]
pub struct ClientCredentials {
    pub bearer_token: Option<String>,
    pub client_cert: Option<ClientCert>,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ClientCredentials {
    pub fn bearer_token(mut self, token: String) -> Self {
        self.bearer_token = Some(token);
        self
    }

    pub fn is_anonymous(&self) -> bool {
        self.bearer_token.is_none()
            && self.client_cert.is_none()
            && self.username.is_none()
            && self.password.is_none()
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedCluster {
    pub server: String,
    pub ca: Option<CaMaterial>,
    pub insecure_skip_tls_verify: bool,
    pub credentials: ClientCredentials,
    pub default_namespace: Option<String>,
}

impl ResolvedCluster {
    pub fn secret_snapshot(&self) -> Vec<String> {
        let mut secrets = Vec::new();
        if let Some(token) = &self.credentials.bearer_token {
            if token.len() >= 4 {
                secrets.push(token.clone());
            }
        }
        if let Some(password) = &self.credentials.password {
            if password.len() >= 4 {
                secrets.push(password.clone());
            }
        }
        secrets
    }
}

pub fn parse_kubeconfig(path: &Path, context: Option<&str>) -> OcpResult<ResolvedCluster> {
    let path = expand_home_path(path, user_home_dir().as_deref());
    let path = path.as_path();
    if !path.exists() {
        return Err(OcpError::config(
            "kubeconfig_missing",
            format!("Kubeconfig '{}' does not exist.", path.display()),
        ));
    }
    let content = fs::read_to_string(path).map_err(|error| {
        OcpError::config(
            "kubeconfig_unreadable",
            format!("Failed to read kubeconfig '{}': {error}", path.display()),
        )
    })?;
    parse_kubeconfig_content(path, &content, context)
}

fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn expand_home_path(path: &Path, home: Option<&Path>) -> PathBuf {
    let Some(path_text) = path.to_str() else {
        return path.to_path_buf();
    };
    if path_text == "~" {
        return home
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf());
    }
    if let Some(relative) = path_text
        .strip_prefix("~/")
        .or_else(|| path_text.strip_prefix("~\\"))
    {
        if let Some(home) = home {
            return home.join(relative);
        }
    }
    path.to_path_buf()
}

fn parse_kubeconfig_content(
    path: &Path,
    content: &str,
    context: Option<&str>,
) -> OcpResult<ResolvedCluster> {
    let docs = YamlLoader::load_from_str(content).map_err(|error| {
        OcpError::config(
            "kubeconfig_yaml",
            format!("Kubeconfig '{}' is not valid YAML: {error}", path.display()),
        )
    })?;
    let doc = docs.into_iter().next().ok_or_else(|| {
        OcpError::config(
            "kubeconfig_empty",
            format!("Kubeconfig '{}' is empty.", path.display()),
        )
    })?;

    let context_name = match context {
        Some(explicit) if !explicit.trim().is_empty() => explicit.trim().to_string(),
        _ => doc["current-context"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                OcpError::config(
                    "kubeconfig_no_context",
                    format!(
                        "Kubeconfig '{}' has no current-context and no context was requested.",
                        path.display()
                    ),
                )
            })?,
    };

    let clusters = named_entries(&doc, "clusters", "cluster");
    let contexts = named_entries(&doc, "contexts", "context");
    let users = named_entries(&doc, "users", "user");

    let context_node = contexts.get(&context_name).cloned().ok_or_else(|| {
        OcpError::config(
            "kubeconfig_context_missing",
            format!("Kubeconfig context '{context_name}' was not found."),
        )
    })?;

    let cluster_name = context_node["cluster"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| {
            OcpError::config(
                "kubeconfig_bad_context",
                format!("Kubeconfig context '{context_name}' has no cluster."),
            )
        })?;
    let user_name = context_node["user"].as_str().map(str::to_string);
    let default_namespace = context_node["namespace"].as_str().map(str::to_string);

    let cluster_node = clusters.get(&cluster_name).ok_or_else(|| {
        OcpError::config(
            "kubeconfig_cluster_missing",
            format!("Kubeconfig cluster '{cluster_name}' was not found."),
        )
    })?;
    let server = cluster_node["server"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            OcpError::config(
                "kubeconfig_no_server",
                format!("Kubeconfig cluster '{cluster_name}' has no server URL."),
            )
        })?;
    validate_server_url(server)?;

    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let (ca, insecure_skip_tls_verify) = resolve_ca(cluster_node, base_dir)?;

    let credentials = match user_name.as_deref() {
        Some(user) => {
            let user_node = users.get(user).cloned().unwrap_or(yaml_rust2::Yaml::Null);
            resolve_user_credentials(&user_node, base_dir)?
        }
        None => ClientCredentials::default(),
    };

    Ok(ResolvedCluster {
        server: server.to_string(),
        ca,
        insecure_skip_tls_verify,
        credentials,
        default_namespace,
    })
}

fn resolve_ca(
    cluster_node: &yaml_rust2::Yaml,
    base_dir: &Path,
) -> OcpResult<(Option<CaMaterial>, bool)> {
    let insecure = cluster_node["insecure-skip-tls-verify"]
        .as_bool()
        .unwrap_or(false);
    if let Some(data) = cluster_node["certificate-authority-data"].as_str() {
        let decoded =
            decode_base64(data).map_err(|error| OcpError::config("kubeconfig_ca_data", error))?;
        if !decoded.is_empty() {
            return Ok((Some(CaMaterial::Data(decoded)), insecure));
        }
    }
    if let Some(file) = cluster_node["certificate-authority"].as_str() {
        if !file.trim().is_empty() {
            return Ok((
                Some(CaMaterial::File(resolve_kube_path(base_dir, file))),
                insecure,
            ));
        }
    }
    Ok((None, insecure))
}

fn resolve_user_credentials(
    user_node: &yaml_rust2::Yaml,
    base_dir: &Path,
) -> OcpResult<ClientCredentials> {
    if let Some(token) = user_node["token"].as_str() {
        if !token.trim().is_empty() {
            return Ok(ClientCredentials::default().bearer_token(token.to_string()));
        }
    }
    if let Some(token_file) = user_node["tokenFile"].as_str() {
        if !token_file.trim().is_empty() {
            let path = resolve_kube_path(base_dir, token_file);
            let token = fs::read_to_string(&path).map_err(|error| {
                OcpError::config(
                    "kubeconfig_token_file",
                    format!("Failed to read token file '{}': {error}", path.display()),
                )
            })?;
            return Ok(ClientCredentials::default().bearer_token(token.trim().to_string()));
        }
    }

    let certificate_data = user_node["client-certificate-data"].as_str();
    let key_data = user_node["client-key-data"].as_str();
    let certificate_file = user_node["client-certificate"].as_str();
    let key_file = user_node["client-key"].as_str();
    if certificate_data.is_some()
        || key_data.is_some()
        || certificate_file.is_some()
        || key_file.is_some()
    {
        let certificate = match certificate_data {
            Some(data) => decode_base64(data)
                .map_err(|error| OcpError::config("kubeconfig_cert_data", error))?,
            None => {
                let file = certificate_file.ok_or_else(|| {
                    OcpError::config("kubeconfig_cert_missing", "Client certificate is missing.")
                })?;
                read_file_bytes(&resolve_kube_path(base_dir, file))
            }
        };
        let key = match key_data {
            Some(data) => decode_base64(data)
                .map_err(|error| OcpError::config("kubeconfig_key_data", error))?,
            None => {
                let file = key_file.ok_or_else(|| {
                    OcpError::config("kubeconfig_key_missing", "Client key is missing.")
                })?;
                read_file_bytes(&resolve_kube_path(base_dir, file))
            }
        };
        return Ok(ClientCredentials {
            client_cert: Some(ClientCert { certificate, key }),
            ..ClientCredentials::default()
        });
    }

    let username = user_node["username"].as_str().map(str::to_string);
    let password = user_node["password"].as_str().map(str::to_string);
    if let (Some(username), Some(password)) = (&username, &password) {
        if !username.trim().is_empty() && !password.trim().is_empty() {
            return Ok(ClientCredentials {
                username: Some(username.clone()),
                password: Some(password.clone()),
                ..ClientCredentials::default()
            });
        }
    }

    if let Some(exec_node) = user_node["exec"].as_hash() {
        let command = exec_node
            .get(&yaml_rust2::Yaml::from_str("command"))
            .and_then(yaml_rust2::Yaml::as_str)
            .unwrap_or("unknown");
        return Err(OcpError::config(
            "kubeconfig_exec_unsupported",
            format!(
                "Kubeconfig user uses an exec auth plugin ('{command}'). For now, use a static token \
                 in the kubeconfig or configure Mode B (API server + token)."
            ),
        ));
    }

    Ok(ClientCredentials::default())
}

pub fn build_api_server_config(
    server: &str,
    token: &str,
    ca_data: Option<&[u8]>,
    default_namespace: Option<&str>,
) -> OcpResult<ResolvedCluster> {
    let server = server.trim();
    validate_server_url(server)?;
    let token = token.trim();
    if token.is_empty() {
        return Err(OcpError::config(
            "token_missing",
            "API server token is required.",
        ));
    }
    Ok(ResolvedCluster {
        server: server.to_string(),
        ca: ca_data
            .filter(|data| !data.is_empty())
            .map(|data| CaMaterial::Data(data.to_vec())),
        insecure_skip_tls_verify: false,
        credentials: ClientCredentials::default().bearer_token(token.to_string()),
        default_namespace: default_namespace.map(str::to_string),
    })
}

pub fn build_in_cluster_config() -> OcpResult<ResolvedCluster> {
    let host = std::env::var("KUBERNETES_SERVICE_HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            OcpError::config(
                "in_cluster_host_missing",
                "KUBERNETES_SERVICE_HOST is not set; in-cluster mode is only available inside a pod.",
            )
        })?;
    let port = std::env::var("KUBERNETES_SERVICE_PORT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "443".to_string());
    let token_path = Path::new("/var/run/secrets/kubernetes.io/serviceaccount/token");
    let ca_path = Path::new("/var/run/secrets/kubernetes.io/serviceaccount/ca.crt");
    if !token_path.exists() {
        return Err(OcpError::config(
            "in_cluster_token_missing",
            "Service account token was not found at the standard in-cluster path.",
        ));
    }
    let token = fs::read_to_string(token_path).map_err(|error| {
        OcpError::config(
            "in_cluster_token_read",
            format!("Failed to read the in-cluster service account token: {error}"),
        )
    })?;
    let ca = if ca_path.exists() {
        Some(CaMaterial::Data(read_file_bytes(ca_path)))
    } else {
        None
    };
    Ok(ResolvedCluster {
        server: format!("https://{host}:{port}"),
        ca,
        insecure_skip_tls_verify: false,
        credentials: ClientCredentials::default().bearer_token(token.trim().to_string()),
        default_namespace: None,
    })
}

fn validate_server_url(server: &str) -> OcpResult<()> {
    let parsed = reqwest::Url::parse(server)
        .map_err(|_| OcpError::config("server_url", "Cluster server URL is invalid."))?;
    if parsed.scheme() != "https" {
        return Err(OcpError::config(
            "server_url",
            "Cluster server URL must use HTTPS.",
        ));
    }
    if parsed.host_str().is_none() {
        return Err(OcpError::config(
            "server_url",
            "Cluster server URL has no host.",
        ));
    }
    Ok(())
}

fn decode_base64(data: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|error| format!("Invalid base64-encoded material: {error}"))
}

fn read_file_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_default()
}

fn resolve_kube_path(base_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn named_entries(
    doc: &yaml_rust2::Yaml,
    list_key: &str,
    item_key: &str,
) -> HashMap<String, yaml_rust2::Yaml> {
    let mut entries = HashMap::new();
    if let Some(items) = doc[list_key].as_vec() {
        for item in items {
            if let Some(name) = item["name"].as_str() {
                entries.insert(name.to_string(), item[item_key].clone());
            }
        }
    }
    entries
}

/// Timeout policy (seconds per stage). Kept as optional so the spec remains valid
/// when fields are absent — callers fall back to `OcpTimeoutPolicy::default()`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OcpTimeoutPolicy {
    #[serde(default)]
    pub connect_secs: Option<u32>,
    #[serde(default)]
    pub request_secs: Option<u32>,
    #[serde(default)]
    pub preflight_secs: Option<u32>,
}

impl Default for OcpTimeoutPolicy {
    fn default() -> Self {
        Self {
            connect_secs: Some(10),
            request_secs: Some(30),
            preflight_secs: Some(60),
        }
    }
}

impl OcpTimeoutPolicy {
    pub fn connect_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.connect_secs.unwrap_or(10) as u64)
    }

    pub fn request_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.request_secs.unwrap_or(30) as u64)
    }

    pub fn preflight_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.preflight_secs.unwrap_or(60) as u64)
    }

    /// Reject obviously invalid (zero) timeouts so a misconfigured draft fails
    /// fast instead of making the connector unresponsive.
    pub fn validate(self) -> OcpResult<Self> {
        let connect = self.connect_secs.unwrap_or(10);
        let request = self.request_secs.unwrap_or(30);
        let preflight = self.preflight_secs.unwrap_or(60);
        if connect == 0 || request == 0 || preflight == 0 {
            return Err(OcpError::config(
                "config_timeout_zero",
                "Timeout values must be greater than zero seconds.",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct OcpConfigSpec {
    pub version: u32,
    pub mode: String,
    // ── Mode A: kubeconfig ──────────────────────────────────────────────────
    pub kubeconfig_path: Option<String>,
    pub kubeconfig_context: Option<String>,
    // ── Mode B: API server + token ──────────────────────────────────────────
    pub server: Option<String>,
    /// True when CA PEM material has been persisted to the credentials file.
    /// Never contains the actual CA bytes; those live in credentials.json.
    pub ca_data_set: bool,
    /// True when a bearer token has been persisted.
    pub token_set: bool,
    // ── Shared ──────────────────────────────────────────────────────────────
    pub default_namespace: Option<String>,
    /// Human-readable label, e.g. "Production OpenShift".
    #[serde(default)]
    pub display_name: Option<String>,
    /// Environment label for UI display.
    #[serde(default)]
    pub environment_label: Option<String>,
    // ── Timeouts ────────────────────────────────────────────────────────────
    #[serde(default)]
    pub timeout_policy: OcpTimeoutPolicy,
    // ── State ───────────────────────────────────────────────────────────────
    pub preflight_passed: bool,
    pub updated_at_ms: u64,
    pub checksum: String,
}

impl OcpConfigSpec {
    pub fn compute_checksum(&self) -> String {
        let canonical = format!(
            "version={};mode={};kubeconfig_path={};kubeconfig_context={};server={};\
             ca_data_set={};token_set={};default_namespace={};display_name={}",
            self.version,
            self.mode,
            self.kubeconfig_path.as_deref().unwrap_or(""),
            self.kubeconfig_context.as_deref().unwrap_or(""),
            self.server.as_deref().unwrap_or(""),
            self.ca_data_set,
            self.token_set,
            self.default_namespace.as_deref().unwrap_or(""),
            self.display_name.as_deref().unwrap_or(""),
        );
        let digest = Sha256::digest(canonical.as_bytes());
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    pub fn validate(&self) -> OcpResult<()> {
        if self.version != CONFIG_SCHEMA_VERSION {
            return Err(OcpError::config(
                "config_version",
                format!(
                    "OCP connector config version {} is unsupported.",
                    self.version
                ),
            ));
        }
        let mode = OcpAuthMode::parse(&self.mode).ok_or_else(|| {
            OcpError::config(
                "config_mode",
                format!("Unknown OCP auth mode '{}'.", self.mode),
            )
        })?;
        match mode {
            OcpAuthMode::Kubeconfig => {
                if self
                    .kubeconfig_path
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("")
                    .is_empty()
                {
                    return Err(OcpError::config(
                        "config_incomplete",
                        "Kubeconfig path is required for kubeconfig mode.",
                    ));
                }
            }
            OcpAuthMode::ApiServerToken => {
                if self
                    .server
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("")
                    .is_empty()
                {
                    return Err(OcpError::config(
                        "config_incomplete",
                        "API server URL is required for API server token mode.",
                    ));
                }
                if !self.token_set {
                    return Err(OcpError::config(
                        "config_incomplete",
                        "API server token is required for API server token mode.",
                    ));
                }
            }
            OcpAuthMode::InCluster => {}
        }
        if self.checksum != self.compute_checksum() {
            return Err(OcpError::config(
                "config_checksum",
                "OCP connector draft config checksum does not match its content.",
            ));
        }
        Ok(())
    }

    /// Convenience builder: seal the spec by computing and storing the checksum.
    pub fn sealed(mut self) -> Self {
        self.checksum = self.compute_checksum();
        self
    }
}

pub struct ConfigStore {
    dir: PathBuf,
}

impl ConfigStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn load_draft(&self) -> OcpResult<Option<OcpConfigSpec>> {
        self.read_spec(DRAFT_FILE)
    }

    pub fn load_last_known_good(&self) -> OcpResult<Option<OcpConfigSpec>> {
        self.read_spec(LAST_KNOWN_GOOD_FILE)
    }

    pub fn save_draft(&self, spec: &OcpConfigSpec) -> OcpResult<()> {
        self.write_spec(DRAFT_FILE, spec)
    }

    pub fn save_last_known_good(&self, spec: &OcpConfigSpec) -> OcpResult<()> {
        self.write_spec(LAST_KNOWN_GOOD_FILE, spec)
    }

    pub fn promote_draft_to_last_known_good(&self) -> OcpResult<()> {
        let Some(spec) = self.load_draft()? else {
            return Err(OcpError::config(
                "config_no_draft",
                "Cannot promote an OCP config that has no draft.",
            ));
        };
        let mut promoted = spec;
        promoted.preflight_passed = true;
        promoted.updated_at_ms = now_millis();
        self.save_last_known_good(&promoted)
    }

    fn read_spec(&self, name: &str) -> OcpResult<Option<OcpConfigSpec>> {
        let path = self.dir.join(name);
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path).map_err(|error| {
            OcpError::config(
                "config_read",
                format!(
                    "Failed to read OCP connector config '{}': {error}",
                    path.display()
                ),
            )
        })?;
        let spec: OcpConfigSpec = serde_json::from_str(&raw).map_err(|error| {
            OcpError::config(
                "config_parse",
                format!(
                    "Failed to parse OCP connector config '{}': {error}",
                    path.display()
                ),
            )
        })?;
        spec.validate()?;
        Ok(Some(spec))
    }

    fn write_spec(&self, name: &str, spec: &OcpConfigSpec) -> OcpResult<()> {
        if let Some(parent) = self.dir.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                OcpError::config(
                    "config_dir",
                    format!("Failed to create OCP connector directory: {error}"),
                )
            })?;
        }
        fs::create_dir_all(&self.dir).map_err(|error| {
            OcpError::config(
                "config_dir",
                format!("Failed to create OCP connector directory: {error}"),
            )
        })?;
        set_private_dir_permissions(&self.dir)?;
        let payload = serde_json::to_string_pretty(spec).map_err(|error| {
            OcpError::config(
                "config_encode",
                format!("Failed to encode OCP connector config: {error}"),
            )
        })?;
        let final_path = self.dir.join(name);
        let temp_path = self.dir.join(format!("{name}.tmp"));
        fs::write(&temp_path, payload).map_err(|error| {
            OcpError::config(
                "config_write",
                format!("Failed to write OCP connector config: {error}"),
            )
        })?;
        set_private_file_permissions(&temp_path)?;
        fs::rename(&temp_path, &final_path).map_err(|error| {
            OcpError::config(
                "config_write",
                format!("Failed to persist OCP connector config: {error}"),
            )
        })?;
        Ok(())
    }
}

pub fn default_connector_dir() -> OcpResult<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or_else(|| {
            OcpError::config(
                "config_dir",
                "Cannot resolve a data directory for the OCP connector.",
            )
        })?;
    Ok(base.join("spacesly").join("connectors").join("ocp"))
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> Result<(), OcpError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
        OcpError::config(
            "config_perms",
            format!("Failed to set directory permissions: {error}"),
        )
    })
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> Result<(), OcpError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), OcpError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        OcpError::config(
            "config_perms",
            format!("Failed to set file permissions: {error}"),
        )
    })
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<(), OcpError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const TOKEN_KUBECONFIG: &str = r#"
apiVersion: v1
kind: Config
current-context: prod
clusters:
- cluster:
    certificate-authority-data: "dGVzdC1jYQ=="
    server: https://api.cluster.example:6443
  name: prod-cluster
- cluster:
    certificate-authority-data: "dGVzdC1jYQ=="
    server: https://staging.cluster.example:6443
  name: staging-cluster
contexts:
- context:
    cluster: prod-cluster
    namespace: team-a
    user: bot
  name: prod
- context:
    cluster: staging-cluster
    namespace: team-b
    user: bot
  name: staging
users:
- name: bot
  user:
    token: secret-token-abc
"#;

    const CERT_KUBECONFIG: &str = r#"
apiVersion: v1
kind: Config
current-context: local
clusters:
- cluster:
    insecure-skip-tls-verify: true
    server: https://127.0.0.1:6443
  name: local-cluster
contexts:
- context:
    cluster: local-cluster
    user: dev
  name: local
users:
- name: dev
  user:
    client-certificate-data: "Y2VydA=="
    client-key-data: "a2V5"
"#;

    const EXEC_KUBECONFIG: &str = r#"
apiVersion: v1
kind: Config
current-context: exec
clusters:
- cluster:
    server: https://exec.example:6443
  name: exec-cluster
contexts:
- context:
    cluster: exec-cluster
    user: exec-user
  name: exec
users:
- name: exec-user
  user:
    exec:
      command: aws
      args: ["eks", "get-token"]
"#;

    fn write_config(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config");
        fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn parses_token_kubeconfig_with_current_context() {
        let (_dir, path) = write_config(TOKEN_KUBECONFIG);
        let cluster = parse_kubeconfig(&path, None).unwrap();
        assert_eq!(cluster.server, "https://api.cluster.example:6443");
        assert_eq!(
            cluster.credentials.bearer_token.as_deref(),
            Some("secret-token-abc")
        );
        assert_eq!(cluster.default_namespace.as_deref(), Some("team-a"));
        assert!(matches!(cluster.ca, Some(CaMaterial::Data(data)) if data == b"test-ca"));
        assert!(!cluster.insecure_skip_tls_verify);
    }

    #[test]
    fn expands_tilde_in_kubeconfig_path() {
        let home = Path::new("/home/developer");
        assert_eq!(
            expand_home_path(Path::new("~/.kube/config"), Some(home)),
            home.join(".kube/config")
        );
        assert_eq!(expand_home_path(Path::new("~"), Some(home)), home);
        assert_eq!(
            expand_home_path(Path::new("/etc/kubernetes/config"), Some(home)),
            PathBuf::from("/etc/kubernetes/config")
        );
    }

    #[test]
    fn explicit_context_overrides_current_context() {
        let (_dir, path) = write_config(TOKEN_KUBECONFIG);
        let cluster = parse_kubeconfig(&path, Some("staging")).unwrap();
        assert_eq!(cluster.server, "https://staging.cluster.example:6443");
        assert_eq!(cluster.default_namespace.as_deref(), Some("team-b"));
    }

    #[test]
    fn missing_context_is_reported() {
        let (_dir, path) = write_config(TOKEN_KUBECONFIG);
        let error = parse_kubeconfig(&path, Some("nope")).unwrap_err();
        assert!(error.message.contains("nope"));
        assert_eq!(error.code, "kubeconfig_context_missing");
    }

    #[test]
    fn parses_client_cert_and_insecure_flag() {
        let (_dir, path) = write_config(CERT_KUBECONFIG);
        let cluster = parse_kubeconfig(&path, None).unwrap();
        assert!(cluster.insecure_skip_tls_verify);
        let cert = cluster.credentials.client_cert.unwrap();
        assert_eq!(cert.certificate, b"cert");
        assert_eq!(cert.key, b"key");
    }

    #[test]
    fn exec_auth_plugin_is_rejected_with_guidance() {
        let (_dir, path) = write_config(EXEC_KUBECONFIG);
        let error = parse_kubeconfig(&path, None).unwrap_err();
        assert_eq!(error.code, "kubeconfig_exec_unsupported");
        assert!(error.message.contains("Mode B"));
    }

    #[test]
    fn non_https_server_is_rejected() {
        let cluster = build_api_server_config("http://insecure:80", "token", None, None);
        assert!(cluster.is_err());
        let cluster = build_api_server_config("https://ok:6443", "token", None, None);
        assert!(cluster.is_ok());
    }

    #[test]
    fn api_server_mode_requires_a_token() {
        assert!(build_api_server_config("https://ok:6443", "  ", None, None).is_err());
    }

    fn make_api_server_spec() -> OcpConfigSpec {
        OcpConfigSpec {
            version: CONFIG_SCHEMA_VERSION,
            mode: "api_server_token".to_string(),
            kubeconfig_path: None,
            kubeconfig_context: None,
            server: Some("https://api.example:6443".to_string()),
            ca_data_set: true,
            token_set: true,
            default_namespace: Some("team-a".to_string()),
            display_name: Some("Prod OpenShift".to_string()),
            environment_label: Some("production".to_string()),
            timeout_policy: OcpTimeoutPolicy::default(),
            preflight_passed: false,
            updated_at_ms: now_millis(),
            checksum: String::new(),
        }
    }

    fn make_kubeconfig_spec() -> OcpConfigSpec {
        OcpConfigSpec {
            version: CONFIG_SCHEMA_VERSION,
            mode: "kubeconfig".to_string(),
            kubeconfig_path: Some("/home/user/.kube/config".to_string()),
            kubeconfig_context: None,
            server: None,
            ca_data_set: false,
            token_set: false,
            default_namespace: None,
            display_name: None,
            environment_label: None,
            timeout_policy: OcpTimeoutPolicy::default(),
            preflight_passed: false,
            updated_at_ms: now_millis(),
            checksum: String::new(),
        }
    }

    #[test]
    fn spec_checksum_round_trips_and_validates() {
        let mut spec = make_api_server_spec();
        spec.checksum = spec.compute_checksum();
        assert!(spec.validate().is_ok());
        spec.server = Some("https://other.example:6443".to_string());
        assert!(spec.validate().is_err());
    }

    #[test]
    fn spec_sealed_builder_sets_checksum() {
        let spec = make_api_server_spec().sealed();
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn timeout_policy_defaults_are_bounded() {
        let policy = OcpTimeoutPolicy::default();
        assert!(policy.connect_duration().as_secs() <= 15);
        assert!(policy.request_duration().as_secs() <= 60);
        assert!(policy.preflight_duration().as_secs() <= 120);
    }

    #[test]
    fn store_persists_and_promotes_draft_to_last_known_good() {
        let dir = tempdir().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        assert!(store.load_draft().unwrap().is_none());
        assert!(store.load_last_known_good().unwrap().is_none());

        let spec = make_kubeconfig_spec().sealed();
        store.save_draft(&spec).unwrap();

        let loaded = store.load_draft().unwrap().unwrap();
        assert_eq!(
            loaded.kubeconfig_path.as_deref(),
            Some("/home/user/.kube/config")
        );

        store.promote_draft_to_last_known_good().unwrap();
        let lkg = store.load_last_known_good().unwrap().unwrap();
        assert!(lkg.preflight_passed);
    }

    #[test]
    fn in_cluster_mode_requires_cluster_environment() {
        std::env::remove_var("KUBERNETES_SERVICE_HOST");
        let error = build_in_cluster_config().unwrap_err();
        assert_eq!(error.code, "in_cluster_host_missing");
    }
}
