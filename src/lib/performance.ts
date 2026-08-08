import { invoke as tauriInvoke } from "@tauri-apps/api/core";

export type PerformanceMode = "normal" | "profiling";

export type MetricSummary = {
  name: string;
  category: string;
  unit: "ms" | "bytes";
  count: number;
  average: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
};

export type CounterSummary = {
  name: string;
  category: string;
  total: number;
  per_second: number;
};

export type RecentSpan = {
  name: string;
  category: string;
  duration_ms: number;
  recorded_at_ms: number;
  context?: Record<string, string>;
};

export type BackendPerformanceSnapshot = {
  schema_version: number;
  app_version: string;
  generated_at_ms: number;
  uptime_ms: number;
  mode: PerformanceMode;
  environment: Record<string, string>;
  startup: {
    backend_start_ms: number | null;
    frontend_boot_ms: number | null;
    hydration_ms: number | null;
    workspace_boot_ms: number | null;
    first_interactive_frame_ms: number | null;
    tti_ms: number | null;
  };
  resources: { rss_bytes: number | null; cpu_percent: number | null };
  metrics: MetricSummary[];
  counters: CounterSummary[];
  recent_spans: RecentSpan[];
  instrumentation_overhead: { samples: number; average_ns: number; max_ns: number };
};

export type FrontendPerformanceSnapshot = {
  mode: PerformanceMode;
  generated_at_ms: number;
  uptime_ms: number;
  metrics: MetricSummary[];
  counters: CounterSummary[];
  recent_spans: RecentSpan[];
};

const FRONTEND_STARTED_AT = performance.now();
const SAMPLE_LIMIT = 1024;
const RECENT_SPAN_LIMIT = 256;
const RATE_WINDOW_MS = 10_000;

type Aggregate = {
  category: string;
  unit: "ms" | "bytes";
  count: number;
  total: number;
  max: number;
  samples: number[];
  cursor: number;
};

type RateCounter = {
  category: string;
  total: number;
  timestamps: Array<{ at: number; amount: number }>;
};

const aggregates = new Map<string, Aggregate>();
const counters = new Map<string, RateCounter>();
const recentSpans: RecentSpan[] = [];
let currentMode: PerformanceMode = "normal";

export function performanceMode(): PerformanceMode {
  return currentMode;
}

export function frontendUptimeMs(): number {
  return performance.now() - FRONTEND_STARTED_AT;
}

export async function reportFrontendStartup(metrics: {
  frontend_boot_ms: number;
  hydration_ms: number;
  workspace_boot_ms: number;
  first_interactive_frame_ms: number;
}): Promise<void> {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
  await measuredInvoke<void>("record_frontend_startup", { metrics });
}

export function setFrontendPerformanceMode(mode: PerformanceMode): void {
  currentMode = mode;
  if (mode === "normal") recentSpans.length = 0;
}

export function recordPerformanceMetric(
  name: string,
  category: string,
  durationMs: number,
  context: Record<string, string> = {},
  unit: "ms" | "bytes" = "ms",
): void {
  if (!Number.isFinite(durationMs) || durationMs < 0) return;
  let aggregate = aggregates.get(name);
  if (!aggregate) {
    aggregate = { category, unit, count: 0, total: 0, max: 0, samples: [], cursor: 0 };
    aggregates.set(name, aggregate);
  }
  aggregate.count += 1;
  aggregate.total += durationMs;
  aggregate.max = Math.max(aggregate.max, durationMs);
  if (aggregate.samples.length < SAMPLE_LIMIT) {
    aggregate.samples.push(durationMs);
  } else {
    aggregate.samples[aggregate.cursor] = durationMs;
    aggregate.cursor = (aggregate.cursor + 1) % SAMPLE_LIMIT;
  }
  if (currentMode === "profiling") {
    if (recentSpans.length === RECENT_SPAN_LIMIT) recentSpans.shift();
    recentSpans.push({
      name,
      category,
      duration_ms: durationMs,
      recorded_at_ms: Math.round(performance.now() - FRONTEND_STARTED_AT),
      context,
    });
  }
}

export function incrementPerformanceCounter(name: string, category: string, amount = 1): void {
  const now = performance.now();
  let counter = counters.get(name);
  if (!counter) {
    counter = { category, total: 0, timestamps: [] };
    counters.set(name, counter);
  }
  counter.total += amount;
  counter.timestamps.push({ at: now, amount });
  trimRateWindow(counter, now);
}

export function recordIpcEvent(eventName: string, payload?: unknown): void {
  incrementPerformanceCounter("ipc_event_messages_total", "ipc_event");
  if (currentMode === "profiling") {
    const bytes = serializedSize(payload);
    incrementPerformanceCounter("ipc_event_payload_bytes_total", "ipc_event", bytes);
    recordPerformanceMetric(
      "ipc_event_payload_bytes",
      "ipc_event",
      bytes,
      { event: eventName },
      "bytes",
    );
  }
}

export function startPerformanceSpan(
  name: string,
  category: string,
  context: Record<string, string> = {},
): () => number {
  const startedAt = performance.now();
  let finished = false;
  return () => {
    if (finished) return 0;
    finished = true;
    const duration = performance.now() - startedAt;
    recordPerformanceMetric(name, category, duration, context);
    return duration;
  };
}

export function finishPerformanceSpanAfterPaint(finish: () => number): void {
  requestAnimationFrame(() => requestAnimationFrame(() => finish()));
}

