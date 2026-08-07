<script lang="ts">
  import { ChevronDown, Route, SlidersHorizontal } from "lucide-svelte";
  import { defaultMcpIntentMetadata, parseEnvText, type McpServerSettings } from "$lib/settings";
  import SettingsDescription from "$lib/components/settings/SettingsDescription.svelte";
  import SettingsHelperText from "$lib/components/settings/SettingsHelperText.svelte";
  import SettingsInput from "$lib/components/settings/SettingsInput.svelte";
  import SettingsLabel from "$lib/components/settings/SettingsLabel.svelte";
  import SettingsPage from "$lib/components/settings/SettingsPage.svelte";
  import SettingsRow from "$lib/components/settings/SettingsRow.svelte";
  import TagEditor from "$lib/components/settings/TagEditor.svelte";

  let {
    server,
    jiraBaseUrl = "",
    jiraPrincipal = "",
    jiraAuthMode = "api_token",
    configuredEnvKeys = [],
    onUpdate,
    onError,
  }: {
    server: McpServerSettings;
    jiraBaseUrl?: string;
    jiraPrincipal?: string;
    jiraAuthMode?: "api_token" | "pat" | "password";
    configuredEnvKeys?: string[];
    onUpdate: (values: Partial<McpServerSettings>) => void;
    onError: (message: string | null) => void;
  } = $props();

  const typeNames: Record<McpServerSettings["kind"], string> = {
    generic: "Generic MCP",
    jira: "Jira",
    ocp: "OpenShift / OCP",
    bamboo: "Bamboo",
    bitbucket: "Bitbucket",
  };

  let hasRouting = $derived(server.domains.length > 0 || server.intentTerms.length > 0);
  let commandPreview = $derived(
    [
      ...(server.command ? [quoteArgument(server.command)] : []),
      ...server.args.map(quoteArgument),
    ].join(" "),
  );

  function quoteArgument(value: string): string {
    if (value.length === 0) return '""';
    return /\s/.test(value) ? `"${value.replaceAll('"', '\\"')}"` : value;
  }

  function updateCommand(value: string) {
    onUpdate({ command: value.trim() });
  }

  function updateEnv(value: string) {
    try {
      onError(null);
      onUpdate({ env: parseEnvText(value) });
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  function updateEnvKey(key: string, value: string) {
    onUpdate({ env: { ...server.env, [key]: value } });
  }

  function envValue(key: string): string {
    return server.env[key] ?? "";
  }

  function envPlaceholder(key: string, placeholder: string): string {
    return configuredEnvKeys.includes(key)
      ? "Saved securely — enter a new value to replace it"
      : placeholder;
  }

  function genericEnvPlaceholder(): string {
    return configuredEnvKeys.length
      ? `Saved securely: ${configuredEnvKeys.join(", ")}`
      : "API_URL=https://service.company.id\nAPI_TOKEN=...";
  }
</script>

<SettingsPage
  id="mcp-settings"
  eyebrow="Connector setup"
  title={server.name || "New MCP connector"}
  description={`Configure how Spacesly starts and routes this ${typeNames[server.kind]} connector.`}
>
  <div class="connector-flow">
    <section class="setup-card setup-card-primary" aria-labelledby="connection-heading">
      <header class="setup-card-header">
        <span class="step-number" aria-hidden="true">1</span>
        <div>
          <h4 id="connection-heading">Connection</h4>
          <p>Name the connector and choose the service it connects to.</p>
        </div>
        <span class="setup-state">Required</span>
      </header>
      <div class="setup-card-body">
        <SettingsRow>
          <SettingsLabel text="Connection name" forId="mcp-connection-name">
            <SettingsInput>
              <input
                id="mcp-connection-name"
                autocomplete="off"
                placeholder="e.g. Team Bitbucket"
                value={server.name}
                oninput={(event) => onUpdate({ name: event.currentTarget.value })}
              />
            </SettingsInput>
          </SettingsLabel>
          <SettingsLabel text="Connection type" forId="mcp-connection-type">
            <SettingsInput>
              <select
                id="mcp-connection-type"
                value={server.kind}
                oninput={(event) => {
                  const kind = event.currentTarget.value as McpServerSettings["kind"];
                  onUpdate({ kind, ...defaultMcpIntentMetadata(kind, server.name) });
                }}
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
      </div>
    </section>

    <section class="setup-card" aria-labelledby="execution-heading">
      <header class="setup-card-header">
        <span class="step-number" aria-hidden="true">2</span>
        <div>
          <h4 id="execution-heading">Launch command</h4>
          <p>The local stdio process Spacesly should start.</p>
        </div>
      </header>
      <div class="setup-card-body execution-fields">
        <SettingsLabel text="Executable" forId="mcp-command">
          <SettingsInput>
            <input
              id="mcp-command"
              class="field-mono"
              autocomplete="off"
              placeholder="npx"
              value={server.command}
              oninput={(event) => updateCommand(event.currentTarget.value)}
            />
          </SettingsInput>
        </SettingsLabel>
        <SettingsLabel text="Arguments" forId="mcp-args" optional>
          <TagEditor
            id="mcp-args"
            variant="list"
            value={server.args}
            onChange={(args) => onUpdate({ args })}
            allowCommaSplit={false}
            placeholder="Add argument"
          />
        </SettingsLabel>
        <div class="command-preview" aria-label={`Command preview: ${commandPreview}`}>
          <span>Preview</span>
          <div class="command-preview-line" role="textbox" aria-readonly="true" tabindex="0">
            <span aria-hidden="true">$</span>
            <code>{commandPreview}</code>
          </div>
        </div>
      </div>
    </section>

    {#if server.kind === "jira"}
      <section class="setup-card" aria-labelledby="service-heading">
        <header class="setup-card-header">
          <span class="step-number" aria-hidden="true">3</span>
          <div>
            <h4 id="service-heading">Jira account</h4>
            <p>Credentials are shared with Jira board sync.</p>
          </div>
          <span class="inherited-badge">Inherited</span>
        </header>
        <div class="setup-card-body">
          <SettingsDescription>
            Update these values from the Jira settings page. Spacesly securely injects them when
            this connector starts.
          </SettingsDescription>
          <div class="account-summary">
            <span><small>Site</small><code>{jiraBaseUrl || "Not configured"}</code></span>
            <span
              ><small>Principal</small><code
                >{jiraPrincipal || (jiraAuthMode === "pat" ? "PAT only" : "Not configured")}</code
              ></span
            >
            <span
              ><small>Authentication</small><code
                >{jiraAuthMode === "api_token"
                  ? "API token"
                  : jiraAuthMode === "pat"
                    ? "Personal access token"
                    : "Password"}</code
              ></span
            >
          </div>
        </div>
      </section>
    {:else if server.kind === "bamboo"}
      <section class="setup-card" aria-labelledby="service-heading">
        <header class="setup-card-header">
          <span class="step-number" aria-hidden="true">3</span>
          <div>
            <h4 id="service-heading">Bamboo account</h4>
            <p>Only Bamboo connection values are shown.</p>
          </div>
          <span class="secure-badge">Stored securely</span>
        </header>
        <div class="setup-card-body service-fields">
          <SettingsLabel text="Bamboo URL" forId="mcp-bamboo-url">
            <SettingsInput
              ><input
                id="mcp-bamboo-url"
                class="field-mono"
                placeholder={envPlaceholder("BAMBOO_URL", "https://bamboo.company.id")}
                value={envValue("BAMBOO_URL")}
                oninput={(event) => updateEnvKey("BAMBOO_URL", event.currentTarget.value)}
              /></SettingsInput
            >
          </SettingsLabel>
          <SettingsRow>
            <SettingsLabel text="Username" forId="mcp-bamboo-user"
              ><SettingsInput
                ><input
                  id="mcp-bamboo-user"
                  autocomplete="username"
                  placeholder={envPlaceholder("BAMBOO_USERNAME", "username")}
                  value={envValue("BAMBOO_USERNAME")}
                  oninput={(event) => updateEnvKey("BAMBOO_USERNAME", event.currentTarget.value)}
                /></SettingsInput
              ></SettingsLabel
            >
            <SettingsLabel text="Access token" forId="mcp-bamboo-token"
              ><SettingsInput
                ><input
                  id="mcp-bamboo-token"
                  type="password"
                  autocomplete="new-password"
                  placeholder={envPlaceholder("BAMBOO_TOKEN", "Token")}
                  value={envValue("BAMBOO_TOKEN")}
                  oninput={(event) => updateEnvKey("BAMBOO_TOKEN", event.currentTarget.value)}
                /></SettingsInput
              ></SettingsLabel
            >
          </SettingsRow>
        </div>
      </section>
    {:else if server.kind === "bitbucket"}
      <section class="setup-card" aria-labelledby="service-heading">
        <header class="setup-card-header">
          <span class="step-number" aria-hidden="true">3</span>
          <div>
            <h4 id="service-heading">Bitbucket account</h4>
            <p>Only Bitbucket connection values are shown.</p>
          </div>
          <span class="secure-badge">Stored securely</span>
        </header>
        <div class="setup-card-body service-fields">
          <SettingsLabel text="Bitbucket URL" forId="mcp-bitbucket-url"
            ><SettingsInput
              ><input
                id="mcp-bitbucket-url"
                class="field-mono"
                placeholder={envPlaceholder("BITBUCKET_URL", "https://bitbucket.company.id")}
                value={envValue("BITBUCKET_URL")}
                oninput={(event) => updateEnvKey("BITBUCKET_URL", event.currentTarget.value)}
              /></SettingsInput
            ></SettingsLabel
          >
          <SettingsRow>
            <SettingsLabel text="Username" forId="mcp-bitbucket-user"
              ><SettingsInput
                ><input
                  id="mcp-bitbucket-user"
                  autocomplete="username"
                  placeholder={envPlaceholder("BITBUCKET_USERNAME", "username")}
                  value={envValue("BITBUCKET_USERNAME")}
                  oninput={(event) => updateEnvKey("BITBUCKET_USERNAME", event.currentTarget.value)}
                /></SettingsInput
              ></SettingsLabel
            >
            <SettingsLabel text="Access token" forId="mcp-bitbucket-token"
              ><SettingsInput
                ><input
                  id="mcp-bitbucket-token"
                  type="password"
                  autocomplete="new-password"
                  placeholder={envPlaceholder("BITBUCKET_TOKEN", "App password or token")}
                  value={envValue("BITBUCKET_TOKEN")}
                  oninput={(event) => updateEnvKey("BITBUCKET_TOKEN", event.currentTarget.value)}
                /></SettingsInput
              ></SettingsLabel
            >
          </SettingsRow>
        </div>
      </section>
    {:else if server.kind === "generic"}
      <details class="setup-card setup-disclosure" open={Object.keys(server.env).length > 0}>
        <summary>
          <span class="summary-icon" aria-hidden="true"
            ><SlidersHorizontal size={12} strokeWidth={2} /></span
          >
          <span
            ><strong>Environment variables</strong><small>Advanced process configuration</small
            ></span
          >
          <span class="summary-action"
            >Advanced <ChevronDown class="disclosure-chevron" size={13} strokeWidth={2} /></span
          >
        </summary>
        <div class="setup-card-body disclosure-body">
          <SettingsLabel text="Environment" forId="mcp-env" optional>
            <SettingsInput>
              <textarea
                id="mcp-env"
                class="env-config"
                rows="3"
                placeholder={genericEnvPlaceholder()}
                oninput={(event) => updateEnv(event.currentTarget.value)}
                value={Object.entries(server.env)
                  .map(([key, value]) => `${key}=${value}`)
                  .join("\n")}></textarea>
            </SettingsInput>
          </SettingsLabel>
          <SettingsHelperText
            >One <code>KEY=value</code> per line. Saved secret values remain unchanged when omitted.</SettingsHelperText
          >
        </div>
      </details>
    {/if}

    <details class="setup-card setup-disclosure" open={hasRouting}>
      <summary>
        <span class="summary-icon" aria-hidden="true"><Route size={12} strokeWidth={2} /></span>
        <span>
          <strong>When should this connector be used?</strong>
          <small
            >{hasRouting
              ? `${server.domains.length + server.intentTerms.length} routing signals configured`
              : "Optional agent routing"}</small
          >
        </span>
        <span class="summary-action"
          >Routing <ChevronDown class="disclosure-chevron" size={13} strokeWidth={2} /></span
        >
      </summary>
      <div class="setup-card-body disclosure-body routing-fields">
        <SettingsLabel text="Agent domains" forId="mcp-domains" optional>
          <TagEditor
            id="mcp-domains"
            value={server.domains}
            onChange={(domains) => onUpdate({ domains })}
            lowercase
            placeholder="Add a domain"
          />
          <SettingsHelperText
            >Broad areas such as <code>git</code>, <code>deployments</code>, or
            <code>tickets</code>.</SettingsHelperText
          >
        </SettingsLabel>
        <SettingsLabel text="Task intents" forId="mcp-intents" optional>
          <TagEditor
            id="mcp-intents"
            value={server.intentTerms}
            onChange={(intentTerms) => onUpdate({ intentTerms })}
            lowercase
            placeholder="Add an intent"
          />
          <SettingsHelperText
            >Phrases users might ask for, such as <code>review pull request</code
            >.</SettingsHelperText
          >
        </SettingsLabel>
      </div>
    </details>
  </div>
</SettingsPage>

<style>
  .connector-flow {
    display: grid;
    gap: 10px;
  }
  .setup-card {
    min-width: 0;
    overflow: hidden;
    border: 1px solid var(--border-subtle);
    border-radius: 11px;
    background: var(--surface-raised);
    transition:
      border-color 120ms ease,
      background 120ms ease;
  }
  .setup-card:hover {
    border-color: var(--border-strong);
  }
  .setup-card-primary {
    border-color: var(--border-strong);
  }
  .setup-card-header {
    display: grid;
    grid-template-columns: 26px minmax(0, 1fr) auto;
    align-items: center;
    gap: 10px;
    padding: 11px 13px;
    border-bottom: 1px solid var(--border-subtle);
  }
  .setup-card-header > div {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .setup-card-header h4,
  .setup-card-header p {
    margin: 0;
  }
  .setup-card-header h4 {
    color: var(--text-bright);
    font-size: 13px;
  }
  .setup-card-header p {
    color: var(--text-secondary);
    font-size: 11.5px;
    line-height: 1.35;
  }
  .step-number,
  .summary-icon {
    display: grid;
    width: 24px;
    height: 24px;
    place-items: center;
    border-radius: 7px;
    background: var(--surface-selected);
    color: var(--accent);
    font-size: 11px;
    font-weight: 900;
  }
  .setup-state,
  .inherited-badge,
  .secure-badge,
  .summary-action {
    color: var(--text-dim);
    font-size: 10px;
    font-weight: 800;
    white-space: nowrap;
  }
  .inherited-badge,
  .secure-badge {
    border: 1px solid var(--info-border);
    border-radius: 999px;
    padding: 4px 7px;
    background: var(--info-bg);
    color: var(--info);
  }
  .secure-badge {
    border-color: var(--success-border);
    background: var(--success-bg);
    color: var(--success);
  }
  .setup-card-body {
    display: grid;
    gap: 10px;
    padding: 12px 13px 13px 49px;
  }
  .execution-fields {
    grid-template-columns: minmax(0, 1fr);
    gap: 11px;
  }
  .command-preview {
    display: grid;
    min-width: 0;
    gap: 6px;
    border-top: 1px solid var(--border-subtle);
    padding-top: 11px;
  }
  .command-preview > span {
    color: var(--text-secondary);
    font-size: 11.5px;
    font-weight: 700;
  }
  .command-preview-line {
    display: flex;
    min-width: 0;
    gap: 8px;
    overflow-x: auto;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    padding: 8px 10px;
    background: var(--code-inline-bg);
    color: var(--text-secondary);
    scrollbar-width: thin;
  }
  .command-preview-line:focus-visible {
    border-color: var(--focus-border);
    box-shadow: 0 0 0 3px var(--focus-ring);
    outline: none;
  }
  .command-preview-line > span {
    flex: 0 0 auto;
    color: var(--text-dim);
    font-family: var(--font-mono);
  }
  .command-preview-line code {
    min-width: max-content;
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 12px;
    white-space: pre;
  }
  .account-summary {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 8px;
  }
  .account-summary > span {
    display: grid;
    min-width: 0;
    gap: 3px;
  }
  .account-summary small {
    color: var(--text-dim);
    font-size: 10px;
  }
  .account-summary code {
    overflow: hidden;
    color: var(--text-primary);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .service-fields {
    gap: 10px;
  }
  .setup-disclosure summary {
    display: grid;
    grid-template-columns: 26px minmax(0, 1fr) auto;
    align-items: center;
    gap: 10px;
    padding: 11px 13px;
    cursor: pointer;
    list-style: none;
  }
  .setup-disclosure summary::-webkit-details-marker {
    display: none;
  }
  .setup-disclosure summary > span:nth-child(2) {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .setup-disclosure summary strong {
    color: var(--text-primary);
    font-size: 12.5px;
  }
  .setup-disclosure summary small {
    color: var(--text-secondary);
    font-size: 11px;
  }
  .summary-action {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .setup-disclosure :global(.disclosure-chevron) {
    transition: transform 120ms ease;
  }
  .setup-disclosure[open] :global(.disclosure-chevron) {
    transform: rotate(180deg);
  }
  .disclosure-body {
    border-top: 1px solid var(--border-subtle);
  }
  .routing-fields {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .env-config {
    min-height: 74px !important;
  }

  @media (max-width: 700px) {
    .routing-fields,
    .account-summary {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  @media (max-width: 520px) {
    .setup-card-header {
      grid-template-columns: 26px minmax(0, 1fr);
    }
    .setup-card-header > :last-child {
      grid-column: 2;
      justify-self: start;
    }
    .setup-card-body {
      padding-left: 13px;
    }
    .setup-state,
    .inherited-badge,
    .secure-badge {
      display: none;
    }
  }
</style>
