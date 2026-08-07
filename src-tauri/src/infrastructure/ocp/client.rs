use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Method;
use serde_json::{json, Value};

use super::config::{CaMaterial, OcpTimeoutPolicy, ResolvedCluster};
use super::errors::{
    handle_api_status, translate_reqwest_error, OcpError, OcpErrorKind, OcpResult,
};

/// Per-request timeout configuration for the OCP HTTP client.
///
/// - `connect` covers TCP dial + TLS handshake (set via `reqwest::connect_timeout`).
/// - `request` is the total time allowed for the entire request including response body.
#[derive(Clone, Copy, Debug)]
pub struct OcpTimeouts {
    pub connect: Duration,
    pub request: Duration,
}

impl Default for OcpTimeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            request: Duration::from_secs(30),
        }
    }
}

impl OcpTimeouts {
    /// Build timeouts from a stored `OcpTimeoutPolicy`, falling back to safe defaults
    /// when individual fields are absent.
    pub fn from_policy(policy: &OcpTimeoutPolicy) -> Self {
        Self {
            connect: policy.connect_duration(),
            request: policy.request_duration(),
        }
    }
}

/// Blocking HTTP client pre-configured for one OpenShift/Kubernetes cluster.
pub struct OcpClient {
    http: Client,
    server: String,
    default_namespace: String,
    discovery: Mutex<HashMap<String, CachedDiscovery>>,
}

const DISCOVERY_CACHE_TTL: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredResource {
    pub api_version: String,
    pub group: String,
    pub version: String,
    pub name: String,
    pub singular_name: Option<String>,
    pub kind: String,
    pub namespaced: bool,
    pub verbs: HashSet<String>,
    pub short_names: Vec<String>,
}

impl DiscoveredResource {
    pub fn qualified_name(&self) -> String {
        if self.group.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.name, self.group)
        }
    }

    pub fn supports(&self, verb: &str) -> bool {
        self.verbs.contains(verb)
    }
}

struct CachedDiscovery {
    loaded_at: Instant,
    resources: Vec<DiscoveredResource>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KubernetesPatchType {
    Merge,
    Json,
    ServerApply,
}

impl KubernetesPatchType {
    fn content_type(self) -> &'static str {
        match self {
            Self::Merge => "application/merge-patch+json",
            Self::Json => "application/json-patch+json",
            Self::ServerApply => "application/apply-patch+yaml",
        }
    }
}

impl OcpClient {
    pub fn build(cluster: &ResolvedCluster, timeouts: OcpTimeouts) -> OcpResult<Self> {
        let mut builder = Client::builder()
            .use_rustls_tls()
            .connect_timeout(timeouts.connect)
            .timeout(timeouts.request)
            .user_agent(concat!("spacesly-ocp/", env!("CARGO_PKG_VERSION")));

        if cluster.insecure_skip_tls_verify {
            builder = builder.danger_accept_invalid_certs(true);
        } else if let Some(ca) = &cluster.ca {
            let ca_bytes = match ca {
                CaMaterial::Data(data) => data.clone(),
                CaMaterial::File(path) => std::fs::read(path).map_err(|error| {
                    OcpError::config(
                        "ca_file",
                        format!(
                            "Failed to read cluster CA file '{}': {error}",
                            path.display()
                        ),
                    )
                })?,
            };
            let certificate = reqwest::Certificate::from_pem(&ca_bytes).map_err(|error| {
                OcpError::config(
                    "ca_invalid",
                    format!("Cluster CA material is not valid PEM: {error}"),
                )
            })?;
            builder = builder.add_root_certificate(certificate);
        }

        if let Some(cert) = &cluster.credentials.client_cert {
            let mut pem = Vec::with_capacity(cert.certificate.len() + cert.key.len() + 1);
            pem.extend_from_slice(&cert.certificate);
            if !pem.ends_with(b"\n") {
                pem.push(b'\n');
            }
            pem.extend_from_slice(&cert.key);
            let identity = reqwest::Identity::from_pem(&pem).map_err(|error| {
                OcpError::config(
                    "client_cert_invalid",
                    format!("Client certificate material is not valid PEM: {error}"),
                )
            })?;
            builder = builder.identity(identity);
        }

        if let Some(token) = cluster.credentials.bearer_token.as_deref() {
            if !token.trim().is_empty() {
                builder = builder.default_headers(bearer_headers(token));
            }
        } else if let (Some(username), Some(password)) = (
            cluster.credentials.username.as_deref(),
            cluster.credentials.password.as_deref(),
        ) {
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
            let mut headers = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_str(&format!("Basic {encoded}")) {
                headers.insert(AUTHORIZATION, value);
            }
            builder = builder.default_headers(headers);
        }

        let http = builder
            .build()
            .map_err(|error| OcpError::internal(format!("Failed to build HTTP client: {error}")))?;
        let default_namespace = cluster
            .default_namespace
            .clone()
            .unwrap_or_else(|| "default".to_string());
        Ok(Self {
            http,
            server: cluster.server.trim_end_matches('/').to_string(),
            default_namespace,
            discovery: Mutex::new(HashMap::new()),
        })
    }

