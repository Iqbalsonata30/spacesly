use std::net::ToSocketAddrs;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::client::OcpClient;
use super::config::{OcpTimeoutPolicy, ResolvedCluster};
use super::errors::{OcpError, OcpErrorKind, OcpResult};
use super::retry::{with_retry, RetryPolicy};
use super::tools;

// ── Stage constants ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreflightStage {
    Environment,
    Config,
    DnsProbe,
    Connectivity,
    Auth,
    Rbac,
    Tools,
}

impl PreflightStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Config => "config",
            Self::DnsProbe => "dns_probe",
            Self::Connectivity => "connectivity",
            Self::Auth => "auth",
            Self::Rbac => "rbac",
            Self::Tools => "tools",
        }
    }
}

// ── Result types ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
pub struct PreflightCheck {
    pub stage: &'static str,
    pub name: String,
    pub required: bool,
    pub passed: bool,
    pub detail: String,
    /// Milliseconds taken by this individual check.
    pub duration_ms: u64,
    /// Stable machine-readable error code when failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PreflightReport {
    pub passed: bool,
    pub passed_with_warnings: bool,
    pub failed_required: usize,
    pub checks: Vec<PreflightCheck>,
    /// Total elapsed milliseconds.
    pub total_duration_ms: u64,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run all preflight checks in order, stopping at the first required failure.
///
/// Checks are:
/// 1. Environment  — server URL and credentials present
/// 2. Config       — URL scheme, cert material validity
/// 3. DNS probe    — hostname resolves from the current runtime (fast; no TLS)
/// 4. Connectivity — HTTP GET /version (TLS + auth headers included)
/// 5. Auth         — namespace list accepted
/// 6. RBAC         — pod/deployment list, optional node/log probes
/// 7. Tools        — tool registry integrity
pub fn run_preflight(
    cluster: &ResolvedCluster,
    client: &OcpClient,
    policy: &OcpTimeoutPolicy,
) -> PreflightReport {
    let overall_start = Instant::now();
    let dns_timeout = policy.preflight_duration();

    let mut checks: Vec<PreflightCheck> = Vec::new();

    // Stage 1: environment
    checks.push(timed(check_environment(cluster)));
    if last_failed(&checks) {
        return finalize(checks, overall_start);
    }

    // Stage 2: config validation
    checks.push(timed(check_config(cluster)));
    if last_failed(&checks) {
        return finalize(checks, overall_start);
    }

    // Stage 3: DNS probe (independent of reqwest — pure OS resolver)
    checks.push(timed(check_dns(cluster, dns_timeout)));
    if last_failed(&checks) {
        return finalize(checks, overall_start);
    }

    // Stage 4: TCP + TLS + HTTP /version
    checks.push(timed(check_connectivity(client)));
    if last_failed(&checks) {
        return finalize(checks, overall_start);
    }

    // Stage 5: authentication
    checks.push(timed(check_auth(client)));
    if last_failed(&checks) {
        return finalize(checks, overall_start);
    }

    // Stage 6: RBAC probes
    checks.extend(check_rbac(client).into_iter().map(timed));
    if checks.iter().any(|c| c.required && !c.passed) {
        return finalize(checks, overall_start);
    }

    // Stage 7: tool registry
    checks.push(timed(check_tools()));

    finalize(checks, overall_start)
}

// ── Finalization ──────────────────────────────────────────────────────────────

/// Wrap a `PreflightCheckBuilder` with a start/stop timer.
fn timed(builder: PreflightCheckBuilder) -> PreflightCheck {
    let elapsed = builder.elapsed.as_millis() as u64;
    PreflightCheck {
        stage: builder.stage,
        name: builder.name,
        required: builder.required,
        passed: builder.passed,
        detail: builder.detail,
        duration_ms: elapsed,
        error_code: builder.error_code,
    }
}

fn last_failed(checks: &[PreflightCheck]) -> bool {
    checks
        .last()
        .map(|c| c.required && !c.passed)
        .unwrap_or(false)
}

fn finalize(mut checks: Vec<PreflightCheck>, started: Instant) -> PreflightReport {
    let failed_required = checks.iter().filter(|c| c.required && !c.passed).count();
    let warnings = checks.iter().filter(|c| !c.required && !c.passed).count();
    checks.sort_by_key(|c| c.stage);
    PreflightReport {
        passed: failed_required == 0,
        passed_with_warnings: failed_required == 0 && warnings > 0,
        failed_required,
        total_duration_ms: started.elapsed().as_millis() as u64,
        checks,
    }
}

// ── Internal builder type ─────────────────────────────────────────────────────

/// Intermediate type carrying the start time so `timed()` can measure duration.
struct PreflightCheckBuilder {
    stage: &'static str,
    name: String,
    required: bool,
    passed: bool,
    detail: String,
    elapsed: Duration,
    error_code: Option<String>,
}

impl PreflightCheckBuilder {
    fn ok(
        stage: &'static str,
        name: &str,
        required: bool,
        detail: String,
        elapsed: Duration,
    ) -> Self {
        Self {
            stage,
            name: name.to_string(),
            required,
            passed: true,
            detail,
            elapsed,
            error_code: None,
        }
    }

