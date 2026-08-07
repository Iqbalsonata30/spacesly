/**
 * Typed IPC wrappers for the OpenShift/Kubernetes connector Tauri commands.
 *
 * Design decisions:
 * - No secret values are ever returned to the frontend.
 * - `token_set` and `ca_data_set` booleans indicate presence without exposing value.
 * - All writes go to the backend; the frontend only holds display state.
 */
import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";
import type { McpConnectionStatus } from "$lib/ipc/jira";

// ── Config types ──────────────────────────────────────────────────────────────

export type OcpConnectionMode = "kubeconfig" | "api_server_token" | "in_cluster";

export type OcpEnvironmentLabel = "development" | "staging" | "production" | "";

export interface OcpTimeoutPolicy {
  connect_secs?: number;
  request_secs?: number;
  preflight_secs?: number;
}

/** Mirror of Rust OcpConfigSpec — no secrets included. */
export interface OcpConfigSpec {
  version: number;
  mode: OcpConnectionMode;
  kubeconfig_path?: string | null;
  kubeconfig_context?: string | null;
  server?: string | null;
  /** Whether a CA certificate is stored on the backend. */
  ca_data_set: boolean;
  /** Whether a bearer token is stored on the backend. */
  token_set: boolean;
  default_namespace?: string | null;
  display_name?: string | null;
  environment_label?: string | null;
  timeout_policy: OcpTimeoutPolicy;
  preflight_passed: boolean;
  updated_at_ms: number;
  checksum: string;
}

/** Which OCP secrets are currently stored — no values returned. */
export interface OcpSecretStatus {
  token_set: boolean;
  ca_data_set: boolean;
}

// ── Preflight types ───────────────────────────────────────────────────────────

export interface PreflightCheck {
  stage: string;
  name: string;
  required: boolean;
  passed: boolean;
  detail: string;
  duration_ms: number;
  error_code?: string | null;
}

export interface PreflightReport {
  passed: boolean;
  passed_with_warnings: boolean;
  failed_required: number;
  total_duration_ms: number;
  checks: PreflightCheck[];
}

// ── Connector status types ────────────────────────────────────────────────────

export interface OcpAuditEntry {
  timestamp: string;
  event: string;
  tool?: string | null;
  target?: string | null;
  outcome: string;
  detail?: string | null;
  latency_ms: number;
}

export interface OcpConnectorStatus {
  config?: OcpConfigSpec | null;
  last_known_good?: OcpConfigSpec | null;
  breaker_state: "closed" | "open" | "half_open";
  audit: OcpAuditEntry[];
}

// ── Draft save input (secrets passed as optional values, never returned) ──────

export interface OcpSaveDraftInput {
  mode: OcpConnectionMode;
  kubeconfig_path?: string | null;
  kubeconfig_context?: string | null;
  server?: string | null;
  default_namespace?: string | null;
  display_name?: string | null;
  environment_label?: string | null;
  /** Pass a new token to update it; omit to keep the existing stored token. */
  token?: string | null;
  /**
   * Base64-encoded PEM CA certificate.
   * Pass a new value to update; omit to keep the existing stored CA.
   */
  ca_pem_base64?: string | null;
  server_id: string;
  /** Optional per-stage timeout overrides in seconds. Omit to use defaults. */
  timeout_policy?: OcpTimeoutPolicy | null;
  [key: string]: unknown;
}

// ── IPC functions ─────────────────────────────────────────────────────────────

/**
 * Run a full staged preflight check against the cluster.
 *
 * `token` and `ca_data` are optional base64-encoded values passed to the
 * backend only for this test run; they are NOT automatically persisted.
 * Use `ocpSaveDraft` to persist configuration.
 */
export async function ocpPreflight(params: {
  mode: OcpConnectionMode;
  kubeconfig_path?: string | null;
  kubeconfig_context?: string | null;
  server?: string | null;
  token?: string | null;
  /** Base64-encoded PEM CA bytes */
  ca_data?: string | null;
  default_namespace?: string | null;
  /** Optional per-stage timeout overrides in seconds. Omit to use defaults. */
  timeout_policy?: OcpTimeoutPolicy | null;
}): Promise<PreflightReport> {
  return invokeWithPolicy<PreflightReport>("ocp_preflight", params, IPC_POLICIES.secret);
}