    pub fn default_namespace(&self) -> &str {
        &self.default_namespace
    }

    // ── Core request helpers ─────────────────────────────────────────────────

    pub fn get_json(&self, path: &str, query: &[(&str, &str)]) -> OcpResult<Value> {
        let url = format!("{}{}", self.server, path);
        let mut request = self.http.get(&url);
        if !query.is_empty() {
            request = request.query(query);
        }
        let response = request.send().map_err(translate_reqwest_error)?;
        let status = response.status();
        let body = response.text().map_err(translate_reqwest_error)?;
        handle_api_status(status, &body)?;
        serde_json::from_str(&body).map_err(|error| {
            OcpError::api(
                "response_parse",
                format!("API server returned invalid JSON: {error}"),
            )
        })
    }

    pub fn get_list(
        &self,
        path: &str,
        query: &[(&str, &str)],
        allow_missing_kind: bool,
    ) -> OcpResult<Value> {
        match self.get_json(path, query) {
            Ok(value) => value.get("items").cloned().ok_or_else(|| {
                OcpError::api("list_shape", "API server list response had no items array.")
            }),
            Err(error)
                if allow_missing_kind
                    && matches!(error.kind, OcpErrorKind::Api | OcpErrorKind::NotFound) =>
            {
                Ok(Value::Array(vec![]))
            }
            Err(error) => Err(error),
        }
    }

    fn send_json(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
        content_type: Option<&str>,
    ) -> OcpResult<Value> {
        let url = format!("{}{}", self.server, path);
        let mut request = self.http.request(method, &url);
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(content_type) = content_type {
            request = request.header(CONTENT_TYPE, content_type);
        }
        if let Some(body) = body {
            request = request.body(serde_json::to_vec(body).map_err(|error| {
                OcpError::invalid_manifest(
                    "manifest_encode",
                    format!("Kubernetes request body could not be encoded: {error}"),
                )
            })?);
        }
        decode_json_response(request)
    }

    /// Resolve a Kind, plural resource name, singular name, or short name through
    /// Kubernetes discovery. Discovery is lazy per apiVersion and cached for five minutes.
    pub fn discover_resource(
        &self,
        api_version: &str,
        identity: &str,
    ) -> OcpResult<DiscoveredResource> {
        let api_version = validate_api_version(api_version)?;
        let identity = identity.trim();
        if identity.is_empty() || identity.contains('/') {
            return Err(OcpError::discovery(
                "api_resource_required",
                "Kubernetes resource kind or resource name is required and must not be a subresource.",
            ));
        }
        let normalized = identity.to_ascii_lowercase();
        let mut cache = self
            .discovery
            .lock()
            .map_err(|_| OcpError::internal("Kubernetes discovery cache lock was poisoned."))?;
        let expired = cache
            .get(&api_version)
            .is_none_or(|entry| entry.loaded_at.elapsed() >= DISCOVERY_CACHE_TTL);
        if expired {
            let resources = self.load_discovery(&api_version)?;
            cache.insert(
                api_version.clone(),
                CachedDiscovery {
                    loaded_at: Instant::now(),
                    resources,
                },
            );
        }
        cache
            .get(&api_version)
            .and_then(|entry| {
                entry.resources.iter().find(|resource| {
                    resource.kind.eq_ignore_ascii_case(identity)
                        || resource.name.eq_ignore_ascii_case(identity)
                        || resource
                            .singular_name
                            .as_deref()
                            .is_some_and(|name| name.eq_ignore_ascii_case(identity))
                        || resource.short_names.iter().any(|name| name == &normalized)
                })
            })
            .cloned()
            .ok_or_else(|| {
                OcpError::discovery(
                    "api_resource_not_found",
                    format!(
                        "Kubernetes API discovery found no resource '{identity}' in apiVersion '{api_version}'."
                    ),
                )
            })
    }

