use serde::Serialize;

/// Fine-grained error kind for the OCP connector.
///
/// `Config`, `Auth`, and `Forbidden` are permanent (no retry).
/// `Connect`, `Timeout`, `Api`, `Tls`, and `DnsResolution` are transient (retry eligible).
/// `Protocol` and `Internal` are non-retryable technical failures.
/// `Cancelled` signals deliberate cancellation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OcpErrorKind {
    /// Bad configuration — wrong kubeconfig path, invalid URL, missing token, etc.
    Config,
    /// TCP connection failure or general transport error.
    Connect,
    /// Request or handshake timed out.
    Timeout,
    /// TLS certificate validation failure (CA mismatch, expired cert, hostname mismatch).
    Tls,
    /// DNS hostname resolution failed.
    DnsResolution,
    /// HTTP 401 — credentials were rejected.
    Auth,
    /// HTTP 403 — credentials are valid but RBAC denies the operation.
    Forbidden,
    /// Non-5xx HTTP error or unexpected API response shape.
    Api,
    /// MCP protocol-level error (malformed JSON-RPC, unexpected message, etc.).
    Protocol,
    /// Deliberate cancellation by the caller.
    Cancelled,
    /// Unexpected internal error.
    Internal,
}

impl OcpErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Connect => "connect",
            Self::Timeout => "timeout",
            Self::Tls => "tls",
            Self::DnsResolution => "dns_resolution",
            Self::Auth => "auth",
            Self::Forbidden => "forbidden",
            Self::Api => "api",
            Self::Protocol => "protocol",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }
}

/// A single connector error.  Cheap to clone; message is heap-allocated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OcpError {
    pub kind: OcpErrorKind,
    /// Stable, machine-readable code (snake_case), e.g. `tcp_connect_timeout`.
    pub code: String,
    /// Safe user-facing description (must not contain secrets).
    pub message: String,
}

impl OcpError {
    pub fn new(kind: OcpErrorKind, code: &str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn config(code: &str, message: impl Into<String>) -> Self {
        Self::new(OcpErrorKind::Config, code, message)
    }

    pub fn connect(code: &str, message: impl Into<String>) -> Self {
        Self::new(OcpErrorKind::Connect, code, message)
    }

    pub fn auth(code: &str, message: impl Into<String>) -> Self {
        Self::new(OcpErrorKind::Auth, code, message)
    }

    pub fn forbidden(code: &str, message: impl Into<String>) -> Self {
        Self::new(OcpErrorKind::Forbidden, code, message)
    }

    pub fn api(code: &str, message: impl Into<String>) -> Self {
        Self::new(OcpErrorKind::Api, code, message)
    }

    pub fn timeout(code: &str, message: impl Into<String>) -> Self {
        Self::new(OcpErrorKind::Timeout, code, message)
    }

    pub fn tls(code: &str, message: impl Into<String>) -> Self {
        Self::new(OcpErrorKind::Tls, code, message)
    }

    pub fn dns(code: &str, message: impl Into<String>) -> Self {
        Self::new(OcpErrorKind::DnsResolution, code, message)
    }

    pub fn protocol(code: &str, message: impl Into<String>) -> Self {
        Self::new(OcpErrorKind::Protocol, code, message)
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(OcpErrorKind::Cancelled, "cancelled", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(OcpErrorKind::Internal, "internal", message)
    }

    /// True when the operation that produced this error may succeed on a later attempt.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.kind,
            OcpErrorKind::Connect
                | OcpErrorKind::Timeout
                | OcpErrorKind::Api
                | OcpErrorKind::DnsResolution
        )
    }

    /// True when retrying will never succeed without a configuration change.
    pub fn is_permanent(&self) -> bool {
        matches!(
            self.kind,
            OcpErrorKind::Config
                | OcpErrorKind::Auth
                | OcpErrorKind::Forbidden
                | OcpErrorKind::Tls
                | OcpErrorKind::Protocol
        )
    }

    pub fn is_cancelled(&self) -> bool {
        self.kind == OcpErrorKind::Cancelled
    }

    /// Return a copy of this error with any secret substring replaced by `[REDACTED]`.
    pub fn redacted(&self, secrets: &[&str]) -> Self {
        Self {
            kind: self.kind,
            code: self.code.clone(),
            message: redact(&self.message, secrets),
        }
    }
}

impl std::fmt::Display for OcpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}[{}]: {}",
            self.kind.as_str(),
            self.code,
            self.message
        )
    }
}

impl std::error::Error for OcpError {}

pub type OcpResult<T> = Result<T, OcpError>;

// ── Structured error (for Tauri responses) ──────────────────────────────────

/// Enriched error returned to the frontend over Tauri IPC.
///
/// Carries a correlation ID and the preflight stage at which the failure
/// occurred so the UI can pinpoint which step failed.
/// Secret fields are never included; `message` is pre-redacted.
#[derive(Clone, Debug, Serialize)]
pub struct OcpStructuredError {
    /// Machine-readable kind string.
    pub kind: String,
    /// Stable error code.
    pub code: String,
    /// Safe user-facing description.
    pub message: String,
    /// True when the same operation may succeed on retry.
    pub retryable: bool,
    /// Identifies which preflight or runtime stage failed.
    pub stage: Option<String>,
    /// Monotonically unique identifier for log correlation.
    pub correlation_id: String,
    /// UTC milliseconds when the error occurred.
    pub timestamp_ms: u64,
}