/** Get the current connector status (config, last-known-good, breaker, audit). */
export async function ocpConnectorStatus(): Promise<OcpConnectorStatus> {
  return invokeWithPolicy<OcpConnectorStatus>(
    "ocp_connector_status",
    undefined,
    IPC_POLICIES.secret,
  );
}

/**
 * Save a draft configuration.
 * Secrets (token, CA) are persisted securely and are never returned.
 * Returns the saved OcpConfigSpec (no secrets).
 */
export async function ocpSaveDraft(input: OcpSaveDraftInput): Promise<OcpConfigSpec> {
  return invokeWithPolicy<OcpConfigSpec>("ocp_save_draft", input, IPC_POLICIES.secret);
}

/** Test the saved connector through Spacesly's embedded OpenShift MCP process. */
export async function ocpTestMcpConnection(
  serverId: string,
  scopeId?: string,
): Promise<McpConnectionStatus> {
  return invokeWithPolicy<McpConnectionStatus>(
    "test_ocp_mcp_connection",
    { server_id: serverId, scope_id: scopeId ?? null },
    IPC_POLICIES.mcpTest,
  );
}

/** Return which secret types are currently stored for a connector. */
export async function ocpSecretStatus(serverId: string): Promise<OcpSecretStatus> {
  return invokeWithPolicy<OcpSecretStatus>(
    "ocp_secret_status",
    { server_id: serverId },
    IPC_POLICIES.secret,
  );
}

/** Delete all connector data: secrets, draft, last-known-good, audit log. */
export async function ocpDeleteConnector(serverId: string): Promise<void> {
  return invokeWithPolicy<void>(
    "ocp_delete_connector",
    { server_id: serverId },
    IPC_POLICIES.secret,
  );
}

/**
 * Rotate OCP credentials.
 * Resets preflight state so the connector must be re-tested after rotation.
 */
export async function ocpRotateCredentials(params: {
  server_id: string;
  token?: string | null;
  ca_pem_base64?: string | null;
}): Promise<void> {
  return invokeWithPolicy<void>("ocp_rotate_credentials", params, IPC_POLICIES.secret);
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/** Human-readable label for a connection mode. */
export function ocpModeName(mode: OcpConnectionMode): string {
  switch (mode) {
    case "kubeconfig":
      return "Existing kubeconfig";
    case "api_server_token":
      return "API server + token";
    case "in_cluster":
      return "In-cluster ServiceAccount";
  }
}

/** Stage display label for the preflight stepper. */
export function preflightStageName(stage: string): string {
  const labels: Record<string, string> = {
    environment: "Validating configuration",
    config: "Verifying URL and certificates",
    dns_probe: "Resolving hostname",
    connectivity: "Connecting to cluster",
    auth: "Verifying identity",
    rbac: "Checking permissions",
    tools: "Verifying tool registry",
  };
  return labels[stage] ?? stage;
}

/** Encode PEM text to base64 for transmission to the backend. */
export function pemToBase64(pem: string): string {
  return btoa(unescape(encodeURIComponent(pem.trim())));
}

/** Parse a PEM certificate block and return subject/issuer/expiry metadata. */
export function parseCertMeta(pem: string): {
  subject: string;
  issuer: string;
  validFrom: string;
  validTo: string;
} | null {
  // This is a best-effort client-side parse using the Web Crypto API.
  // Full parsing is done server-side; here we just extract PEM boundary lines
  // to show the user something useful before saving.
  const lines = pem.trim().split("\n");
  const header = lines.find((l) => l.startsWith("-----BEGIN"));
  if (!header) return null;
  // We cannot easily parse DER in the browser without a library.
  // Return a placeholder indicating the certificate was found.
  return {
    subject: "(parsed on save)",
    issuer: "(parsed on save)",
    validFrom: "",
    validTo: "",
  };
}
