<script lang="ts">
  import {
    ocpConnectorStatus,
    ocpDeleteConnector,
    ocpPreflight,
    ocpSaveDraft,
    ocpSecretStatus,
    pemToBase64,
    preflightStageName,
    type OcpConfigSpec,
    type OcpConnectionMode,
    type OcpConnectorStatus,
    type OcpSecretStatus,
    type OcpTimeoutPolicy,
    type PreflightCheck,
    type PreflightReport,
  } from "$lib/ipc/ocp";
  import SettingsCard from "$lib/components/settings/SettingsCard.svelte";
  import SettingsGroup from "$lib/components/settings/SettingsGroup.svelte";
  import SettingsHelperText from "$lib/components/settings/SettingsHelperText.svelte";
  import SettingsInput from "$lib/components/settings/SettingsInput.svelte";
  import SettingsLabel from "$lib/components/settings/SettingsLabel.svelte";
  import SettingsPage from "$lib/components/settings/SettingsPage.svelte";
  import SettingsRow from "$lib/components/settings/SettingsRow.svelte";
  import SettingsSection from "$lib/components/settings/SettingsSection.svelte";
  import { defaultMcpIntentMetadata, type McpServerSettings } from "$lib/settings";

  let {
    serverId,
    serverName = "OpenShift Connector",
    kind = "ocp",
    onUpdate,
    onSaved,
  }: {
    serverId: string;
    serverName?: string;
    kind?: McpServerSettings["kind"];
    onUpdate?: (values: Partial<McpServerSettings>) => void;
    onSaved?: (spec: OcpConfigSpec) => void;
  } = $props();

  // ── Form state ──────────────────────────────────────────────────────────────

  type Mode = OcpConnectionMode;

  let mode = $state<Mode>("kubeconfig");
  let kubeconfigPath = $state("");
  let kubeconfigContext = $state("");
  let server = $state("");
  let token = $state("");
  let caText = $state("");
  let defaultNamespace = $state("");
  let displayName = $state("");

  // Initialise displayName from prop once on mount
  $effect.pre(() => {
    if (!displayName) displayName = serverName;
  });
  let showAdvanced = $state(false);

  // Advanced timeout overrides (seconds). Empty string = use default.
  let connectTimeout = $state("");
  let requestTimeout = $state("");
  let preflightTimeout = $state("");

  // Secret status loaded from backend
  let secretStatus = $state<OcpSecretStatus>({ token_set: false, ca_data_set: false });

  // Dirty tracking
  let dirty = $state(false);

  // UI state
  let saving = $state(false);
  let saveError = $state<string | null>(null);
  let saveSuccess = $state(false);

  // Preflight state
  type PreflightState = "idle" | "running" | "done";
  let preflightState = $state<PreflightState>("idle");
  let preflightReport = $state<PreflightReport | null>(null);
  let preflightError = $state<string | null>(null);

  // Connector status (health / audit)
  let connectorStatus = $state<OcpConnectorStatus | null>(null);

  // Validation state
  let fieldErrors = $state<Record<string, string>>({});
  /** Fields the user has interacted with — errors only surface here (or after submit). */
  let touched = $state<Record<string, boolean>>({});
  let submitAttempted = $state(false);

  // ── Derived form validity ───────────────────────────────────────────────────

  const requiredValid = $derived(
    mode === "kubeconfig"
      ? kubeconfigPath.trim().length > 0
      : mode === "api_server_token"
        ? server.trim().startsWith("https://") &&
          (secretStatus.token_set || token.trim().length > 0)
        : true,
  );

  const canTest = $derived(requiredValid && !saving && preflightState !== "running");
  const canSave = $derived(requiredValid && !saving && dirty && preflightState !== "running");

  // ── Lifecycle ───────────────────────────────────────────────────────────────

  $effect(() => {
    loadSecretStatus();
    loadStatus();
  });

  async function loadSecretStatus() {
    try {
      secretStatus = await ocpSecretStatus(serverId);
    } catch {
      // non-fatal
    }
  }

  async function loadStatus() {
    try {
      connectorStatus = await ocpConnectorStatus();
      if (connectorStatus?.config) {
        const cfg = connectorStatus.config;
        mode = cfg.mode;
        kubeconfigPath = cfg.kubeconfig_path ?? "";
        kubeconfigContext = cfg.kubeconfig_context ?? "";
        server = cfg.server ?? "";
        defaultNamespace = cfg.default_namespace ?? "";
        displayName = cfg.display_name ?? serverName;
        connectTimeout =
          cfg.timeout_policy?.connect_secs != null ? String(cfg.timeout_policy.connect_secs) : "";
        requestTimeout =
          cfg.timeout_policy?.request_secs != null ? String(cfg.timeout_policy.request_secs) : "";
        preflightTimeout =
          cfg.timeout_policy?.preflight_secs != null
            ? String(cfg.timeout_policy.preflight_secs)
            : "";
        dirty = false;
      }
    } catch {
      // Non-fatal — connector may not be configured yet
    }
  }

  // ── Validation ──────────────────────────────────────────────────────────────

  function validate(): boolean {
    const errors: Record<string, string> = {};
    if (mode === "kubeconfig" && !kubeconfigPath.trim()) {
      errors.kubeconfigPath = "Kubeconfig file path is required.";
    }
    if (mode === "api_server_token") {
      if (!server.trim()) {
        errors.server = "API server URL is required.";
      } else if (!server.trim().startsWith("https://")) {
        errors.server = "API server URL must start with https://.";
      }
      if (!secretStatus.token_set && !token.trim()) {
        errors.token = "A bearer token is required.";
      }
    }
    fieldErrors = errors;
    return Object.keys(errors).length === 0;
  }

  /** Show a field error only after the user has touched it or attempted to save. */
  function fieldError(name: string): string | null {
    if (!touched[name] && !submitAttempted) return null;
    return fieldErrors[name] || null;
  }

  function markTouched(name: string) {
    touched[name] = true;
  }

  function clearError(name: string) {
    fieldErrors[name] = "";
  }

  // ── Save draft ──────────────────────────────────────────────────────────────

  async function saveDraft() {
    submitAttempted = true;
    if (!validate()) return;
    saving = true;
    saveError = null;
    saveSuccess = false;
    try {
      const spec = await ocpSaveDraft({
        mode,
        kubeconfig_path: kubeconfigPath.trim() || null,
        kubeconfig_context: kubeconfigContext.trim() || null,
        server: server.trim() || null,
        default_namespace: defaultNamespace.trim() || null,
        display_name: displayName.trim() || null,
        token: token.trim() || null,
        ca_pem_base64: caText.trim() ? pemToBase64(caText) : null,
        server_id: serverId,
        timeout_policy: timeoutPolicy(),
      });
      secretStatus = { token_set: spec.token_set, ca_data_set: spec.ca_data_set };
      token = "";
      caText = "";
      dirty = false;
      saveSuccess = true;
      onSaved?.(spec);
    } catch (err) {
      saveError = err instanceof Error ? err.message : String(err);
    } finally {
      saving = false;
    }
  }

  // ── Preflight ───────────────────────────────────────────────────────────────

  async function runPreflight() {
    submitAttempted = true;
    if (!validate()) return;
    preflightState = "running";
    preflightReport = null;
    preflightError = null;
    try {
      preflightReport = await ocpPreflight({
        mode,
        kubeconfig_path: kubeconfigPath.trim() || null,
        kubeconfig_context: kubeconfigContext.trim() || null,
        server: server.trim() || null,
        token: token.trim() || null,
        ca_data: caText.trim() ? pemToBase64(caText) : null,
        default_namespace: defaultNamespace.trim() || null,
        timeout_policy: timeoutPolicy(),
      });
    } catch (err) {
      preflightError = err instanceof Error ? err.message : String(err);
    } finally {
      preflightState = "done";
    }
  }

  async function saveAndTest() {
    submitAttempted = true;
    if (!validate()) return;
    await saveDraft();
    if (!saveError) await runPreflight();
  }

  // ── Delete ──────────────────────────────────────────────────────────────────

  let confirmDelete = $state(false);
  let deleting = $state(false);

  async function deleteConnector() {
    deleting = true;
    try {
      await ocpDeleteConnector(serverId);
      connectorStatus = null;
      secretStatus = { token_set: false, ca_data_set: false };
      confirmDelete = false;
    } catch (err) {
      saveError = err instanceof Error ? err.message : String(err);
    } finally {
      deleting = false;
    }
  }

  // ── Helpers ─────────────────────────────────────────────────────────────────

  function markDirty() {
    dirty = true;
    saveSuccess = false;
  }

  function changeKind(next: McpServerSettings["kind"]) {
    if (next === kind) return;
    onUpdate?.({ kind: next, ...defaultMcpIntentMetadata(next, serverName) });
  }

  /** Build the timeout override payload, or null when nothing is customized. */
  function timeoutPolicy(): OcpTimeoutPolicy | null {
    const policy: OcpTimeoutPolicy = {};
    if (connectTimeout.trim()) policy.connect_secs = Number(connectTimeout);
    if (requestTimeout.trim()) policy.request_secs = Number(requestTimeout);
    if (preflightTimeout.trim()) policy.preflight_secs = Number(preflightTimeout);
    return Object.keys(policy).length > 0 ? policy : null;
  }

  function checkIcon(check: PreflightCheck): string {
    if (check.passed) return "✓";
    if (check.required) return "✗";
    return "⚠";
  }

  function checkTone(check: PreflightCheck): string {
    if (check.passed) return "ok";
    if (check.required) return "fail";
    return "warn";
  }

  // ── Health badge helpers ────────────────────────────────────────────────────

  function breakerShortLabel(state: string): string {
    switch (state) {
      case "closed":
        return "Healthy";
      case "open":
        return "Connection refused";
      case "half_open":
        return "Probing";
      default:
        return state;
    }
  }

  function breakerLabel(state: string): string {
    switch (state) {
      case "closed":
        return "Healthy";
      case "open":
        return "Tripped — connection refused until reset window";
      case "half_open":
        return "Probing — next attempt will test connectivity";
      default:
        return state;
    }
  }

  function breakerTone(state: string): string {
    if (state === "closed") return "ok";
    if (state === "open") return "fail";
    return "warn";
  }

  /** Most recent activity timestamp across audit + saved config. */
  function lastCheckedMs(): number | null {
    if (!connectorStatus) return null;
    let latest = 0;
    for (const entry of connectorStatus.audit) {
      const ms = new Date(entry.timestamp).getTime();
      if (!Number.isNaN(ms) && ms > latest) latest = ms;
    }
    if (connectorStatus.config?.updated_at_ms) {
      latest = Math.max(latest, connectorStatus.config.updated_at_ms);
    }
    return latest || null;
  }

  function timeAgo(ms: number): string {
    const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
    if (s < 60) return "just now";
    const m = Math.floor(s / 60);
    if (m < 60) return `${m} min ago`;
    const h = Math.floor(m / 60);
    if (h < 24) return `${h} h ago`;
    return `${Math.floor(h / 24)} d ago`;
  }

  const modeOptions: { value: Mode; label: string; description: string }[] = [
    {
      value: "kubeconfig",
      label: "Existing kubeconfig",
      description: "Use a kubeconfig file already present on this machine.",
    },
    {
      value: "api_server_token",
      label: "API server + token",
      description: "Connect directly with an API server URL and a bearer token.",
    },
    {
      value: "in_cluster",
      label: "In-cluster ServiceAccount",
      description: "Use the pod's mounted service account (only valid inside a Kubernetes pod).",
    },
  ];
