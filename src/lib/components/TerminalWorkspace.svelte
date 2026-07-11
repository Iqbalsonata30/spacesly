<script lang="ts">
  type Props = {
    workdir: string;
    opened: boolean;
    onWorkdirChange: (workdir: string) => void;
    onContainerReady: (container: HTMLDivElement | null) => void;
  };

  let { workdir, opened, onWorkdirChange, onContainerReady }: Props = $props();
  let container: HTMLDivElement | null = $state(null);

  $effect(() => {
    onContainerReady(container);
    return () => onContainerReady(null);
  });
</script>

<section class="workspace-terminal-pane" aria-label="Local shell terminal">
  <header>
    <div>
      <p>Local Shell</p>
      <h2>Workspace Terminal</h2>
    </div>
    <input
      aria-label="Terminal working directory"
      placeholder="Initial directory (optional)"
      value={workdir}
      disabled={opened}
      oninput={(event) => onWorkdirChange(event.currentTarget.value)}
    />
  </header>
  <div class="xterm-host" bind:this={container}></div>
</section>
