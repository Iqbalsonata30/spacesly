<script lang="ts">
  import {
    parseArgsText,
    parseCommandText,
    parseEnvText,
    type McpServerSettings,
  } from "$lib/settings";

  let {
    server,
    jiraBaseUrl = "",
    jiraPrincipal = "",
    jiraAuthMode = "api_token",
    onUpdate,
    onError,
  }: {
    server: McpServerSettings;
    jiraBaseUrl?: string;
    jiraPrincipal?: string;
    jiraAuthMode?: "api_token" | "pat" | "password";
    onUpdate: (values: Partial<McpServerSettings>) => void;
    onError: (message: string | null) => void;
  } = $props();

  function updateCommand(value: string) {
    const parsed = parseCommandText(value);
    onUpdate({ command: parsed.command, args: parsed.args });
  }

  function updateArgs(value: string) {
    try {
      onError(null);
      onUpdate({ args: parseArgsText(value) });
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  function updateEnv(value: string) {
    try {
      onError(null);
      onUpdate({ env: { ...server.env, ...parseEnvText(value) } });
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  function updateEnvKey(key: string, value: string) {
    onUpdate({
      env: {
        ...server.env,
        [key]: value,
      },
    });
  }

  function envValue(key: string): string {
    return server.env[key] ?? "";
  }
</script>

<section class="settings-section">
  <div>
    <p class="section-kicker">MCP Connection</p>
    <h3>Server runtime</h3>
  </div>

  <label>
    <span>Connection Name</span>
    <input value={server.name} oninput={(event) => onUpdate({ name: event.currentTarget.value })} />
  </label>

  <label>
    <span>Connection Type</span>
    <select
      value={server.kind}
      oninput={(event) =>
        onUpdate({ kind: event.currentTarget.value as McpServerSettings["kind"] })}
    >
      <option value="generic">Generic MCP</option>
      <option value="jira">Jira</option>
      <option value="ocp">OpenShift / OCP</option>
      <option value="bamboo">Bamboo</option>
    </select>
  </label>

  <label>
    <span>Command or command line</span>
    <input
      placeholder="uvx mcp-server-package"
      value={server.command}
      oninput={(event) => updateCommand(event.currentTarget.value)}
    />
  </label>

  <label>
    <span>Arguments</span>
    <textarea
      placeholder={JSON.stringify(["--transport", "stdio"])}
      oninput={(event) => updateArgs(event.currentTarget.value)}
      value={JSON.stringify(server.args)}></textarea>
  </label>

  <div class="type-config">
    {#if server.kind === "jira"}
      <div class="inherited-card">
        <div>
          <p class="section-kicker">Jira Identity</p>
          <h3>Inherited from Jira Sync</h3>
        </div>
        <p>
          Jira MCP uses the same account as board sync. Configure credentials once in the Jira tab;
          Spacesly injects the right environment variables when this MCP server starts.
        </p>
        <div class="inherited-grid">
          <span>
            <strong>Site</strong>
            <code>{jiraBaseUrl || "Not configured"}</code>
          </span>
          <span>
            <strong>Principal</strong>
            <code>{jiraPrincipal || (jiraAuthMode === "pat" ? "PAT only" : "Not configured")}</code>
          </span>
          <span>
            <strong>Auth</strong>
            <code
              >{jiraAuthMode === "api_token"
                ? "API token"
                : jiraAuthMode === "pat"
                  ? "Personal access token"
                  : "Username + password"}</code
            >
          </span>
        </div>
      </div>
    {:else if server.kind === "ocp"}
      <p class="section-kicker">OpenShift / Kubernetes MCP Config</p>
      <label>
        <span>Kubeconfig Path</span>
        <input
          placeholder="/home/user/.kube/config"
          value={envValue("KUBECONFIG")}
          oninput={(event) => updateEnvKey("KUBECONFIG", event.currentTarget.value)}
        />
      </label>
      <div class="field-row">
        <label>
          <span>Cluster API Server</span>
          <input
            placeholder="https://api.cluster:6443"
            value={envValue("OPENSHIFT_SERVER")}
            oninput={(event) => updateEnvKey("OPENSHIFT_SERVER", event.currentTarget.value)}
          />
        </label>
        <label>
          <span>Token</span>
          <input
            type="password"
            placeholder="OpenShift/Kubernetes token"
            value={envValue("OPENSHIFT_TOKEN")}
            oninput={(event) => updateEnvKey("OPENSHIFT_TOKEN", event.currentTarget.value)}
          />
        </label>
      </div>
    {:else if server.kind === "bamboo"}
      <p class="section-kicker">Bamboo MCP Config</p>
      <label>
        <span>Bamboo URL</span>
        <input
          placeholder="https://bamboo.company.id"
          value={envValue("BAMBOO_URL")}
          oninput={(event) => updateEnvKey("BAMBOO_URL", event.currentTarget.value)}
        />
      </label>
      <div class="field-row">
        <label>
          <span>Username</span>
          <input
            placeholder="user"
            value={envValue("BAMBOO_USERNAME")}
            oninput={(event) => updateEnvKey("BAMBOO_USERNAME", event.currentTarget.value)}
          />
        </label>
        <label>
          <span>Token</span>
          <input
            type="password"
            placeholder="Bamboo token"
            value={envValue("BAMBOO_TOKEN")}
            oninput={(event) => updateEnvKey("BAMBOO_TOKEN", event.currentTarget.value)}
          />
        </label>
      </div>
    {:else}
      <p class="section-kicker">Generic MCP Config</p>
      <label>
        <span>Environment</span>
        <textarea
          class="env-config"
          placeholder="API_URL=https://service.company.id&#10;API_TOKEN=..."
          oninput={(event) => updateEnv(event.currentTarget.value)}
          value={Object.entries(server.env)
            .map(([key, value]) => `${key}=${value}`)
            .join("\n")}></textarea>
      </label>
    {/if}
  </div>
</section>

<style>
  .settings-section,
  .type-config {
    display: grid;
    gap: 14px;
  }

  .settings-section {
    border: 1px solid #2a2832;
    border-radius: 12px;
    padding: 14px;
    background: rgba(17, 16, 22, 0.56);
  }

  .type-config {
    border-top: 1px solid #2a2832;
    padding-top: 14px;
  }

  .section-kicker,
  label span {
    margin: 0 0 6px;
    color: #8f88a8;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  h3 {
    margin: 0;
    color: #f1edf5;
  }

  label {
    display: grid;
    gap: 6px;
  }

  input,
  select,
  textarea {
    color-scheme: dark;
    border: 1px solid #25232c;
    border-radius: 9px;
    padding: 11px 12px;
    background: linear-gradient(180deg, #0b0b10, #09090d);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.025),
      0 1px 0 rgba(0, 0, 0, 0.38);
    color: #cfc8dc;
    font: inherit;
  }

  select {
    height: 42px;
    appearance: none;
    background-image:
      linear-gradient(45deg, transparent 50%, #716a83 50%),
      linear-gradient(135deg, #716a83 50%, transparent 50%),
      linear-gradient(180deg, #0b0b10, #09090d);
    background-position:
      calc(100% - 18px) 18px,
      calc(100% - 13px) 18px,
      0 0;
    background-size:
      5px 5px,
      5px 5px,
      100% 100%;
    background-repeat: no-repeat;
    padding-right: 34px;
  }

  input:focus,
  select:focus,
  textarea:focus {
    border-color: rgba(184, 214, 228, 0.42);
    box-shadow:
      0 0 0 3px rgba(184, 214, 228, 0.08),
      inset 0 1px 0 rgba(255, 255, 255, 0.03);
    outline: none;
  }

  input::placeholder,
  textarea::placeholder {
    color: #4e485c;
  }

  textarea {
    min-height: 86px;
    resize: vertical;
    font-family: var(--font-mono);
    font-size: 13px;
    line-height: 1.45;
  }

  textarea.env-config {
    min-height: 118px;
  }

  .inherited-card {
    display: grid;
    gap: 12px;
    border: 1px solid rgba(184, 214, 228, 0.18);
    border-radius: 14px;
    padding: 14px;
    background:
      radial-gradient(circle at top right, rgba(184, 214, 228, 0.12), transparent 34%),
      linear-gradient(135deg, rgba(37, 42, 49, 0.72), rgba(17, 16, 22, 0.74));
  }

  .inherited-card p {
    margin: 0;
    color: #aaa1c3;
    font-size: 13px;
    line-height: 1.45;
  }

  .inherited-grid {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 10px;
  }

  .inherited-grid span {
    display: grid;
    gap: 5px;
    min-width: 0;
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 10px;
    padding: 10px;
    background: rgba(9, 9, 13, 0.42);
  }

  .inherited-grid strong {
    color: #8f88a8;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }

  .inherited-grid code {
    overflow: hidden;
    color: #b8d6e4;
    font-family: var(--font-mono);
    font-size: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .field-row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }

  @media (max-width: 860px) {
    .inherited-grid,
    .field-row {
      grid-template-columns: 1fr;
    }
  }
</style>