</script>

<SettingsPage
  id="ocp-settings"
  eyebrow="OpenShift / Kubernetes"
  title={displayName || "OCP Connector"}
  description="Configure cluster access, authentication, and safety policy."
>
  {#snippet status()}
    {#if connectorStatus}
      <div
        class="health-badge health-badge-{breakerTone(connectorStatus.breaker_state)}"
        title="Circuit breaker: {breakerLabel(connectorStatus.breaker_state)}"
      >
        <i class="health-dot" aria-hidden="true"></i>
        <span class="health-text">
          <strong>{breakerShortLabel(connectorStatus.breaker_state)}</strong>
          {#if lastCheckedMs()}
            <span class="health-last">Last checked {timeAgo(lastCheckedMs() ?? 0)}</span>
          {/if}
        </span>
      </div>
    {/if}
  {/snippet}

  <!-- 1. Connector Identity -->
  <SettingsSection
    title="Connection"
    description="Name this connector and choose its service type."
  >
    <SettingsCard tone="subtle">
      <SettingsRow columns={2}>
        <SettingsLabel text="Display name" forId="ocp-display-name">
          <SettingsInput>
            <input
              id="ocp-display-name"
              value={displayName}
              oninput={(e) => {
                displayName = e.currentTarget.value;
                markDirty();
              }}
              placeholder="Production OpenShift"
            />
          </SettingsInput>
        </SettingsLabel>
        <SettingsLabel text="Connection type" forId="ocp-connection-type">
          <SettingsInput>
            <select
              id="ocp-connection-type"
              value={kind}
              onchange={(e) => changeKind(e.currentTarget.value as McpServerSettings["kind"])}
            >
              <option value="generic">Generic MCP</option>
              <option value="jira">Jira</option>
              <option value="ocp">OpenShift / OCP</option>
              <option value="bamboo">Bamboo</option>
              <option value="bitbucket">Bitbucket</option>
            </select>
          </SettingsInput>
        </SettingsLabel>
      </SettingsRow>
    </SettingsCard>
  </SettingsSection>

  <!-- 2. Connection Method -->
  <SettingsSection
    title="Connection method"
    description="Choose how Spacesly authenticates to the cluster."
  >
    <SettingsCard>
      <div class="mode-cards" role="radiogroup" aria-label="Connection method">
        {#each modeOptions as opt (opt.value)}
          <label class="mode-card" class:selected={mode === opt.value}>
            <input
              type="radio"
              name="ocp-mode"
              value={opt.value}
              checked={mode === opt.value}
              onchange={() => {
                mode = opt.value;
                markDirty();
              }}
            />
            <div class="mode-card-body">
              <strong>{opt.label}</strong>
              <span>{opt.description}</span>
            </div>
          </label>
        {/each}
      </div>
    </SettingsCard>
  </SettingsSection>

  <!-- 3. Connection Details -->
  <SettingsSection
    title="Connection details"
    description={mode === "api_server_token"
      ? "Endpoint, namespace, and credentials for the cluster."
      : mode === "kubeconfig"
        ? "Path and optional context for the kubeconfig file."
        : "No credentials needed — Spacesly uses the in-cluster service account."}
  >
    <SettingsCard>
      {#if mode === "kubeconfig"}
        <SettingsRow columns={2}>
          <SettingsLabel text="Kubeconfig path" forId="ocp-kubeconfig-path">
            <SettingsInput>
              <input
                id="ocp-kubeconfig-path"
                class="field-mono"
                class:invalid={fieldError("kubeconfigPath")}
                value={kubeconfigPath}
                oninput={(e) => {
                  kubeconfigPath = e.currentTarget.value;
                  clearError("kubeconfigPath");
                  markDirty();
                }}
                onblur={() => {
                  markTouched("kubeconfigPath");
                  validate();
                }}
                placeholder="/home/user/.kube/config"
                aria-invalid={!!fieldError("kubeconfigPath")}
                aria-describedby="ocp-kubeconfig-path-err"
              />
            </SettingsInput>
            {#if fieldError("kubeconfigPath")}
              <p class="field-error" id="ocp-kubeconfig-path-err" role="alert">
                {fieldError("kubeconfigPath")}
              </p>
            {/if}
          </SettingsLabel>
          <SettingsLabel text="Context" forId="ocp-kubeconfig-context" optional>
            <SettingsInput>
              <input
                id="ocp-kubeconfig-context"
                value={kubeconfigContext}
                oninput={(e) => {
                  kubeconfigContext = e.currentTarget.value;
                  markDirty();
                }}
                placeholder="Leave blank to use current-context"
              />
            </SettingsInput>
          </SettingsLabel>
        </SettingsRow>
        <SettingsHelperText>
          The path must exist on <strong>this machine</strong>. Spacesly does not access remote or
          container filesystems.
        </SettingsHelperText>
      {:else if mode === "api_server_token"}
        <SettingsGroup title="API server">
          <SettingsRow columns={2}>
            <SettingsLabel text="API server URL" forId="ocp-server">
              <SettingsInput>
                <input
                  id="ocp-server"
                  class="field-mono"
                  class:invalid={fieldError("server")}
                  value={server}
                  oninput={(e) => {
                    server = e.currentTarget.value;
                    clearError("server");
                    markDirty();
                  }}
                  onblur={() => {
                    markTouched("server");
                    validate();
                  }}
                  placeholder="https://api.cluster.example:6443"
                  aria-invalid={!!fieldError("server")}
                  aria-describedby="ocp-server-err"
                />
              </SettingsInput>
              {#if fieldError("server")}
                <p class="field-error" id="ocp-server-err" role="alert">{fieldError("server")}</p>
              {/if}
            </SettingsLabel>
            <SettingsLabel text="Default namespace" forId="ocp-namespace" optional>
              <SettingsInput>
                <input
                  id="ocp-namespace"
                  value={defaultNamespace}
                  oninput={(e) => {
                    defaultNamespace = e.currentTarget.value;
                    markDirty();
                  }}
                  placeholder="default"
                />
              </SettingsInput>
            </SettingsLabel>
          </SettingsRow>
        </SettingsGroup>
        <SettingsGroup title="Credentials">
          <SettingsRow columns={2}>
            <SettingsLabel text="Bearer token" forId="ocp-token">
              <SettingsInput>
                <input
                  id="ocp-token"
                  type="password"
                  class:invalid={fieldError("token")}
                  value={token}
                  oninput={(e) => {
                    token = e.currentTarget.value;
                    clearError("token");
                    markDirty();
                  }}
                  onblur={() => {
                    markTouched("token");
                    validate();
                  }}
                  placeholder={secretStatus.token_set
                    ? "Saved securely. Enter a new token to replace it."
                    : "Paste token here"}
                  autocomplete="off"
                  aria-invalid={!!fieldError("token")}
                  aria-describedby="ocp-token-err"
                />
              </SettingsInput>
              {#if fieldError("token")}
                <p class="field-error" id="ocp-token-err" role="alert">{fieldError("token")}</p>
              {/if}
              {#if secretStatus.token_set && !token}
                <p class="secret-hint">Token saved. Leave blank to keep the existing token.</p>
              {/if}
            </SettingsLabel>
          </SettingsRow>
          <SettingsRow columns={1}>
            <SettingsLabel text="CA certificate (PEM)" forId="ocp-ca" optional>
              <SettingsInput wide>
                <textarea
                  id="ocp-ca"
                  value={caText}
                  oninput={(e) => {
                    caText = e.currentTarget.value;
                    markDirty();
                  }}
                  placeholder={secretStatus.ca_data_set
                    ? "CA certificate saved. Paste a new certificate to replace it."
                    : "-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----"}
                  class="ca-textarea"></textarea>
              </SettingsInput>
              {#if secretStatus.ca_data_set && !caText}
                <p class="secret-hint">
                  CA certificate saved. Leave blank to keep the existing certificate.
                </p>
              {/if}
              <SettingsHelperText>
                Required when the cluster uses a self-signed or internal CA. Paste the full PEM
                chain.
              </SettingsHelperText>
            </SettingsLabel>
          </SettingsRow>
        </SettingsGroup>
      {:else if mode === "in_cluster"}
        <div class="info-banner">
          <p class="banner-title">In-cluster ServiceAccount</p>
          <p>
            Spacesly will use the mounted service account token at
            <code>/var/run/secrets/kubernetes.io/serviceaccount/token</code>. No additional
            credentials are needed. This mode only works when Spacesly itself runs inside a
            Kubernetes pod.
          </p>
        </div>
      {/if}
    </SettingsCard>
  </SettingsSection>

  <!-- 4. Test Connection -->
  <SettingsSection
    title="Test connection"
    description="Validate every stage — from hostname resolution to RBAC — before going live."
  >
    <SettingsCard>
      <div class="test-actions">
        <button
          class="btn-primary"
          onclick={saveAndTest}
          disabled={!canTest}
          aria-busy={saving || preflightState === "running"}
        >
          {#if saving || preflightState === "running"}
            <span class="spinner" aria-hidden="true"></span>
            {saving ? "Saving…" : "Testing connection…"}
          {:else}
            Save &amp; test connection
          {/if}
        </button>
        <button class="btn-ghost" onclick={saveDraft} disabled={!canSave}>
          Save without testing
        </button>
      </div>

      {#if !requiredValid}
        <p class="test-hint">Complete the required fields above to save or test.</p>
      {/if}

      {#if saveError}
        <p class="test-status test-status-error" role="alert">{saveError}</p>
      {:else if saveSuccess}
        <p class="test-status test-status-success" role="status">Configuration saved.</p>
      {/if}

      <!-- Preflight stepper -->
      {#if preflightState !== "idle"}
        <div class="preflight-stepper" aria-live="polite" aria-label="Preflight progress">
          {#if preflightReport}
            {#each preflightReport.checks as check, i (check.name)}
              <div
                class={`preflight-step preflight-step-${checkTone(check)}`}
                style={`animation-delay: ${i * 55}ms`}
              >
                <span class="step-icon" aria-hidden="true">{checkIcon(check)}</span>
                <div class="step-body">
                  <span class="step-name">{preflightStageName(check.stage)} — {check.name}</span>
                  <span class="step-detail">{check.detail}</span>
                  {#if check.error_code}
                    <code class="step-code">{check.error_code}</code>
                  {/if}
                </div>
                {#if !check.passed && check.required}
                  <span class="step-badge step-badge-fail">Failed</span>
                {:else if !check.passed}
                  <span class="step-badge step-badge-warn">Warning</span>
                {/if}
                <span class="step-duration">{check.duration_ms}ms</span>
              </div>
            {/each}
            <div
              class={`preflight-summary ${preflightReport.passed ? "preflight-passed" : "preflight-failed"}`}
            >
              {#if preflightReport.passed}
                ✓ All checks passed
                {#if preflightReport.passed_with_warnings}
                  (with {preflightReport.checks.filter((c) => !c.passed).length} warning{preflightReport.checks.filter(
                    (c) => !c.passed,
                  ).length !== 1
                    ? "s"
                    : ""})
                {/if}
              {:else}
                ✗ {preflightReport.failed_required} required check{preflightReport.failed_required !==
                1
                  ? "s"
                  : ""} failed
              {/if}
              <span class="total-duration">Total: {preflightReport.total_duration_ms}ms</span>
            </div>
          {:else if preflightState === "running"}
            <div class="preflight-running">
              <span class="spinner" aria-hidden="true"></span>
              <span>Testing connection…</span>
            </div>
          {/if}
          {#if preflightError}
            <div class="preflight-step preflight-step-fail">
              <span class="step-icon" aria-hidden="true">✗</span>
              <span class="step-detail">{preflightError}</span>
            </div>
          {/if}
        </div>
      {/if}
    </SettingsCard>
  </SettingsSection>

  <!-- 5. Advanced Settings -->
  <SettingsSection title="Advanced" description="Timeout overrides, activity log, and danger zone.">
    <SettingsCard>
      <button
        class="btn-ghost advanced-toggle"
        onclick={() => (showAdvanced = !showAdvanced)}
        aria-expanded={showAdvanced}
      >
        {showAdvanced ? "Hide advanced settings" : "Show advanced settings"}
      </button>

      {#if showAdvanced}
        <SettingsGroup
          title="Timeout policy"
          description="Leave blank to use safe defaults (connect 10s, request 30s, preflight 60s)."
        >
          <SettingsRow columns={3}>
            <SettingsLabel text="Connect timeout (s)" forId="ocp-connect-timeout" optional>
              <SettingsInput>
                <input
                  id="ocp-connect-timeout"
                  type="number"
                  min="1"
                  max="60"
                  placeholder="10"
                  value={connectTimeout}
                  oninput={(e) => {
                    connectTimeout = e.currentTarget.value;
                    markDirty();
                  }}
                />
              </SettingsInput>
            </SettingsLabel>
            <SettingsLabel text="Request timeout (s)" forId="ocp-request-timeout" optional>
              <SettingsInput>
                <input
                  id="ocp-request-timeout"
                  type="number"
                  min="1"
                  max="300"
                  placeholder="30"
                  value={requestTimeout}
                  oninput={(e) => {
                    requestTimeout = e.currentTarget.value;
                    markDirty();
                  }}
                />
              </SettingsInput>
            </SettingsLabel>
            <SettingsLabel text="Preflight timeout (s)" forId="ocp-preflight-timeout" optional>
              <SettingsInput>
                <input
                  id="ocp-preflight-timeout"
                  type="number"
                  min="5"
                  max="300"
                  placeholder="60"
                  value={preflightTimeout}
                  oninput={(e) => {
                    preflightTimeout = e.currentTarget.value;
                    markDirty();
                  }}
                />
              </SettingsInput>
            </SettingsLabel>
          </SettingsRow>
        </SettingsGroup>

        {#if connectorStatus && connectorStatus.audit.length > 0}
          <SettingsGroup title="Recent activity">
            <details class="audit-log">
              <summary>Show activity log ({connectorStatus.audit.length} events)</summary>
              <ul class="audit-list">
                {#each connectorStatus.audit.slice(0, 10) as entry (entry.timestamp + entry.event)}
                  <li>
                    <span class="audit-ts">{new Date(entry.timestamp).toLocaleTimeString()}</span>
                    <span class={`audit-outcome audit-${entry.outcome}`}>{entry.outcome}</span>
                    <span class="audit-event">{entry.event}</span>
                    {#if entry.tool}<span class="audit-tool">{entry.tool}</span>{/if}
                  </li>
                {/each}
              </ul>
            </details>
          </SettingsGroup>
        {/if}

        <!-- Danger zone -->
        <SettingsGroup title="Danger zone">
          {#if !confirmDelete}
            <button class="btn-danger" onclick={() => (confirmDelete = true)}>
              Delete connector
            </button>
          {:else}
            <div class="confirm-delete">
              <p>
                This will permanently delete all credentials, configuration, and audit logs for this
                connector.
              </p>
              <div class="confirm-delete-actions">
                <button class="btn-danger" onclick={deleteConnector} disabled={deleting}>
                  {deleting ? "Deleting…" : "Yes, delete"}
                </button>
                <button class="btn-ghost" onclick={() => (confirmDelete = false)}>Cancel</button>
              </div>
            </div>
          {/if}
        </SettingsGroup>
      {/if}
    </SettingsCard>
  </SettingsSection>
</SettingsPage>

<style>
  /* ── Header health badge ── */
  .health-badge {
    display: inline-flex;
    align-items: center;
    gap: 9px;
    flex: 0 0 auto;
    border: 1px solid var(--border-subtle);
    border-radius: 999px;
    padding: 7px 12px;
    background: var(--surface-inset);
    white-space: nowrap;
  }

  .health-dot {
    width: 8px;
    height: 8px;
    flex-shrink: 0;
    border-radius: 50%;
    background: var(--text-muted);
  }

  .health-badge-ok .health-dot {
    background: var(--success);
    box-shadow: 0 0 0 3px var(--success-bg);
  }
  .health-badge-warn .health-dot {
    background: var(--warning);
    box-shadow: 0 0 0 3px var(--warning-bg);
  }
  .health-badge-fail .health-dot {
    background: var(--danger);
    box-shadow: 0 0 0 3px var(--danger-bg);
  }

  .health-text {
    display: grid;
    gap: 1px;
    min-width: 0;
  }
  .health-text strong {
    font-size: 10px;
    font-weight: 900;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-bright);
    line-height: 1.1;
  }
  .health-last {
    font-size: 10px;
    color: var(--text-secondary);
    line-height: 1.2;
  }

  @media (max-width: 600px) {
    .health-last {
      display: none;
    }
  }

  /* ── Mode radio cards ── */
  .mode-cards {
    display: grid;
    gap: 8px;
    grid-template-columns: repeat(3, 1fr);
  }

  @media (max-width: 760px) {
    .mode-cards {
      grid-template-columns: 1fr;
    }
  }

  .mode-card {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    border: 1px solid var(--border-subtle);
    border-radius: 10px;
    padding: 12px;
    background: var(--surface-inset);
    cursor: pointer;
    transition:
      border-color 0.12s,
      background 0.12s;
  }

  .mode-card.selected {
    border-color: var(--focus-border);
    background: color-mix(in srgb, var(--accent) 8%, var(--surface-inset));
  }

  .mode-card input[type="radio"] {
    width: auto;
    margin-top: 3px;
    accent-color: var(--accent);
    flex-shrink: 0;
    border: 0;
    padding: 0;
    background: none;
    box-shadow: none;
  }

  .mode-card-body {
    display: grid;
    gap: 3px;
    min-width: 0;
  }

  .mode-card-body strong {
    font-size: 13px;
    color: var(--text-bright);
  }

  .mode-card-body span {
    font-size: 12px;
    font-weight: 400;
    letter-spacing: 0;
    text-transform: none;
    margin: 0;
    color: var(--text-secondary);
    line-height: 1.4;
  }

  /* ── Inline field validation ── */
  .field-error {
    margin: 0;
    font-size: 11px;
    line-height: 1.4;
    color: var(--danger);
  }

  :global(.settings-form .settings-input input.invalid),
  :global(.settings-form .settings-input textarea.invalid) {
    border-color: var(--danger-border);
  }
  :global(.settings-form .settings-input input.invalid:focus),
  :global(.settings-form .settings-input textarea.invalid:focus) {
    border-color: var(--danger-border);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--danger) 22%, transparent);
  }

  /* ── Test actions ── */
  .test-actions {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
    margin-bottom: 12px;
  }

  .test-hint {
    margin: 0 0 8px;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .test-status {
    margin: 0 0 8px;
    font-size: 12px;
    font-weight: 600;
  }
  .test-status-error {
    color: var(--danger);
  }
  .test-status-success {
    color: var(--success);
  }

  /* ── Preflight stepper ── */
  .preflight-stepper {
    display: grid;
    gap: 6px;
    margin-top: 10px;
  }

  .preflight-step {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--surface-inset);
    border: 1px solid var(--border-subtle);
    font-size: 13px;
    animation: preflight-pop 0.18s ease both;
  }

  @keyframes preflight-pop {
    from {
      opacity: 0;
      transform: translateY(3px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }

  .preflight-step-ok {
    border-color: var(--success-border);
    background: var(--success-bg);
  }
  .preflight-step-fail {
    border-color: var(--danger-border);
    background: var(--danger-bg);
  }
  .preflight-step-warn {
    border-color: var(--warning-border);
    background: var(--warning-bg);
  }

  .preflight-running {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 9px 12px;
    border-radius: 8px;
    border: 1px dashed var(--border-subtle);
    background: var(--surface-inset);
    font-size: 13px;
    color: var(--text-secondary);
  }
  .preflight-running .spinner {
    color: var(--accent);
  }

  .step-icon {
    font-size: 14px;
    flex-shrink: 0;
    margin-top: 1px;
  }
  .step-body {
    flex: 1;
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .step-name {
    font-weight: 600;
    color: var(--text-bright);
  }
  .step-detail {
    color: var(--text-secondary);
    line-height: 1.4;
  }
  .step-code {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-muted);
    background: var(--surface-inset);
    border-radius: 4px;
    padding: 1px 5px;
    width: fit-content;
  }
  .step-duration {
    font-size: 11px;
    color: var(--text-muted);
    flex-shrink: 0;
    align-self: center;
    white-space: nowrap;
  }

  .step-badge {
    align-self: center;
    flex-shrink: 0;
    font-size: 10px;
    font-weight: 800;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    border-radius: 999px;
    padding: 2px 8px;
  }
  .step-badge-fail {
    background: var(--danger-bg);
    color: var(--danger);
  }
  .step-badge-warn {
    background: var(--warning-bg);
    color: var(--warning);
  }

  .preflight-summary {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 10px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 600;
    border: 1px solid transparent;
  }

  .preflight-passed {
    background: var(--success-bg);
    border-color: var(--success-border);
    color: var(--success);
  }
  .preflight-failed {
    background: var(--danger-bg);
    border-color: var(--danger-border);
    color: var(--danger);
  }
  .total-duration {
    font-weight: 400;
    font-size: 12px;
    color: var(--text-muted);
  }

  /* ── Audit log ── */
  .audit-log {
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    overflow: hidden;
  }

  .audit-log summary {
    padding: 8px 12px;
    font-size: 12px;
    color: var(--text-secondary);
    cursor: pointer;
    background: var(--surface-inset);
    user-select: none;
  }

  .audit-log summary::-webkit-details-marker {
    display: none;
  }

  .audit-list {
    margin: 0;
    padding: 0;
    list-style: none;
    max-height: 220px;
    overflow-y: auto;
  }

  .audit-list li {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 6px 12px;
    border-top: 1px solid var(--border-subtle);
    font-size: 12px;
    font-family: var(--font-mono);
  }

  .audit-ts {
    color: var(--text-muted);
    flex-shrink: 0;
  }
  .audit-event {
    color: var(--text-primary);
  }
  .audit-tool {
    color: var(--text-secondary);
    font-style: italic;
  }
  .audit-outcome {
    font-weight: 700;
    flex-shrink: 0;
  }
  .audit-passed,
  .audit-success {
    color: var(--success);
  }
  .audit-failed {
    color: var(--danger);
  }
  .audit-started {
    color: var(--text-muted);
  }

  /* ── Info banner ── */
  .info-banner {
    border: 1px solid var(--info-border);
    border-radius: 10px;
    padding: 14px;
    background: var(--info-bg);
  }

  .banner-title {
    margin: 0 0 6px;
    font-size: 13px;
    font-weight: 700;
    color: var(--info);
  }

  .info-banner p {
    margin: 0;
    font-size: 13px;
    color: var(--text-secondary);
    line-height: 1.5;
  }

  .info-banner code {
    font-family: var(--font-mono);
    font-size: 12px;
    background: var(--surface-inset);
    border-radius: 4px;
    padding: 1px 5px;
  }

  /* ── Secret hint ── */
  .secret-hint {
    margin: 4px 0 0;
    font-size: 12px;
    color: var(--text-muted);
    font-style: italic;
  }

  /* ── CA textarea ── */
  .ca-textarea {
    min-height: 110px;
    font-family: var(--font-mono);
    font-size: 12px;
    resize: vertical;
    width: 100%;
  }

  /* ── Advanced toggle ── */
  .advanced-toggle {
    margin-bottom: 14px;
    font-size: 13px;
    color: var(--text-link);
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    text-decoration: underline;
    text-underline-offset: 3px;
  }

  /* ── Confirm delete ── */
  .confirm-delete {
    border: 1px solid var(--danger-border);
    border-radius: 9px;
    padding: 14px;
    background: var(--danger-bg);
    display: grid;
    gap: 12px;
  }

  .confirm-delete p {
    margin: 0;
    font-size: 13px;
    color: var(--text-primary);
  }

  .confirm-delete-actions {
    display: flex;
    gap: 10px;
  }

  /* ── Buttons ── */
  .btn-primary,
  .btn-ghost,
  .btn-danger {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border-radius: 8px;
    padding: 8px 14px;
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.1s;
    border: 1px solid transparent;
  }

  .btn-primary {
    background: var(--accent);
    color: var(--bg-base);
    border-color: var(--accent);
  }

  .btn-ghost {
    background: var(--surface-inset);
    color: var(--text-primary);
    border-color: var(--border-subtle);
  }

  .btn-danger {
    background: var(--danger-bg);
    color: var(--danger);
    border-color: var(--danger-border);
  }

  .btn-primary:disabled,
  .btn-ghost:disabled,
  .btn-danger:disabled {
    opacity: var(--disabled-opacity);
    cursor: not-allowed;
  }

  .btn-primary:hover:not(:disabled) {
    opacity: 0.88;
  }
  .btn-ghost:hover:not(:disabled) {
    background: var(--surface-hover);
  }
  .btn-danger:hover:not(:disabled) {
    opacity: 0.88;
  }

  /* ── Spinner ── */
  .spinner {
    width: 14px;
    height: 14px;
    flex: 0 0 auto;
    border-radius: 50%;
    border: 2px solid color-mix(in srgb, currentColor 25%, transparent);
    border-top-color: currentColor;
    animation: ocp-spin 0.7s linear infinite;
  }

  .btn-primary .spinner {
    color: var(--bg-base);
  }

  @keyframes ocp-spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