    fn load_discovery(&self, api_version: &str) -> OcpResult<Vec<DiscoveredResource>> {
        let (group, version) = split_api_version(api_version)?;
        let path = discovery_path(&group, &version);
        let value = self.get_json(&path, &[]).map_err(|error| {
            if error.kind == OcpErrorKind::NotFound {
                OcpError::discovery(
                    "api_version_not_found",
                    format!(
                        "Kubernetes API discovery could not find apiVersion '{api_version}': {}",
                        error.message
                    ),
                )
            } else {
                error
            }
        })?;
        let resources = value
            .get("resources")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                OcpError::discovery(
                    "discovery_response_invalid",
                    format!(
                        "Kubernetes discovery response for '{api_version}' had no resources array."
                    ),
                )
            })?;
        let discovered = resources
            .iter()
            .filter_map(|resource| {
                let name = resource.get("name")?.as_str()?;
                if name.contains('/') {
                    return None;
                }
                let kind = resource.get("kind")?.as_str()?;
                let verbs = resource
                    .get("verbs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<HashSet<_>>();
                let short_names = resource
                    .get("shortNames")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|name| name.to_ascii_lowercase())
                    .collect();
                Some(DiscoveredResource {
                    api_version: api_version.to_string(),
                    group: group.clone(),
                    version: version.clone(),
                    name: name.to_string(),
                    singular_name: resource
                        .get("singularName")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    kind: kind.to_string(),
                    namespaced: resource
                        .get("namespaced")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    verbs,
                    short_names,
                })
            })
            .collect::<Vec<_>>();
        if discovered.is_empty() {
            return Err(OcpError::discovery(
                "discovery_empty",
                format!("Kubernetes discovery returned no resources for '{api_version}'."),
            ));
        }
        Ok(discovered)
    }

    pub fn list_resource(
        &self,
        resource: &DiscoveredResource,
        namespace: Option<&str>,
        query: &[(&str, String)],
    ) -> OcpResult<Value> {
        require_verb(resource, "list")?;
        let path = resource_path(resource, namespace, None)?;
        self.send_json(Method::GET, &path, query, None, None)
            .map_err(|error| contextualize_resource_error(error, "list", resource, namespace))
    }

    pub fn list_resource_all_namespaces(
        &self,
        resource: &DiscoveredResource,
        query: &[(&str, String)],
    ) -> OcpResult<Value> {
        require_verb(resource, "list")?;
        if !resource.namespaced {
            return Err(OcpError::config(
                "all_namespaces_not_applicable",
                format!(
                    "Kubernetes resource '{}' is cluster-scoped and has no all-namespaces endpoint.",
                    resource.qualified_name()
                ),
            ));
        }
        let path = format!(
            "{}/{}",
            discovery_path(&resource.group, &resource.version),
            resource.name
        );
        self.send_json(Method::GET, &path, query, None, None)
            .map_err(|error| contextualize_resource_error(error, "list", resource, None))
    }

    pub fn get_resource(
        &self,
        resource: &DiscoveredResource,
        namespace: Option<&str>,
        name: &str,
    ) -> OcpResult<Value> {
        require_verb(resource, "get")?;
        let path = resource_path(resource, namespace, Some(name))?;
        self.send_json(Method::GET, &path, &[], None, None)
            .map_err(|error| contextualize_resource_error(error, "get", resource, namespace))
    }

    pub fn create_resource(
        &self,
        resource: &DiscoveredResource,
        namespace: Option<&str>,
        manifest: &Value,
    ) -> OcpResult<Value> {
        require_verb(resource, "create")?;
        let path = resource_path(resource, namespace, None)?;
        self.send_json(
            Method::POST,
            &path,
            &[],
            Some(manifest),
            Some("application/json"),
        )
        .map_err(|error| contextualize_resource_error(error, "create", resource, namespace))
    }

    pub fn update_resource(
        &self,
        resource: &DiscoveredResource,
        namespace: Option<&str>,
        name: &str,
        manifest: &Value,
    ) -> OcpResult<Value> {
        require_verb(resource, "update")?;
        let path = resource_path(resource, namespace, Some(name))?;
        self.send_json(
            Method::PUT,
            &path,
            &[],
            Some(manifest),
            Some("application/json"),
        )
        .map_err(|error| contextualize_resource_error(error, "update", resource, namespace))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn patch_resource(
        &self,
        resource: &DiscoveredResource,
        namespace: Option<&str>,
        name: &str,
        patch: &Value,
        patch_type: KubernetesPatchType,
        field_manager: Option<&str>,
        force: bool,
    ) -> OcpResult<Value> {
        require_verb(resource, "patch")?;
        let path = resource_path(resource, namespace, Some(name))?;
        let mut query = Vec::new();
        if patch_type == KubernetesPatchType::ServerApply {
            let field_manager = field_manager
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    OcpError::invalid_manifest(
                        "field_manager_required",
                        "Server-side apply requires a non-empty field_manager.",
                    )
                })?;
            query.push(("fieldManager", field_manager.to_string()));
            if force {
                query.push(("force", "true".to_string()));
            }
        }
        self.send_json(
            Method::PATCH,
            &path,
            &query,
            Some(patch),
            Some(patch_type.content_type()),
        )
        .map_err(|error| contextualize_resource_error(error, "patch", resource, namespace))
    }

    pub fn delete_resource(
        &self,
        resource: &DiscoveredResource,
        namespace: Option<&str>,
        name: &str,
        options: &Value,
    ) -> OcpResult<Value> {
        require_verb(resource, "delete")?;
        let path = resource_path(resource, namespace, Some(name))?;
        self.send_json(
            Method::DELETE,
            &path,
            &[],
            Some(options),
            Some("application/json"),
        )
        .map_err(|error| contextualize_resource_error(error, "delete", resource, namespace))
    }

    // ── High-level operations ────────────────────────────────────────────────

    pub fn get_namespaces(&self) -> OcpResult<Vec<String>> {
        let items = self.get_list("/api/v1/namespaces", &[], false)?;
        Ok(names_of_items(&items))
    }

    pub fn list_namespaced(
        &self,
        group: &str,
        version: &str,
        plural: &str,
        namespace: Option<&str>,
    ) -> OcpResult<Value> {
        let namespace = namespace.unwrap_or(&self.default_namespace);
        let path = namespaced_path(group, version, namespace, plural, None);
        self.get_list(&path, &[], true)
    }

    pub fn get_namespaced(
        &self,
        group: &str,
        version: &str,
        plural: &str,
        name: &str,
        namespace: Option<&str>,
    ) -> OcpResult<Value> {
        let namespace = namespace.unwrap_or(&self.default_namespace);
        let path = namespaced_path(group, version, namespace, plural, Some(name));
        self.get_json(&path, &[])
    }

    pub fn patch_namespaced(
        &self,
        group: &str,
        version: &str,
        plural: &str,
        name: &str,
        namespace: Option<&str>,
        patch: &Value,
    ) -> OcpResult<Value> {
        let namespace = namespace.unwrap_or(&self.default_namespace);
        let path = namespaced_path(group, version, namespace, plural, Some(name));
        let url = format!("{}{}", self.server, path);
        let response = self
            .http
            .patch(&url)
            .header(CONTENT_TYPE, "application/merge-patch+json")
            .json(patch)
            .send()
            .map_err(translate_reqwest_error)?;
        let status = response.status();
        let body = response.text().map_err(translate_reqwest_error)?;
        handle_api_status(status, &body)?;
        serde_json::from_str(&body).map_err(|error| {
            OcpError::api(
                "response_json",
                format!("API server returned invalid JSON: {error}"),
            )
        })
    }

    pub fn delete_namespaced(
        &self,
        group: &str,
        version: &str,
        plural: &str,
        name: &str,
        namespace: Option<&str>,
    ) -> OcpResult<Value> {
        let namespace = namespace.unwrap_or(&self.default_namespace);
        let path = namespaced_path(group, version, namespace, plural, Some(name));
        let url = format!("{}{}", self.server, path);
        let response = self
            .http
            .delete(&url)
            .send()
            .map_err(translate_reqwest_error)?;
        let status = response.status();
        let body = response.text().map_err(translate_reqwest_error)?;
        handle_api_status(status, &body)?;
        if body.trim().is_empty() {
            return Ok(json!({ "status": "Success" }));
        }
        serde_json::from_str(&body).map_err(|error| {
            OcpError::api(
                "response_json",
                format!("API server returned invalid JSON: {error}"),
            )
        })
    }

    pub fn pod_logs(
        &self,
        namespace: &str,
        pod: &str,
        container: Option<&str>,
        tail_lines: Option<u32>,
    ) -> OcpResult<String> {
        let namespace = namespace.trim();
        if namespace.is_empty() {
            return Err(OcpError::config(
                "namespace",
                "Namespace is required for pod logs.",
            ));
        }
        let pod = pod.trim();
        if pod.is_empty() {
            return Err(OcpError::config("pod", "Pod name is required for logs."));
        }
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(container) = container.filter(|value| !value.trim().is_empty()) {
            query.push(("container", container.to_string()));
        }
        if let Some(tail) = tail_lines.filter(|value| *value > 0) {
            query.push(("tailLines", tail.to_string()));
        }
        let query: Vec<(&str, &str)> = query
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let path = format!("/api/v1/namespaces/{namespace}/pods/{pod}/log");
        let url = format!("{}{}", self.server, path);
        let response = self
            .http
            .get(&url)
            .query(&query)
            .send()
            .map_err(translate_reqwest_error)?;
        let status = response.status();
        let body = response.text().map_err(translate_reqwest_error)?;
        handle_api_status(status, &body)?;
        Ok(body)
    }

    pub fn version(&self) -> OcpResult<Value> {
        self.get_json("/version", &[])
    }
}

