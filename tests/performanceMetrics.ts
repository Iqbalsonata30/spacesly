import {
  frontendPerformanceSnapshot,
  incrementPerformanceCounter,
  recordPerformanceMetric,
  resetFrontendPerformanceMetrics,
  setFrontendPerformanceMode,
} from "../src/lib/performance";

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

resetFrontendPerformanceMetrics();
setFrontendPerformanceMode("normal");
recordPerformanceMetric("interaction_ms", "frontend", 1);
recordPerformanceMetric("interaction_ms", "frontend", 2);
recordPerformanceMetric("interaction_ms", "frontend", 100);
incrementPerformanceCounter("events_total", "ipc_event", 3);

let snapshot = frontendPerformanceSnapshot();
const interaction = snapshot.metrics.find((metric) => metric.name === "interaction_ms");
assert(interaction?.count === 3, "normal mode should aggregate every timing");
assert(interaction?.p50 === 2, "p50 should use the measured distribution");
assert(interaction?.p95 === 100, "p95 should preserve tail latency");
assert(snapshot.recent_spans.length === 0, "normal mode must not retain detailed spans");
assert(
  snapshot.counters.find((counter) => counter.name === "events_total")?.per_second === 0.3,
  "counter rate should use the bounded ten-second window",
);

resetFrontendPerformanceMetrics();
setFrontendPerformanceMode("profiling");
for (let index = 0; index < 300; index += 1) {
  recordPerformanceMetric("profiled_ms", "frontend", index, { operation: "test" });
}
snapshot = frontendPerformanceSnapshot();
assert(snapshot.recent_spans.length === 256, "profiling span retention must remain bounded");
assert(snapshot.recent_spans[0]?.duration_ms === 44, "profiling should retain the newest spans");

setFrontendPerformanceMode("normal");
resetFrontendPerformanceMetrics();