    fn fail(
        stage: &'static str,
        name: &str,
        required: bool,
        detail: String,
        elapsed: Duration,
        code: &str,
    ) -> Self {
        Self {
            stage,
            name: name.to_string(),
            required,
            passed: false,
            detail,
            elapsed,
            error_code: Some(code.to_string()),
        }
    }
}

// ── Stage implementations ─────────────────────────────────────────────────────

fn check_environment(cluster: &ResolvedCluster) -> PreflightCheckBuilder {
    let start = Instant::now();
    let mut failures: Vec<String> = Vec::new();

    if cluster.server.trim().is_empty() {
        failures.push("No cluster server URL was resolved.".to_string());
    }
    if cluster.credentials.is_anonymous() {
        failures.push("No cluster credentials were resolved.".to_string());
    }

    let elapsed = start.elapsed();
    if failures.is_empty() {
        PreflightCheckBuilder::ok(
            PreflightStage::Environment.as_str(),
            "Environment",
            true,
            "Cluster endpoint and credentials are present.".to_string(),
            elapsed,
        )
    } else {
        PreflightCheckBuilder::fail(
            PreflightStage::Environment.as_str(),
            "Environment",
            true,
            failures.join(" "),
            elapsed,
            "config_incomplete",
        )
    }
}

fn check_config(cluster: &ResolvedCluster) -> PreflightCheckBuilder {
    let start = Instant::now();
    let elapsed;

    match reqwest::Url::parse(&cluster.server) {
        Ok(url) if url.scheme() == "https" && url.host_str().is_some() => {}
        _ => {
            elapsed = start.elapsed();
            return PreflightCheckBuilder::fail(
                PreflightStage::Config.as_str(),
                "Config",
                true,
                "Cluster server URL must be a valid HTTPS URL with a host.".to_string(),
                elapsed,
                "server_url_invalid",
            );
        }
    }

    if let Some(cert) = &cluster.credentials.client_cert {
        if cert.certificate.is_empty() || cert.key.is_empty() {
            elapsed = start.elapsed();
            return PreflightCheckBuilder::fail(
                PreflightStage::Config.as_str(),
                "Config",
                true,
                "Client certificate material is incomplete.".to_string(),
                elapsed,
                "client_cert_incomplete",
            );
        }
    }

    elapsed = start.elapsed();
    PreflightCheckBuilder::ok(
        PreflightStage::Config.as_str(),
        "Config",
        true,
        "Connector configuration is valid.".to_string(),
        elapsed,
    )
}

/// DNS probe: resolve the API server hostname using the OS resolver.
///
/// This runs before the full HTTP request so we can distinguish a DNS failure
/// (NXDOMAIN / no route to host) from a TLS or auth failure.
/// The probe uses `std::net::ToSocketAddrs` which is synchronous but fast —
/// it respects `/etc/hosts`, `/etc/resolv.conf`, and the platform DNS cache.
fn check_dns(cluster: &ResolvedCluster, timeout: Duration) -> PreflightCheckBuilder {
    let start = Instant::now();

    // Extract host:port from the server URL.
    let url = match reqwest::Url::parse(&cluster.server) {
        Ok(u) => u,
        Err(_) => {
            return PreflightCheckBuilder::fail(
                PreflightStage::DnsProbe.as_str(),
                "DNS resolution",
                true,
                "Cluster server URL could not be parsed.".to_string(),
                start.elapsed(),
                "server_url_invalid",
            );
        }
    };
    let host = match url.host_str() {
        Some(h) => h.to_string(),
        None => {
            return PreflightCheckBuilder::fail(
                PreflightStage::DnsProbe.as_str(),
                "DNS resolution",
                true,
                "Cluster server URL has no host.".to_string(),
                start.elapsed(),
                "server_url_no_host",
            );
        }
    };
    let port = url.port_or_known_default().unwrap_or(6443);
    let addr_str = format!("{host}:{port}");

    match addr_str.to_socket_addrs() {
        Ok(mut addrs) => {
            let elapsed = start.elapsed();
            let first_ip = addrs.next().map(|a| a.ip().to_string()).unwrap_or_default();
            let detail = if first_ip.is_empty() {
                format!("Hostname '{host}' resolved (no addresses returned — may be proxied).")
            } else {
                format!("Hostname '{host}' resolved to {first_ip}.")
            };
            PreflightCheckBuilder::ok(
                PreflightStage::DnsProbe.as_str(),
                "DNS resolution",
                true,
                detail,
                elapsed,
            )
        }
        Err(err) => PreflightCheckBuilder::fail(
            PreflightStage::DnsProbe.as_str(),
            "DNS resolution",
            true,
            format!(
                "Could not resolve '{host}' within {}s: {err}. \
                 Verify the API server address and check VPN or network access from this machine.",
                timeout.as_secs()
            ),
            start.elapsed(),
            "dns_resolution_failed",
        ),
    }
}

fn check_connectivity(client: &OcpClient) -> PreflightCheckBuilder {
    let start = Instant::now();
    // Single-shot diagnostic probe: do not mask a transient failure with retries,
    // because the report must reflect the immediate state the user can act on.
    let cancelled = AtomicBool::new(false);
    let result = with_retry(RetryPolicy::none(), &cancelled, || client.version());
    match result {
        Ok(version) => {
            let git = version["gitVersion"].as_str().unwrap_or("unknown");
            PreflightCheckBuilder::ok(
                PreflightStage::Connectivity.as_str(),
                "Connectivity",
                true,
                format!("Reached the cluster API server (Kubernetes {git})."),
                start.elapsed(),
            )
        }
        Err(error) => {
            let code = connectivity_error_code(&error);
            PreflightCheckBuilder::fail(
                PreflightStage::Connectivity.as_str(),
                "Connectivity",
                true,
                format!("{} — {}", connectivity_user_hint(&error), error.message),
                start.elapsed(),
                code,
            )
        }
    }
}

/// Map a connectivity OcpError to a stable error code suitable for the UI.
fn connectivity_error_code(error: &OcpError) -> &'static str {
    match error.kind {
        OcpErrorKind::Timeout => "tcp_connect_timeout",
        OcpErrorKind::Tls => match error.code.as_str() {
            "tls_certificate_expired" => "tls_certificate_expired",
            "tls_hostname_mismatch" => "tls_hostname_mismatch",
            "tls_ca_invalid" => "tls_ca_invalid",
            _ => "tls_handshake_failed",
        },
        OcpErrorKind::Connect => "connection_refused",
        OcpErrorKind::Auth => "authentication_failed",
        // Transient kinds (API flakiness, DNS) may clear on retry; permanent kinds
        // indicate a configuration problem that must be fixed, not retried.
        kind if is_retryable_preflight_failure(&kind) => "cluster_api_unavailable",
        _ => "cluster_config_error",
    }
}