fn decode_json_response(request: RequestBuilder) -> OcpResult<Value> {
    let response = request.send().map_err(translate_reqwest_error)?;
    let status = response.status();
    let body = response.text().map_err(translate_reqwest_error)?;
    handle_api_status(status, &body)?;
    if body.trim().is_empty() {
        return Ok(json!({ "status": "Success" }));
    }
    serde_json::from_str(&body).map_err(|error| {
        OcpError::api(
            "response_json",
            format!("API server returned invalid JSON: {error}"),
        )
    })
}

fn validate_api_version(api_version: &str) -> OcpResult<String> {
    let api_version = api_version.trim();
    let (group, version) = split_api_version(api_version)?;
    let mut segments = vec![("API version", version.as_str())];
    if !group.is_empty() {
        segments.push(("API group", group.as_str()));
    }
    for (field, value) in segments {
        if value.is_empty()
            || value.len() > 253
            || value.chars().any(|character| {
                !(character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
            })
        {
            return Err(OcpError::discovery(
                "api_version_invalid",
                format!("{field} '{value}' is not a valid Kubernetes discovery path segment."),
            ));
        }
    }
    Ok(api_version.to_string())
}

fn split_api_version(api_version: &str) -> OcpResult<(String, String)> {
    let mut parts = api_version.trim().split('/');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    if parts.next().is_some() || first.is_empty() {
        return Err(OcpError::discovery(
            "api_version_invalid",
            "apiVersion must be 'v1' for core resources or 'group/version' for grouped resources.",
        ));
    }
    match second {
        Some(version) if !version.is_empty() => Ok((first.to_string(), version.to_string())),
        Some(_) => Err(OcpError::discovery(
            "api_version_invalid",
            "Grouped apiVersion must include a non-empty version.",
        )),
        None => Ok((String::new(), first.to_string())),
    }
}

fn discovery_path(group: &str, version: &str) -> String {
    if group.is_empty() {
        format!("/api/{version}")
    } else {
        format!("/apis/{group}/{version}")
    }
}

fn resource_path(
    resource: &DiscoveredResource,
    namespace: Option<&str>,
    name: Option<&str>,
) -> OcpResult<String> {
    let namespace = namespace.map(str::trim).filter(|value| !value.is_empty());
    if resource.namespaced && namespace.is_none() {
        return Err(OcpError::config(
            "namespace_required",
            format!(
                "Namespace is required for namespaced Kubernetes resource '{}'.",
                resource.qualified_name()
            ),
        ));
    }
    if !resource.namespaced && namespace.is_some() {
        return Err(OcpError::config(
            "namespace_not_allowed",
            format!(
                "Kubernetes resource '{}' is cluster-scoped; omit namespace.",
                resource.qualified_name()
            ),
        ));
    }
    if let Some(namespace) = namespace {
        validate_path_segment(namespace, "namespace")?;
    }
    if let Some(name) = name {
        validate_path_segment(name, "name")?;
    }
    let mut path = discovery_path(&resource.group, &resource.version);
    if let Some(namespace) = namespace {
        path.push_str("/namespaces/");
        path.push_str(namespace);
    }
    path.push('/');
    path.push_str(&resource.name);
    if let Some(name) = name {
        path.push('/');
        path.push_str(name);
    }
    Ok(path)
}

fn validate_path_segment(value: &str, field: &str) -> OcpResult<()> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 253
        || value.starts_with(['-', '.'])
        || value.ends_with(['-', '.'])
        || value.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '_' | ':'))
        })
    {
        return Err(OcpError::config(
            "invalid_arguments",
            format!("Kubernetes {field} '{value}' is not a valid path segment."),
        ));
    }
    Ok(())
}