impl OcpStructuredError {
    pub fn from_error(error: &OcpError, stage: Option<&str>) -> Self {
        Self {
            kind: error.kind.as_str().to_string(),
            code: error.code.clone(),
            message: error.message.clone(),
            retryable: error.is_retryable(),
            stage: stage.map(str::to_string),
            correlation_id: new_correlation_id(),
            timestamp_ms: crate::infrastructure::ocp::config::now_millis(),
        }
    }
}

fn new_correlation_id() -> String {
    // Deterministic but unique within a process lifetime using a monotonic counter
    // combined with the current millisecond timestamp.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = crate::infrastructure::ocp::config::now_millis();
    format!("{ts:016x}-{seq:08x}")
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Replace every occurrence of each secret string (≥ 4 chars) with `[REDACTED]`.
pub fn redact(text: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .filter(|value| value.len() >= 4)
        .fold(text.to_string(), |message, value| {
            message.replace(value, "[REDACTED]")
        })
}

/// Classify a `reqwest::Error` into the appropriate `OcpError`.
///
/// This is the single place where reqwest error semantics are mapped to our
/// domain.  TLS errors are identified by inspecting the error chain for known
/// marker text; reqwest itself does not expose a dedicated TLS variant.
pub fn translate_reqwest_error(error: reqwest::Error) -> OcpError {
    if error.is_timeout() {
        return OcpError::timeout(
            "request_timeout",
            format!("Cluster request timed out: {error}"),
        );
    }

    // Check for TLS-related error text before the general connect test,
    // because reqwest marks TLS failures as connect errors.
    let error_string = error.to_string().to_lowercase();
    if is_dns_error(&error_string) {
        return OcpError::dns(
            "dns_resolution_failed",
            format!("Could not resolve the cluster API server hostname: {error}"),
        );
    }
    if is_tls_error(&error_string) {
        return classify_tls_error(&error_string, error);
    }

    if error.is_connect() {
        return OcpError::connect(
            "connect_failed",
            format!("Failed to reach the cluster API server: {error}"),
        );
    }
    if error.is_redirect() {
        return OcpError::connect(
            "redirect",
            format!("Cluster API server redirect failed: {error}"),
        );
    }
    if error.is_decode() {
        return OcpError::api(
            "response_decode",
            format!("Cluster response could not be decoded: {error}"),
        );
    }
    OcpError::connect("transport", format!("Cluster request failed: {error}"))
}

fn is_dns_error(message: &str) -> bool {
    message.contains("name or service not known")
        || message.contains("nodename nor servname provided")
        || message.contains("getaddrinfo")
        || message.contains("failed to lookup address information")
        || message.contains("no address associated with hostname")
        || message.contains("temporary failure in name resolution")
        || message.contains("dns error")
}

fn is_tls_error(message: &str) -> bool {
    message.contains("certificate")
        || message.contains("tls")
        || message.contains("ssl")
        || message.contains("handshake")
        || message.contains("hostname mismatch")
        || message.contains("invalid cert")
        || message.contains("self-signed")
        || message.contains("unknown ca")
        || message.contains("certificate verify failed")
        || message.contains("certificate has expired")
}

fn classify_tls_error(message: &str, original: reqwest::Error) -> OcpError {
    if message.contains("expired") || message.contains("has expired") {
        return OcpError::tls(
            "tls_certificate_expired",
            format!("The cluster TLS certificate has expired: {original}"),
        );
    }
    if message.contains("hostname mismatch")
        || message.contains("alt names")
        || message.contains("san")
    {
        return OcpError::tls(
            "tls_hostname_mismatch",
            format!("The cluster TLS certificate hostname does not match: {original}"),
        );
    }
    if message.contains("unknown ca")
        || message.contains("self-signed")
        || message.contains("certificate verify failed")
    {
        return OcpError::tls(
            "tls_ca_invalid",
            format!("The cluster TLS certificate authority is not trusted: {original}"),
        );
    }
    OcpError::tls(
        "tls_handshake_failed",
        format!("TLS handshake with the cluster API server failed: {original}"),
    )
}

/// Build a user-facing message from an HTTP status code and response body.
pub fn api_status_message(code: reqwest::StatusCode, body: &str, fallback: &str) -> String {
    let reason = code.canonical_reason().unwrap_or("error");
    let detail = parse_api_message(body).unwrap_or_else(|| fallback.to_string());
    format!("HTTP {code} {reason}: {detail}")
}

fn parse_api_message(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

// ── HTTP status → OcpError ───────────────────────────────────────────────────

pub fn handle_api_status(status: reqwest::StatusCode, body: &str) -> OcpResult<()> {
    if status.is_success() {
        return Ok(());
    }
    match status.as_u16() {
        401 => Err(OcpError::auth(
            "token_rejected",
            api_status_message(status, body, "The cluster rejected the credentials."),
        )),
        403 => Err(OcpError::forbidden(
            "rbac_denied",
            api_status_message(status, body, "The cluster denied access to this resource."),
        )),
        404 => Err(OcpError::api(
            "not_found",
            api_status_message(
                status,
                body,
                "The requested cluster resource was not found.",
            ),
        )),
        408 | 425 | 429 | 500 | 502 | 503 | 504 => Err(OcpError::api(
            "server_error",
            api_status_message(status, body, "The cluster API server returned an error."),
        )),
        _ => Err(OcpError::api(
            "http_error",
            api_status_message(status, body, "The cluster API server returned an error."),
        )),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_classification_drives_retry_and_permanence() {
        let transient = OcpError::connect("tcp_failed", "dial error");
        assert!(transient.is_retryable());
        assert!(!transient.is_permanent());
        assert!(OcpError::timeout("stage_timeout", "slow").is_retryable());
        assert!(OcpError::api("bad_gateway", "502").is_retryable());
        assert!(OcpError::dns("dns_failed", "NXDOMAIN").is_retryable());

        let permanent = OcpError::auth("token_rejected", "unauthorized");
        assert!(!permanent.is_retryable());
        assert!(permanent.is_permanent());
        assert!(OcpError::config("missing_kubeconfig", "missing").is_permanent());
        assert!(OcpError::forbidden("rbac_denied", "denied").is_permanent());
        assert!(OcpError::tls("tls_ca_invalid", "unknown CA").is_permanent());
    }

    #[test]
    fn tls_is_not_retryable() {
        // TLS failures require configuration changes — never retry blindly.
        assert!(!OcpError::tls("tls_ca_invalid", "bad CA").is_retryable());
        assert!(OcpError::tls("tls_ca_invalid", "bad CA").is_permanent());
    }

    #[test]
    fn redaction_replaces_configured_secrets() {
        let error = OcpError::connect("tls_failed", "server rejected token 's3cret-token'");
        let redacted = error.redacted(&["s3cret-token"]);
        assert!(!redacted.message.contains("s3cret-token"));
        assert!(redacted.message.contains("[REDACTED]"));
        assert_eq!(redacted.kind, OcpErrorKind::Connect);
    }

    #[test]
    fn short_secret_fragments_are_never_redacted() {
        let error = OcpError::api("boom", "body mentions 'ab' often");
        let redacted = error.redacted(&["ab", "abc"]);
        assert!(redacted.message.contains("ab"));
    }

    #[test]
    fn api_status_extracts_kubernetes_message_field() {
        let body = r#"{"kind":"Status","message":"deployments.apps is forbidden: User cannot get resource","reason":"Forbidden"}"#;
        let message = api_status_message(reqwest::StatusCode::FORBIDDEN, body, "fallback");
        assert!(message.contains("deployments.apps is forbidden"));
        assert!(message.contains("403"));
    }

    #[test]
    fn handle_api_status_maps_all_standard_codes() {
        assert_eq!(
            handle_api_status(reqwest::StatusCode::UNAUTHORIZED, "{}")
                .unwrap_err()
                .kind,
            OcpErrorKind::Auth
        );
        assert_eq!(
            handle_api_status(reqwest::StatusCode::FORBIDDEN, "{}")
                .unwrap_err()
                .kind,
            OcpErrorKind::Forbidden
        );
        assert_eq!(
            handle_api_status(reqwest::StatusCode::NOT_FOUND, "{}")
                .unwrap_err()
                .kind,
            OcpErrorKind::Api
        );
        assert_eq!(
            handle_api_status(reqwest::StatusCode::TOO_MANY_REQUESTS, "{}")
                .unwrap_err()
                .kind,
            OcpErrorKind::Api
        );
        assert!(handle_api_status(reqwest::StatusCode::OK, "{}").is_ok());
    }

    #[test]
    fn is_tls_error_recognises_common_patterns() {
        assert!(is_tls_error("tls handshake failed"));
        assert!(is_tls_error("ssl error: certificate verify failed"));
        assert!(is_tls_error("unknown ca"));
        assert!(is_tls_error("hostname mismatch"));
        assert!(is_tls_error("certificate has expired"));
        assert!(!is_tls_error("connection refused"));
        assert!(!is_tls_error("dns lookup failed"));
    }

    #[test]
    fn structured_error_carries_correlation_id_and_stage() {
        let error = OcpError::connect("tcp_timeout", "timed out");
        let structured = OcpStructuredError::from_error(&error, Some("connectivity"));
        assert!(!structured.correlation_id.is_empty());
        assert_eq!(structured.stage.as_deref(), Some("connectivity"));
        assert_eq!(structured.code, "tcp_timeout");
        assert!(structured.retryable);
        assert!(structured.timestamp_ms > 0);
    }

    #[test]
    fn correlation_ids_are_unique_across_consecutive_calls() {
        let e = OcpError::internal("x");
        let a = OcpStructuredError::from_error(&e, None);
        let b = OcpStructuredError::from_error(&e, None);
        assert_ne!(a.correlation_id, b.correlation_id);
    }
}