/// Return an actionable hint sentence for common connectivity failures.
fn connectivity_user_hint(error: &OcpError) -> &'static str {
    match error.kind {
        OcpErrorKind::Timeout => {
            "The connection to the cluster API server timed out. \
             Check VPN, firewall rules, and whether the API server port (6443) is reachable."
        }
        OcpErrorKind::Tls => match error.code.as_str() {
            "tls_certificate_expired" => {
                "The cluster TLS certificate has expired. \
                 Contact your cluster administrator to renew it."
            }
            "tls_hostname_mismatch" => {
                "The cluster TLS certificate does not match the configured hostname. \
                 Verify the API server URL is correct."
            }
            "tls_ca_invalid" => {
                "The cluster CA certificate is not trusted. \
                 Provide the correct CA certificate in the connector settings."
            }
            _ => {
                "The TLS handshake with the cluster API server failed. \
                 Check the CA certificate configuration."
            }
        },
        OcpErrorKind::Connect => {
            "The connection to the cluster API server was refused. \
             Verify the API server URL and port."
        }
        _ => "An unexpected error occurred while connecting to the cluster API server.",
    }
}

fn check_auth(client: &OcpClient) -> PreflightCheckBuilder {
    let start = Instant::now();
    match client.list_namespaced("", "v1", "pods", None) {
        Ok(_) => PreflightCheckBuilder::ok(
            PreflightStage::Auth.as_str(),
            "Authentication",
            true,
            format!(
                "The cluster accepted the configured identity in namespace '{}'.",
                client.default_namespace()
            ),
            start.elapsed(),
        ),
        Err(error) => {
            let code = match error.kind {
                OcpErrorKind::Auth => "token_rejected",
                OcpErrorKind::Forbidden => "rbac_insufficient",
                _ => "authentication_failed",
            };
            PreflightCheckBuilder::fail(
                PreflightStage::Auth.as_str(),
                "Authentication",
                true,
                error.message.clone(),
                start.elapsed(),
                code,
            )
        }
    }
}

