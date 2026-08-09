# Spacesly Work Progress Report

## Objective
- Turn Spacesly into a production-grade local AI agent orchestration platform through instrumentation, observability, and hardened invariants — without broad rewrites or changing existing behavior.
- The latest completed architecture increment is the blocked-continuation provenance fix in `f79ad2a`.

## Important Details
- Persistent constraints: do not rewrite existing systems, change business logic, redesign unrelated UI, introduce cloud dependencies, or silently convert Resume into Retry; preserve Task Session isolation, fencing, OpenCode resume, approvals, workspace isolation, and 5-worker concurrency.
- Performance work is already committed; the Activity Log projection fix must remain intact.
- Nix environment: browser tests require `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/nix/store/ghvr17543vw7ykqalnc0dj60gaagbqip-chromium-151.0.7922.75/bin/chromium`.
- Full Playwright suite intermittently needs `--workers=1`; parallel runs previously timed out on unrelated specs under resource contention.
- TTI baseline on this machine used `llvmpipe` software rendering because hardware EGL initialization failed; do not treat those TTI figures as representative hardware results.
- Shutdown was requested and approved, but `systemctl poweroff`, `sudo -n systemctl poweroff`, and the login1 D-Bus poweroff call all failed with "Interactive authentication required" / "sudo: a password is required". This is an environment limitation, not a project issue.
- Relevant history: `f79ad2a fix: allow same-session continuations to supersede run projection provenance` → `75e3941 perf: optimize agent activity projection` → `c1db3ca feat: add performance diagnostics and refine agent UX` → `a35108d fix: complete agent approval resume flow` → `13453bf fix: harden agent rules and skills runtime`.
- Worktree must remain clean; only committed changes are acceptable.

## Work State
### Completed
- Performed a read-only architecture review across orchestration, runtime/context/tools, frontend/IPC/observability, and recovery/policy/evaluation.
- Delivered a full architecture report with execution flow, ownership boundaries, durable/ephemeral state, recovery semantics, and scale-readiness evidence; no code was changed during the review.
- Confirmed existing strengths: durable scheduler journal, assignment fencing, immutable runtime profiles, durable OpenCode session identity, completion outbox projection, bounded frontend presentation, and 5-worker concurrency.
- Committed `75e3941 perf: optimize agent activity projection` (`src/lib/agentTimeline.ts` only), leaving the worktree clean.
- Earlier this session, committed approval-flow fixes (`a35108d`) and performance diagnostics (`c1db3ca`).
- Previously attempted an `initialUiPainted` deferral of background hydration; measured no TTI improvement (`6.8 s` and `17.6 s` after vs `6.1–14.2 s` before) and reverted it, keeping only the proven Activity Log optimization.
- Earlier measured improvements for `timelineActivities`: 100-event projection p50 `4.73 ms` → `2.41–3.98 ms`; 1,000-event p50 `102.83 ms` → `21.86–27.69 ms`; 10,000-event p50 `31.34 s` → `219–276 ms`.
- Prior regression proof passed: Svelte check, ESLint, `npm test`, `npm run check`, Rust `cargo test` 337 passed + 2 ignored, full Playwright serial suite 6 passed, and production `tauri build --no-bundle`.
- Completed Increment 1: same-session continuation attempts may supersede prior `worker.execute` projection provenance while projections from different Task Sessions remain rejected. Integration and focused provenance tests were added; `f79ad2a` is committed.
- Completed Increment 2: scheduler Chat completion projection now derives the immutable expected user-message head from the durable V2 Task Session envelope, compares and appends atomically in one `executions.db` transaction, and permanently terminalizes stale-head conflicts as `blocked` instead of retrying forever in `committing`.
- Increment 2 verification: Rust `cargo test` 349 passed + 2 ignored; clippy reports only the two pre-existing warnings in `ocp/mod.rs` and `performance.rs`.

### Active
- Architecture implementation has completed the two P0 correctness increments. The next increment is authority hardening and execution-trace observability, scoped to preserve existing Task Session behavior.

### Blocked
- (none)

## Next Move
1. Implement authority hardening and an execution-trace query surface without changing scheduler ownership or frontend behavior.
2. Follow with context/manifest visibility, reliability/evaluation instrumentation, and multi-agent prerequisites.

## Relevant Files
- `src-tauri/src/infrastructure/execution_store.rs`: step-run projection provenance; blocked continuation conflict site.
- `src-tauri/src/infrastructure/scheduler_store.rs`: resume/continue requeue and attempt/fence creation.
- `src-tauri/src/application/execution_engine.rs`: scheduler loop, worker ownership, notifier, publish paths.
- `src-tauri/src/application/agent_task_executor.rs`: agent Task Session execution and event persistence.
- `src-tauri/src/application/prompt_task_executor.rs`: Chat execution; buffered callbacks and freshness validation.
- `src-tauri/src/infrastructure/mcp.rs`: MCP client lifecycle, schema caching, initialization coalescing.
- `src-tauri/src/infrastructure/performance.rs`: bounded span/histogram instrumentation and diagnostics.
- `src/lib/agentTimeline.ts`: optimized Activity Log projection (committed).
- `src/routes/+page.svelte`: frontend session projections, IPC replay, Activity/Progress, settings and diagnostics UI.
- `src/lib/agentTaskSessions.ts`: typed Task Session orchestration, replay cursor, approval-pause handling.
- `src/lib/performance.ts`, `src/lib/components/PerformanceDiagnostics.svelte`, `docs/performance-diagnostics.md`: existing observability base.
- `tests/performance-benchmark.ts`, `tests/taskSessions.ts`, `tests/approval-ui.spec.ts`: benchmarks and regression coverage.
- `/tmp/opencode/spacesly-startup-{baseline,before-*,after-*}.json`: release startup reports from this check; only baselines `spacesly-startup-baseline.json` and `spacesly-startup-warm.json` should be treated as unoptimized comparison data.