fn require_verb(resource: &DiscoveredResource, verb: &str) -> OcpResult<()> {
    if resource.supports(verb) {
        Ok(())
    } else {
        Err(OcpError::discovery(
            "verb_not_supported",
            format!(
                "Kubernetes discovery reports that '{}' does not support verb '{verb}'.",
                resource.qualified_name()
            ),
        ))
    }
}

fn contextualize_resource_error(
    mut error: OcpError,
    verb: &str,
    resource: &DiscoveredResource,
    namespace: Option<&str>,
) -> OcpError {
    if matches!(
        error.kind,
        OcpErrorKind::Forbidden | OcpErrorKind::NotFound | OcpErrorKind::Conflict
    ) {
        let scope = namespace
            .map(|namespace| format!(" in namespace '{namespace}'"))
            .unwrap_or_else(|| " at cluster scope".to_string());
        error.message = format!(
            "{} Requested verb '{verb}' on '{}'{}.",
            error.message,
            resource.qualified_name(),
            scope
        );
    }
    error
}

fn namespaced_path(
    group: &str,
    version: &str,
    namespace: &str,
    plural: &str,
    name: Option<&str>,
) -> String {
    let prefix = if group.is_empty() {
        format!("/api/{version}")
    } else {
        format!("/apis/{group}/{version}")
    };
    let suffix = name.map_or_else(String::new, |name| format!("/{name}"));
    format!("{prefix}/namespaces/{namespace}/{plural}{suffix}")
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn names_of_items(items: &Value) -> Vec<String> {
    items
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["metadata"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub fn bearer_headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
        headers.insert(AUTHORIZATION, value);
    }
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_of_items_extracts_metadata_names() {
        let items = serde_json::json!([
            {"metadata": {"name": "a"}},
            {"metadata": {"name": "b"}}
        ]);
        assert_eq!(names_of_items(&items), vec!["a", "b"]);
    }

    #[test]
    fn namespaced_paths_honor_explicit_namespace() {
        assert_eq!(
            namespaced_path("", "v1", "kube-system", "pods", None),
            "/api/v1/namespaces/kube-system/pods"
        );
        assert_eq!(
            namespaced_path("apps", "v1", "team-a", "deployments", Some("web")),
            "/apis/apps/v1/namespaces/team-a/deployments/web"
        );
    }

    #[test]
    fn generic_paths_support_cluster_rbac_names() {
        let resource = DiscoveredResource {
            api_version: "rbac.authorization.k8s.io/v1".to_string(),
            group: "rbac.authorization.k8s.io".to_string(),
            version: "v1".to_string(),
            name: "clusterroles".to_string(),
            singular_name: Some("clusterrole".to_string()),
            kind: "ClusterRole".to_string(),
            namespaced: false,
            verbs: HashSet::from(["get".to_string()]),
            short_names: vec![],
        };
        assert_eq!(
            resource_path(&resource, None, Some("system:node")).unwrap(),
            "/apis/rbac.authorization.k8s.io/v1/clusterroles/system:node"
        );
    }

    #[test]
    fn bearer_headers_carry_authorization_without_exposing_body() {
        let headers = bearer_headers("token-abc");
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer token-abc");
    }

    #[test]
    fn default_timeouts_are_bounded() {
        let timeouts = OcpTimeouts::default();
        assert!(timeouts.connect <= Duration::from_secs(10));
        assert!(timeouts.request <= Duration::from_secs(30));
    }

    #[test]
    fn timeouts_from_policy_respect_configured_values() {
        let policy = OcpTimeoutPolicy {
            connect_secs: Some(5),
            request_secs: Some(20),
            preflight_secs: Some(45),
        };
        let timeouts = OcpTimeouts::from_policy(&policy);
        assert_eq!(timeouts.connect, Duration::from_secs(5));
        assert_eq!(timeouts.request, Duration::from_secs(20));
    }

    #[test]
    fn timeouts_from_policy_uses_defaults_for_absent_fields() {
        let policy = OcpTimeoutPolicy {
            connect_secs: None,
            request_secs: None,
            preflight_secs: None,
        };
        let timeouts = OcpTimeouts::from_policy(&policy);
        assert_eq!(timeouts.connect, OcpTimeouts::default().connect);
        assert_eq!(timeouts.request, OcpTimeouts::default().request);
    }
}
