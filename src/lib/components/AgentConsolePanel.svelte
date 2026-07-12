<script lang="ts">
  import AgentOutput from "$lib/components/AgentOutput.svelte";
  import type { AiWorkerTaskResult } from "$lib/ipc";

  type AgentRunLog = {
    id: string;
    at: string;
    tone: "info" | "success" | "error";
    label: string;
    message: string;
  };

  type AgentTerminalLine = {
    id: string;
    prompt: string;
    text: string;
  };

  type AgentPhase = {
    key: string;
    state: string;
    label: string;
  };

  type AgentRunStatus = "idle" | "running" | "completed" | "blocked";

  type Props = {
    style?: string;
    title: string;
    status: string;
    progress: number;
    activityTitle: string;
    activityDetail: string;
    nextStep: string;
    phases: AgentPhase[];
    logs: AgentRunLog[];
    output: string;
    result: AiWorkerTaskResult | null;
    runStatus: AgentRunStatus;
    terminalLines: AgentTerminalLine[];
    terminalInput: string;
    runCardId: string | null;
    onClose: () => void;
    onResizeLog: (event: PointerEvent) => void;
    onResizeOutput: (event: PointerEvent) => void;
    onTerminalInputChange: (value: string) => void;
    onSubmitTerminalInput: () => void;
    onOpenCard: (cardId: string) => void;
  };

  let {
    style,
    title,
    status,
    progress,
    activityTitle,
    activityDetail,
    nextStep,
    phases,
    logs,
    output,
    result,
    runStatus,
    terminalLines,
    terminalInput,
    runCardId,
    onClose,
    onResizeLog,
    onResizeOutput,
    onTerminalInputChange,
    onSubmitTerminalInput,
    onOpenCard,
  }: Props = $props();
</script>

<aside class="agent-console" aria-label="Agent run console" style={style}>
  <header>
    <div>
      <p>Agent Console</p>
      <h3>{title}</h3>
    </div>
    <div class={`run-state ${status}`}>{status}</div>
    <button type="button" aria-label="Close Agent console" onclick={onClose}>×</button>
  </header>
  <div class="console-progress" aria-label="Agent run progress">
    <div class="agent-progress-head">
      <div>
        <span>Now</span>
        <strong>{activityTitle}</strong>
      </div>
      <strong>{progress}%</strong>
    </div>
    <progress max="100" value={progress}></progress>
    <p>{activityDetail}</p>
    <div class="agent-next-step">
      <span>Next</span>
      <strong>{nextStep}</strong>
    </div>
    <div class="agent-phase-row" aria-label="Agent execution phases">
      {#each phases as phase (phase.key)}
        <span class={`agent-phase ${phase.state}`}>{phase.label}</span>
      {/each}
    </div>
  </div>
  <div class="console-lines">
    {#each logs as log (log.id)}
      <div class={`console-line ${log.tone}`}>
        <time>{log.at}</time>
        <code>{log.label}</code>
        <span>{log.message}</span>
      </div>
    {/each}
  </div>
  <div class="stack-resize-handle">
    <span
      class="drag-handle vertical"
      role="separator"
      aria-orientation="vertical"
      onpointerdown={onResizeLog}
    ></span>
  </div>
  <div class="console-output">
    <header>
      <span>Current output</span>
    </header>
    <AgentOutput output={output} result={result} runStatus={runStatus} />
  </div>
  <div class="stack-resize-handle">
    <span
      class="drag-handle vertical"
      role="separator"
      aria-orientation="vertical"
      onpointerdown={onResizeOutput}
    ></span>
  </div>
  <div class="operator-terminal">
    <div class="terminal-lines">
      {#each terminalLines as line (line.id)}
        <div>
          <code>{line.prompt}$</code>
          <span>{line.text}</span>
        </div>
      {/each}
    </div>
    <form
      onsubmit={(event) => {
        event.preventDefault();
        onSubmitTerminalInput();
      }}
    >
      <code>operator$</code>
      <input
        placeholder="Type approval, constraint, or note for this run"
        value={terminalInput}
        oninput={(event) => onTerminalInputChange(event.currentTarget.value)}
      />
      <button type="submit">Send</button>
    </form>
  </div>
  {#if runCardId}
    <footer>
      <span>{runCardId}</span>
      <button type="button" onclick={() => onOpenCard(runCardId)}>Open card</button>
    </footer>
  {/if}
</aside>