export async function measuredInvoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (
    command === "get_performance_metrics" ||
    command === "set_performance_mode" ||
    command === "reset_performance_metrics" ||
    command === "record_frontend_startup"
  ) {
    return tauriInvoke<T>(command, args);
  }
  const startedAt = performance.now();
  const requestBytes = currentMode === "profiling" ? serializedSize(args) : 0;
  incrementPerformanceCounter("ipc_calls_total", "ipc");
  incrementPerformanceCounter(`ipc_${command}_calls_total`, "ipc");
  try {
    const response = await tauriInvoke<T>(command, args);
    const duration = performance.now() - startedAt;
    recordPerformanceMetric("ipc_latency_ms", "ipc", duration, { command });
    recordPerformanceMetric(`ipc_${command}_latency_ms`, "ipc", duration);
    if (currentMode === "profiling") {
      const payloadBytes = requestBytes + serializedSize(response);
      incrementPerformanceCounter("ipc_payload_bytes_total", "ipc", payloadBytes);
      recordPerformanceMetric("ipc_payload_bytes", "ipc", payloadBytes, { command }, "bytes");
    }
    return response;
  } catch (error) {
    recordPerformanceMetric("ipc_latency_ms", "ipc", performance.now() - startedAt, {
      command,
      outcome: "error",
    });
    recordPerformanceMetric(`ipc_${command}_latency_ms`, "ipc", performance.now() - startedAt, {
      outcome: "error",
    });
    throw error;
  }
}

export async function getPerformanceReport(): Promise<{
  backend: BackendPerformanceSnapshot;
  frontend: FrontendPerformanceSnapshot;
}> {
  const backend = await measuredInvoke<BackendPerformanceSnapshot>("get_performance_metrics");
  return { backend, frontend: frontendPerformanceSnapshot() };
}

export async function setPerformanceMode(
  mode: PerformanceMode,
): Promise<BackendPerformanceSnapshot> {
  const snapshot = await measuredInvoke<BackendPerformanceSnapshot>("set_performance_mode", {
    mode,
  });
  setFrontendPerformanceMode(mode);
  return snapshot;
}

export async function resetPerformanceMetrics(): Promise<BackendPerformanceSnapshot> {
  resetFrontendPerformanceMetrics();
  return measuredInvoke<BackendPerformanceSnapshot>("reset_performance_metrics");
}

export function exportPerformanceReport(report: {
  backend: BackendPerformanceSnapshot;
  frontend: FrontendPerformanceSnapshot;
}): void {
  const payload = JSON.stringify(
    {
      schema_version: 1,
      exported_at: new Date().toISOString(),
      ...report,
    },
    null,
    2,
  );
  const url = URL.createObjectURL(new Blob([payload], { type: "application/json" }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `spacesly-performance-${new Date().toISOString().replaceAll(":", "-")}.json`;
  anchor.click();
  URL.revokeObjectURL(url);
}

export function frontendPerformanceSnapshot(): FrontendPerformanceSnapshot {
  const now = performance.now();
  const metrics = [...aggregates.entries()]
    .map(([name, aggregate]) => summarize(name, aggregate))
    .sort(
      (left, right) =>
        left.category.localeCompare(right.category) || left.name.localeCompare(right.name),
    );
  const counterSummaries = [...counters.entries()]
    .map(([name, counter]) => {
      trimRateWindow(counter, now);
      return {
        name,
        category: counter.category,
        total: counter.total,
        per_second:
          counter.timestamps.reduce((total, sample) => total + sample.amount, 0) /
          (RATE_WINDOW_MS / 1_000),
      };
    })
    .sort(
      (left, right) =>
        left.category.localeCompare(right.category) || left.name.localeCompare(right.name),
    );
  return {
    mode: currentMode,
    generated_at_ms: Date.now(),
    uptime_ms: Math.round(now - FRONTEND_STARTED_AT),
    metrics,
    counters: counterSummaries,
    recent_spans: currentMode === "profiling" ? [...recentSpans] : [],
  };
}

export function resetFrontendPerformanceMetrics(): void {
  aggregates.clear();
  counters.clear();
  recentSpans.length = 0;
}

export function observeFrontendPerformance(): () => void {
  if (typeof PerformanceObserver === "undefined") return () => {};
  const supported = PerformanceObserver.supportedEntryTypes ?? [];
  if (!supported.includes("longtask")) return () => {};
  const observer = new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) {
      recordPerformanceMetric("frontend_long_task_ms", "frontend", entry.duration);
      incrementPerformanceCounter("frontend_long_tasks_total", "frontend");
    }
  });
  observer.observe({ entryTypes: ["longtask"] });
  return () => observer.disconnect();
}

function summarize(name: string, aggregate: Aggregate): MetricSummary {
  const sorted = [...aggregate.samples].sort((left, right) => left - right);
  return {
    name,
    category: aggregate.category,
    unit: aggregate.unit,
    count: aggregate.count,
    average: aggregate.count === 0 ? 0 : aggregate.total / aggregate.count,
    p50: percentile(sorted, 0.5),
    p95: percentile(sorted, 0.95),
    p99: percentile(sorted, 0.99),
    max: aggregate.max,
  };
}

function percentile(sorted: number[], percentileValue: number): number {
  if (sorted.length === 0) return 0;
  return sorted[
    Math.min(sorted.length - 1, Math.max(0, Math.ceil(sorted.length * percentileValue) - 1))
  ];
}

function trimRateWindow(counter: RateCounter, now: number): void {
  const cutoff = now - RATE_WINDOW_MS;
  let firstRetained = 0;
  while (
    firstRetained < counter.timestamps.length &&
    counter.timestamps[firstRetained].at < cutoff
  ) {
    firstRetained += 1;
  }
  if (firstRetained > 0) counter.timestamps.splice(0, firstRetained);
}

function serializedSize(value: unknown): number {
  if (value === undefined) return 0;
  try {
    return new TextEncoder().encode(JSON.stringify(value)).byteLength;
  } catch {
    return 0;
  }
}
