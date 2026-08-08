import { timelineActivities } from "../src/lib/agentTimeline";
import { agentSessionReplay, type AgentRunLog, type AgentSessionEvent } from "../src/lib/agentRun";
import {
  frontendPerformanceSnapshot,
  recordPerformanceMetric,
  resetFrontendPerformanceMetrics,
  setFrontendPerformanceMode,
} from "../src/lib/performance";

const sizes = [100, 1_000, 10_000];
const runsBySize = new Map([
  [100, 20],
  [1_000, 10],
  [10_000, 3],
]);

function distribution(samples: number[]) {
  const sorted = [...samples].sort((left, right) => left - right);
  const percentile = (value: number) =>
    sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * value) - 1)] ?? 0;
  return {
    p50_ms: percentile(0.5),
    p95_ms: percentile(0.95),
    p99_ms: percentile(0.99),
    max_ms: sorted.at(-1) ?? 0,
  };
}

function measure(operation: () => void, runs: number): number[] {
  const samples: number[] = [];
  for (let run = 0; run < runs; run += 1) {
    const startedAt = performance.now();
    operation();
    samples.push(performance.now() - startedAt);
  }
  return samples;
}

function logs(size: number): AgentRunLog[] {
  return Array.from({ length: size }, (_, index) => ({
    id: `log-${index}`,
    at: "12:00:00",
    tone: "info" as const,
    label: `tool-${index % 20}`,
    message: [
      "STATUS: Running",
      `SUMMARY: Synthetic operation ${index}`,
      "EVIDENCE:",
      `- Event ${index}`,
      "DETAILS:",
      "- Repeatable benchmark payload",
    ].join("\n"),
  }));
}

function transcript(size: number): AgentSessionEvent[] {
  return Array.from({ length: size }, (_, index) => ({
    id: `event-${index}`,
    type: "system" as const,
    at: 1_700_000_000_000 + index,
    text: `Synthetic Task Session event ${index}`,
  }));
}

const scenarios = sizes.flatMap((size) => {
  const runs = runsBySize.get(size) ?? 3;
  const syntheticLogs = logs(size);
  const syntheticTranscript = transcript(size);
  return [
    {
      scenario: "agent_console_projection",
      events: size,
      runs,
      ...distribution(measure(() => void timelineActivities(syntheticLogs, 10), runs)),
    },
    {
      scenario: "task_session_switch_projection",
      events: size,
      runs,
      ...distribution(measure(() => void agentSessionReplay(syntheticTranscript, 12_000), runs)),
    },
  ];
});

function instrumentationOverhead(mode: "normal" | "profiling") {
  resetFrontendPerformanceMetrics();
  setFrontendPerformanceMode(mode);
  const iterations = 25_000;
  const startedAt = performance.now();
  for (let index = 0; index < iterations; index += 1) {
    recordPerformanceMetric("benchmark_sample_ms", "benchmark", index % 10);
  }
  const elapsedMs = performance.now() - startedAt;
  return {
    mode,
    iterations,
    elapsed_ms: elapsedMs,
    average_ns: (elapsedMs * 1_000_000) / iterations,
    retained_spans: frontendPerformanceSnapshot().recent_spans.length,
  };
}

console.log(
  JSON.stringify(
    {
      schema_version: 1,
      generated_at: new Date().toISOString(),
      runs_by_size: Object.fromEntries(runsBySize),
      scenarios,
      instrumentation_overhead: [
        instrumentationOverhead("normal"),
        instrumentationOverhead("profiling"),
      ],
    },
    null,
    2,
  ),
);
