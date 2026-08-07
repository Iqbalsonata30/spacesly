<script lang="ts">
  import "../page.css";
  import McpConnectionSettings from "$lib/components/McpConnectionSettings.svelte";
  import type { McpServerSettings } from "$lib/settings";

  let server = $state<McpServerSettings>({
    id: "mcp-test",
    name: "Bitbucket Tools",
    kind: "generic",
    command: "uvx",
    args: ["--transport", "stdio"],
    env: {
      API_URL: "https://service.company.id",
      API_TOKEN: "secret",
    },
    domains: ["bitbucket", "git", "pull request", "repository"],
    intentTerms: ["deploy", "prerelease", "pod logs"],
  });

  function update(values: Partial<McpServerSettings>) {
    server = { ...server, ...values };
  }
</script>

<svelte:head>
  <title>MCP Settings Responsiveness Harness</title>
</svelte:head>

<!--
  Test-only harness: renders the generic MCP connector settings page in a plain
  browser so Playwright can verify responsiveness and the tag editors. Not part
  of the shipped app UI. No IPC calls are needed by this component.
-->
<div class="resp-scaffold">
  <form class="settings-form" onsubmit={(event) => event.preventDefault()}>
    <McpConnectionSettings
      {server}
      jiraBaseUrl="https://company.atlassian.net"
      jiraPrincipal="bot@company.id"
      jiraAuthMode="api_token"
      configuredEnvKeys={["API_TOKEN"]}
      onUpdate={update}
      onError={() => {}}
    />
  </form>
</div>

<style>
  .resp-scaffold {
    min-height: 100vh;
  }
</style>
