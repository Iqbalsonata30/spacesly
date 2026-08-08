# Performance diagnostics

Spacesly records local performance metrics in memory. Metrics are never sent externally and the
export contains timing, counts, sizes, process metadata, and non-secret correlation identifiers
only. Prompt text, tool arguments, environment values, credentials, and response bodies are not
recorded.

## Modes and retention

- **Normal** is always enabled. It records aggregate counters and fixed-bucket backend histograms.
  Frontend distributions use a 1,024-sample rolling reservoir per metric.
- **Profiling** is opt-in from Settings → Performance. It additionally retains the latest 512
  backend spans and 256 frontend spans. Leaving Profiling mode immediately discards those spans.
- Rates use a rolling ten-second window. Restarting Spacesly clears all in-memory metrics.

The diagnostics view refreshes every two seconds while it is visible. Its own snapshot IPC calls
are excluded from IPC metrics to avoid measurement feedback.

## Measurement boundaries

| Metric                  | Start                                                                            | End                                                                                                                                           |
| ----------------------- | -------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| Native TTI              | Entry into native `main`, before argument routing and application initialization | Second animation frame after synchronous Settings hydration, frontend mount, workspace cache hydration, and an initial workspace is available |
| Backend ready           | Native process measurement start                                                 | Stores, scheduler, runtime resolver, and execution engine have initialized                                                                    |
| Agent Console open      | User open action                                                                 | Lazy module loaded, state applied, Svelte flushed, and two animation frames painted                                                           |
| Task Session switch     | User selects a different Agent session while the console is open                 | Selected state and required console content painted                                                                                           |
| IPC latency             | Immediately before Tauri `invoke`                                                | Promise resolves or rejects in the frontend                                                                                                   |
| SQLite operation        | Entry to the named repository operation                                          | Result/error returns after query or transaction completion                                                                                    |
| Runtime preparation     | Scheduler begins trusted runtime resolution                                      | Runtime configuration, workspace, OpenCode session, and tool authorities are ready                                                            |
| Workspace resolution    | Trusted profile workspace lookup begins                                          | Trusted path resolution completes or fails                                                                                                    |
| MCP cold initialization | Session cache miss                                                               | Process spawn, initialize negotiation, and initial tool/schema discovery complete                                                             |
| MCP warm initialization | Session cache lookup begins                                                      | A live cached client is resolved                                                                                                              |
| MCP schema discovery    | `tools/list` request begins                                                      | Tool metadata is parsed and cached                                                                                                            |

TTI is not based on a timeout. It represents the first painted frame at which primary navigation
can respond without waiting for blocking startup hydration. Secure credential-status hydration is
progressive and therefore does not delay TTI. Hydration failures still complete the boundary because
the application becomes interactive with a visible error state.

## Correlation

Profiling spans may include `task_session_id`, `execution_attempt`, `worker_id`, `runtime_id`, and
`mcp_connection_id`. These identifiers allow one execution to be followed across scheduler,
runtime, workspace, and MCP stages without retaining sensitive request content.

## Repeatable benchmark

Run the deterministic projection and instrumentation benchmark:

```sh
bun run benchmark:performance
```

It emits JSON and covers Agent Console projection and Task Session replay with 100, 1,000, and
10,000 events, plus Normal and Profiling instrumentation overhead. Use a release build on an idle
machine and retain the JSON with the commit SHA when comparing changes.

Run the bounded SQLite Task Session and local mock MCP harnesses with:

```sh
cargo test performance_baseline_sqlite_task_sessions --release -- --ignored --nocapture
cargo test performance_baseline_mcp_cold_and_warm --release -- --ignored --nocapture
```

For native and infrastructure baselines:

1. Start a release build with a clean application data directory.
2. Open Settings → Performance and reset samples.
3. Run the target scenario (cold startup, real Agent execution, MCP cold connection, then MCP warm
   reuse).
4. Export JSON after the scenario.
5. Compare p50, p95, p99, maximum, count, and instrumentation overhead—not averages alone.

Automated native startup runs may set `SPACESLY_PERFORMANCE_STARTUP_REPORT` to an absolute JSON
path. Spacesly writes one sanitized backend snapshot after the first interactive frame is reported.
The environment variable is opt-in and does not enable telemetry or periodic file writes.

SQLite, runtime, workspace, MCP, and provider timings require those real paths to execute. The
diagnostics UI intentionally reports “No samples yet” rather than manufacturing a value.