fn check_rbac(client: &OcpClient) -> Vec<PreflightCheckBuilder> {
    let namespace = client.default_namespace();
    let mut checks = Vec::new();

    checks.push(rbac_probe("List namespaces (optional)", false, || {
        client
            .get_namespaces()
            .map(|_| "Namespaces can be listed.".to_string())
    }));
    checks.push(rbac_probe(
        &format!("List pods in '{namespace}'"),
        true,
        || {
            client.list_namespaced("", "v1", "pods", None).map(|items| {
                format!(
                    "{} pod(s) can be listed.",
                    items.as_array().map_or(0, Vec::len)
                )
            })
        },
    ));
    checks.push(rbac_probe(
        &format!("List deployments in '{namespace}'"),
        true,
        || {
            client
                .list_namespaced("apps", "v1", "deployments", None)
                .map(|_| "Deployments can be listed.".to_string())
        },
    ));
    checks.push(rbac_probe("List nodes (optional)", false, || {
        client
            .get_list("/api/v1/nodes", &[], false)
            .map(|_| "Nodes can be listed.".to_string())
    }));
    checks.push(rbac_probe(
        &format!("Read pod logs in '{namespace}' (optional)"),
        false,
        || {
            let pods = client.list_namespaced("", "v1", "pods", None)?;
            let pods = pods.as_array().cloned().unwrap_or_default();
            let Some(pod) = pods
                .first()
                .and_then(|item| item["metadata"]["name"].as_str())
            else {
                return Ok("No pods exist to sample log access.".to_string());
            };
            client
                .pod_logs(namespace, pod, None, Some(5))
                .map(|_| "Pod logs can be read.".to_string())
        },
    ));

    checks
}

fn rbac_probe(
    name: &str,
    required: bool,
    operation: impl FnOnce() -> OcpResult<String>,
) -> PreflightCheckBuilder {
    let start = Instant::now();
    match operation() {
        Ok(detail) => PreflightCheckBuilder::ok(
            PreflightStage::Rbac.as_str(),
            name,
            required,
            detail,
            start.elapsed(),
        ),
        Err(error) => {
            let code = match error.kind {
                OcpErrorKind::Forbidden => "rbac_denied",
                OcpErrorKind::Auth => "token_rejected",
                _ => "rbac_probe_failed",
            };
            PreflightCheckBuilder::fail(
                PreflightStage::Rbac.as_str(),
                name,
                required,
                error.message.clone(),
                start.elapsed(),
                code,
            )
        }
    }
}

