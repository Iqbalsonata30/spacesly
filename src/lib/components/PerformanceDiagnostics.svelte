<script lang="ts">
  import { onMount } from "svelte";
  import {
    exportPerformanceReport,
    getPerformanceReport,
    resetPerformanceMetrics,
    setPerformanceMode,
    type BackendPerformanceSnapshot,
    type FrontendPerformanceSnapshot,
    type MetricSummary,
    type PerformanceMode,
  } from "$lib/performance";

  type Report = {
    backend: BackendPerformanceSnapshot;
    frontend: FrontendPerformanceSnapshot;
  };

  let report = $state<Report | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let changingMode = $state(false);

  const categories = [
    { id: "frontend", label: "Frontend" },
    { id: "ipc", label: "IPC" },
    { id: "sqlite", label: "SQLite" },
    { id: "agent_runtime", label: "Agent Runtime" },
    { id: "workspace", label: "Workspace" },
    { id: "mcp", label: "MCP" },
    { id: "provider", label: "Provider / Runtime" },
  ] as const;

  onMount(() => {
    let disposed = false;
    void refresh().finally(() => {
      if (!disposed) loading = false;
    });
    const timer = window.setInterval(() => void refresh(), 2_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  });

  async function refresh() {
    try {
      const next = await getPerformanceReport();
      report = next;
      error = null;
    } catch (reason) {
      error = reason instanceof Error ? reason.message : String(reason);
    }
  }

  async function changeMode(mode: PerformanceMode) {
    changingMode = true;
    try {
      await setPerformanceMode(mode);
      await refresh();
    } finally {
      changingMode = false;
    }
  }

  async function reset() {
    await resetPerformanceMetrics();
    await refresh();
  }

  function categoryMetrics(category: string): MetricSummary[] {
    if (!report) return [];
    return [...report.backend.metrics, ...report.frontend.metrics]
      .filter(
        (metric) => metric.category === category || metric.category.startsWith(`${category}_`),
      )
      .sort((left, right) => right.p95 - left.p95);
  }

  function counter(name: string) {
    const matches = report
      ? [...report.backend.counters, ...report.frontend.counters].filter(
          (candidate) => candidate.name === name,
        )
      : [];
    return {
      total: matches.reduce((sum, candidate) => sum + candidate.total, 0),
      perSecond: matches.reduce((sum, candidate) => sum + candidate.per_second, 0),
    };
  }

  function categorySummary(category: string): string {
    if (category === "ipc") {
      const calls = counter("ipc_calls_total").perSecond;
      const events = counter("ipc_event_messages_total").perSecond;
      const bytes = counter("ipc_payload_bytes_total").perSecond;
      return `${calls.toFixed(1)} calls/s · ${events.toFixed(1)} events/s${bytes > 0 ? ` · ${formatBytes(bytes)}/s` : ""}`;
    }
    if (category === "sqlite") {
      return `${counter("sqlite_reads_total").perSecond.toFixed(1)} R/s · ${counter("sqlite_writes_total").perSecond.toFixed(1)} W/s`;
    }
    if (category === "mcp") {
      const hits = counter("mcp_session_cache_hits_total").total;
      const misses = counter("mcp_session_cache_misses_total").total;
      const ratio = hits + misses === 0 ? null : (hits / (hits + misses)) * 100;
      return ratio === null ? "No cache samples" : `${ratio.toFixed(0)}% session cache hit`;
    }
    const samples = categoryMetrics(category).reduce((sum, metric) => sum + metric.count, 0);
    return `${samples} samples`;
  }

  function metricLabel(name: string): string {
    return name.replace(/_(ms|bytes|total)$/g, "").replaceAll("_", " ");
  }

  function value(metric: MetricSummary, amount: number): string {
    if (metric.unit === "bytes") return formatBytes(amount);
    return `${amount.toFixed(amount < 10 ? 2 : 1)} ms`;
  }

  function formatBytes(bytes: number): string {
    if (bytes < 1_024) return `${Math.round(bytes)} B`;
    if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(1)} KiB`;
    return `${(bytes / 1_048_576).toFixed(1)} MiB`;
  }
</script>

<section class="diagnostics" id="diagnostics-settings-panel" role="tabpanel">
  <header class="diagnostics-header">
    <div>
      <p>Developer diagnostics</p>
      <h3>Performance</h3>
      <span>Local, bounded measurements. No telemetry leaves this device.</span>
    </div>
    <div class="diagnostics-actions">
      <button type="button" onclick={() => void refresh()}>Refresh</button>
      <button
        type="button"
        onclick={() => report && exportPerformanceReport(report)}
        disabled={!report}>Export JSON</button
      >
    </div>
  </header>

  {#if error}
    <p class="diagnostics-error">{error}</p>
  {:else if loading || !report}
    <p class="diagnostics-empty">Loading performance metrics…</p>
  {:else}
    <div class="mode-row">
      <div>
        <strong>Collection mode</strong>
        <span>Profiling adds capped per-operation spans and payload sizing.</span>
      </div>
      <div class="mode-toggle" role="group" aria-label="Performance collection mode">
        <button
          class:active={report.backend.mode === "normal"}
          type="button"
          disabled={changingMode}
          onclick={() => void changeMode("normal")}>Normal</button
        >
        <button
          class:active={report.backend.mode === "profiling"}
          type="button"
          disabled={changingMode}
          onclick={() => void changeMode("profiling")}>Profiling</button
        >
      </div>
    </div>

    <div class="startup-grid">
      <article>
        <span>TTI</span><strong>{report.backend.startup.tti_ms?.toFixed(1) ?? "—"} ms</strong>
      </article>
      <article>
        <span>Backend ready</span><strong
          >{report.backend.startup.backend_start_ms?.toFixed(1) ?? "—"} ms</strong
        >
      </article>
      <article>
        <span>Workspace ready</span><strong
          >{report.backend.startup.workspace_boot_ms?.toFixed(1) ?? "—"} ms</strong
        >
      </article>
      <article>
        <span>Process RSS</span><strong
          >{report.backend.resources.rss_bytes
            ? formatBytes(report.backend.resources.rss_bytes)
            : "—"}</strong
        >
      </article>
      <article>
        <span>Process CPU</span><strong
          >{report.backend.resources.cpu_percent === null
            ? "Sampling…"
            : `${report.backend.resources.cpu_percent.toFixed(1)}%`}</strong
        >
      </article>
    </div>

    <div class="category-grid">
      {#each categories as category (category.id)}
        {@const metrics = categoryMetrics(category.id)}
        <section class="metric-group">
          <header>
            <strong>{category.label}</strong>
            <span>{categorySummary(category.id)}</span>
          </header>
          {#if metrics.length === 0}
            <p>No samples yet.</p>
          {:else}
            <div class="metric-table" role="table" aria-label={`${category.label} metrics`}>
              <div class="metric-row metric-heading" role="row">
                <span>Metric</span><span>p50</span><span>p95</span><span>p99</span><span>Count</span
                >
              </div>
              {#each metrics.slice(0, 8) as metric (`${metric.category}:${metric.name}`)}
                <div class="metric-row" role="row">
                  <span title={metric.name}>{metricLabel(metric.name)}</span>
                  <span>{value(metric, metric.p50)}</span>
                  <span>{value(metric, metric.p95)}</span>
                  <span>{value(metric, metric.p99)}</span>
                  <span>{metric.count}</span>
                </div>
              {/each}
            </div>
          {/if}
        </section>
      {/each}
    </div>

    {#if report.backend.mode === "profiling"}
      <details class="recent-spans">
        <summary>Recent correlated spans</summary>
        <div>
          {#each [...report.backend.recent_spans, ...report.frontend.recent_spans]
            .slice(-12)
            .reverse() as span, index (`${span.category}:${span.name}:${span.recorded_at_ms}:${index}`)}
            <article>
              <code>{span.name}</code>
              <strong>{span.duration_ms.toFixed(2)} ms</strong>
              <span>
                {Object.entries(span.context ?? {})
                  .map(([key, entry]) => `${key}=${entry}`)
                  .join(" · ") || span.category}
              </span>
            </article>
          {/each}
        </div>
      </details>
    {/if}

    <footer class="diagnostics-footer">
      <span>
        Instrumentation overhead: {report.backend.instrumentation_overhead.average_ns} ns average ·
        {report.backend.instrumentation_overhead.max_ns} ns max
      </span>
      <button type="button" onclick={() => void reset()}>Reset samples</button>
    </footer>
  {/if}
</section>

<style>
  .diagnostics {
    display: grid;
    gap: 14px;
    min-width: 0;
    padding: 2px;
  }
  .diagnostics-header,
  .mode-row,
  .diagnostics-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }
  .diagnostics-header p {
    margin: 0 0 3px;
    color: var(--accent);
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  .diagnostics-header h3 {
    margin: 0;
    color: var(--text-bright);
    font-size: 20px;
  }
  .diagnostics-header span,
  .mode-row span,
  .diagnostics-footer span {
    color: var(--text-muted);
    font-size: 12px;
  }
  .diagnostics-actions,
  .mode-toggle {
    display: flex;
    gap: 6px;
  }
  button {
    min-height: 32px;
    border: 1px solid var(--border-subtle);
    border-radius: 7px;
    background: var(--surface-raised);
    color: var(--text-primary);
    padding: 0 11px;
    font: inherit;
    cursor: pointer;
  }
  button:hover {
    border-color: var(--border-strong);
    background: var(--surface-hover);
  }
  button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  button:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .mode-row {
    border: 1px solid var(--border-subtle);
    border-radius: 9px;
    background: var(--surface);
    padding: 10px 12px;
  }
  .mode-row > div:first-child {
    display: grid;
    gap: 2px;
  }
  .mode-row strong {
    color: var(--text-bright);
    font-size: 13px;
  }
  .mode-toggle button.active {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    background: color-mix(in srgb, var(--accent) 13%, var(--surface-raised));
    color: var(--accent);
  }
  .startup-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
    gap: 8px;
  }
  .startup-grid article {
    display: grid;
    gap: 4px;
    min-width: 0;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    background: var(--surface);
    padding: 10px 11px;
  }
  .startup-grid span {
    color: var(--text-muted);
    font-size: 11px;
  }
  .startup-grid strong {
    overflow: hidden;
    color: var(--text-bright);
    font:
      600 15px/1.2 ui-monospace,
      SFMono-Regular,
      Menlo,
      monospace;
    text-overflow: ellipsis;
  }
  .category-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 9px;
  }
  .metric-group {
    min-width: 0;
    overflow: hidden;
    border: 1px solid var(--border-subtle);
    border-radius: 9px;
    background: var(--surface);
  }
  .metric-group > header {
    display: flex;
    justify-content: space-between;
    border-bottom: 1px solid var(--border-subtle);
    padding: 9px 11px;
  }
  .metric-group > header strong {
    color: var(--text-bright);
    font-size: 13px;
  }
  .metric-group > header span {
    color: var(--text-muted);
    font:
      11px ui-monospace,
      SFMono-Regular,
      Menlo,
      monospace;
  }
  .metric-group > p,
  .diagnostics-empty,
  .diagnostics-error {
    margin: 0;
    padding: 14px 11px;
    color: var(--text-muted);
    font-size: 12px;
  }
  .diagnostics-error {
    color: var(--status-fail);
  }
  .metric-table {
    overflow-x: auto;
  }
  .metric-row {
    display: grid;
    grid-template-columns: minmax(120px, 1fr) repeat(4, minmax(58px, auto));
    gap: 8px;
    min-width: 455px;
    padding: 6px 10px;
    color: var(--text-secondary);
    font:
      11px/1.35 ui-monospace,
      SFMono-Regular,
      Menlo,
      monospace;
  }
  .metric-row:not(.metric-heading) + .metric-row {
    border-top: 1px solid color-mix(in srgb, var(--border-subtle) 55%, transparent);
  }
  .metric-row span:first-child {
    overflow: hidden;
    color: var(--text-primary);
    text-overflow: ellipsis;
    white-space: nowrap;
    text-transform: capitalize;
  }
  .metric-heading {
    color: var(--text-muted);
    font-family: inherit;
    font-size: 10px;
    text-transform: uppercase;
  }
  .diagnostics-footer {
    padding-top: 2px;
  }
  .diagnostics-footer button {
    color: var(--text-muted);
  }
  .recent-spans {
    overflow: hidden;
    border: 1px solid var(--border-subtle);
    border-radius: 9px;
    background: var(--surface);
  }
  .recent-spans summary {
    padding: 9px 11px;
    color: var(--text-primary);
    font-size: 12px;
    cursor: pointer;
  }
  .recent-spans > div {
    border-top: 1px solid var(--border-subtle);
  }
  .recent-spans article {
    display: grid;
    grid-template-columns: minmax(130px, 1fr) auto;
    gap: 3px 12px;
    padding: 7px 11px;
  }
  .recent-spans article + article {
    border-top: 1px solid color-mix(in srgb, var(--border-subtle) 55%, transparent);
  }
  .recent-spans code,
  .recent-spans strong {
    color: var(--text-primary);
    font-size: 11px;
  }
  .recent-spans span {
    grid-column: 1 / -1;
    overflow: hidden;
    color: var(--text-muted);
    font-size: 10px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  @media (max-width: 780px) {
    .diagnostics-header,
    .mode-row,
    .diagnostics-footer {
      align-items: stretch;
      flex-direction: column;
    }
    .startup-grid,
    .category-grid {
      grid-template-columns: 1fr;
    }
    .diagnostics-actions button,
    .mode-toggle button {
      flex: 1;
    }
  }
</style>
