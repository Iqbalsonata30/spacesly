<script lang="ts">
  import "../page.css";
  import OcpConnectorSettings from "$lib/components/OcpConnectorSettings.svelte";
</script>

<svelte:head>
  <title>OCP Settings Responsiveness Harness</title>
</svelte:head>

<!--
  Test-only harness: renders the OCP connector settings page in a plain browser
  so Playwright can verify responsiveness. Not part of the shipped app UI.
  OCP IPC commands are stubbed via window.__TAURI_INTERNALS__.invoke in the test.
-->
<div class="resp-scaffold">
  <form class="settings-form" onsubmit={(event) => event.preventDefault()}>
    <OcpConnectorSettings serverId="ocp-test" serverName="Production OpenShift" kind="ocp" />
  </form>
</div>

<style>
  .resp-scaffold {
    min-height: 100vh;
  }
</style>