fn check_tools() -> PreflightCheckBuilder {
    let start = Instant::now();
    let metadata = tools::tool_metadata();
    let valid = !metadata.is_empty()
        && metadata.iter().all(|spec| {
            spec["name"].as_str().is_some()
                && spec["description"].as_str().is_some()
                && spec["inputSchema"].is_object()
        });
    let elapsed = start.elapsed();
    if valid {
        PreflightCheckBuilder::ok(
            PreflightStage::Tools.as_str(),
            "Tool registry",
            true,
            format!(
                "{} tools available (read-only diagnostics plus approval-gated remediation).",
                metadata.len()
            ),
            elapsed,
        )
    } else {
        PreflightCheckBuilder::fail(
            PreflightStage::Tools.as_str(),
            "Tool registry",
            true,
            "The tool registry failed to validate.".to_string(),
            elapsed,
            "tool_registry_invalid",
        )
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn is_retryable_preflight_failure(kind: &OcpErrorKind) -> bool {
    matches!(
        kind,
        OcpErrorKind::Connect
            | OcpErrorKind::Timeout
            | OcpErrorKind::Api
            | OcpErrorKind::DnsResolution
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::ocp::config::ClientCredentials;

    fn cluster_with(server: &str, creds: ClientCredentials) -> ResolvedCluster {
        ResolvedCluster {
            server: server.to_string(),
            ca: None,
            insecure_skip_tls_verify: false,
            credentials: creds,
            default_namespace: None,
        }
    }

    #[test]
    fn stages_have_stable_string_names() {
        assert_eq!(PreflightStage::Environment.as_str(), "environment");
        assert_eq!(PreflightStage::Config.as_str(), "config");
        assert_eq!(PreflightStage::DnsProbe.as_str(), "dns_probe");
        assert_eq!(PreflightStage::Connectivity.as_str(), "connectivity");
        assert_eq!(PreflightStage::Auth.as_str(), "auth");
        assert_eq!(PreflightStage::Rbac.as_str(), "rbac");
        assert_eq!(PreflightStage::Tools.as_str(), "tools");
    }

    #[test]
    fn anonymous_identity_fails_environment_stage() {
        let cluster = cluster_with("https://api.example:6443", ClientCredentials::default());
        let check = timed(check_environment(&cluster));
        assert!(!check.passed);
        assert!(check.detail.contains("credentials"));
        assert_eq!(check.error_code.as_deref(), Some("config_incomplete"));
    }

    #[test]
    fn invalid_server_fails_config_stage() {
        let cluster = cluster_with(
            "ftp://nope",
            ClientCredentials::default().bearer_token("tok".to_string()),
        );
        let check = timed(check_config(&cluster));
        assert!(!check.passed);
        assert!(check.detail.contains("HTTPS"));
        assert_eq!(check.error_code.as_deref(), Some("server_url_invalid"));
    }

    #[test]
    fn dns_probe_fails_on_unresolvable_host() {
        let cluster = cluster_with(
            "https://this-host-definitely-does-not-exist.invalid:6443",
            ClientCredentials::default().bearer_token("tok".to_string()),
        );
        let check = timed(check_dns(&cluster, Duration::from_secs(3)));
        // Should fail — .invalid TLD is guaranteed to not resolve.
        assert!(!check.passed);
        assert_eq!(check.error_code.as_deref(), Some("dns_resolution_failed"));
        assert!(check.duration_ms < 15_000, "DNS probe should not hang");
    }

    #[test]
    fn dns_probe_succeeds_on_loopback() {
        let cluster = cluster_with(
            "https://localhost:6443",
            ClientCredentials::default().bearer_token("tok".to_string()),
        );
        let check = timed(check_dns(&cluster, Duration::from_secs(3)));
        // localhost always resolves.
        assert!(check.passed, "localhost should resolve: {:?}", check.detail);
        assert!(check.detail.contains("localhost"));
    }

    #[test]
    fn retryable_kinds_include_dns_exclude_auth_and_tls() {
        assert!(is_retryable_preflight_failure(&OcpErrorKind::Connect));
        assert!(is_retryable_preflight_failure(&OcpErrorKind::Timeout));
        assert!(is_retryable_preflight_failure(&OcpErrorKind::DnsResolution));
        assert!(!is_retryable_preflight_failure(&OcpErrorKind::Auth));
        assert!(!is_retryable_preflight_failure(&OcpErrorKind::Forbidden));
        assert!(!is_retryable_preflight_failure(&OcpErrorKind::Tls));
    }

    #[test]
    fn connectivity_error_code_maps_tls_subtypes() {
        assert_eq!(
            connectivity_error_code(&OcpError::tls("tls_certificate_expired", "x")),
            "tls_certificate_expired"
        );
        assert_eq!(
            connectivity_error_code(&OcpError::tls("tls_hostname_mismatch", "x")),
            "tls_hostname_mismatch"
        );
        assert_eq!(
            connectivity_error_code(&OcpError::tls("tls_ca_invalid", "x")),
            "tls_ca_invalid"
        );
        assert_eq!(
            connectivity_error_code(&OcpError::timeout("request_timeout", "x")),
            "tcp_connect_timeout"
        );
        assert_eq!(
            connectivity_error_code(&OcpError::connect("connect_failed", "x")),
            "connection_refused"
        );
    }

    #[test]
    fn preflight_check_carries_duration() {
        let cluster = cluster_with(
            "https://api.example:6443",
            ClientCredentials::default().bearer_token("tok".to_string()),
        );
        let check = timed(check_config(&cluster));
        // Duration in ms should be a non-negative integer (may be 0 on fast CPUs).
        assert!(check.duration_ms < 1_000);
    }
}
