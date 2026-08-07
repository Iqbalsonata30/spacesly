use std::time::Duration;

use base64::Engine;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
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
            Err(error) if allow_missing_kind && error.kind == OcpErrorKind::Api => {
                Ok(Value::Array(vec![]))
            }
            Err(error) => Err(error),
        }
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
