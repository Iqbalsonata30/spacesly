# Production Readiness Engineering Report

Date: 2026-07-29

Reviewed baseline: `f69c6f8 feat: add concurrent task session execution`

## Executive Summary

The Task Session architecture has a sound concurrency foundation: five long-lived workers, durable FIFO queueing, assignment attempts, leases, fencing tokens, isolated runtime processes, staged results, and per-session projections. Deterministic tests with 10, 20, and 50 tasks confirm the worker cap, automatic queue draining, worker reuse, event isolation, and explicit session deletion.

The implementation is not yet ready for unattended production execution. Several failure paths can still produce duplicate side effects, orphan an active assignment, block the scheduler control plane, or mix renderer-supplied context into another conversation. These correctness risks must be solved before throughput optimization or UI polish.

## Architecture

- One scheduler coordinator thread owns dispatch and process-local assignment state.
- Five long-lived worker threads execute assignments.
- SQLite stores sessions, attempts, leases, fencing tokens, grants, events, progress, and staged results.
- Agent, Chat, and Edit use isolated runtime state per Task Session.
- OpenCode runs in an isolated process per assignment.
- Task-local MCP and built-in tools validate assignment authority.
- Structured results are staged before terminal completion and projected idempotently.
- A shared Tauri event stream provides best-effort wake-up hints; the durable journal remains authoritative.
- Frontend Agent state is card-owned and Chat state is conversation-owned.
- Shared infrastructure stores and registries remain process-level singleton services.

## Validation Results

### Deterministic Stress Tests

| Scenario | Result | Verified behavior |
| --- | --- | --- |
| 10 tasks | Passed | Exactly 5 running and 5 queued at saturation |
| 20 tasks | Passed | FIFO dispatch and automatic queue drain |
| 50 tasks | Passed | Maximum 5 workers, worker reuse, automatic drain, event isolation, and session deletion |

The 50-task scenario verified:

- Peak running workers equals `MAX_EXECUTION_WORKERS` (5).
- All 50 sessions reach `succeeded`.
- All five workers process multiple sessions.
- Total worker completion count equals 50.
- Every event remains scoped to its owning session.
- Activity labels do not leak between tasks.
- Every terminal session can be removed.
- No sessions remain after explicit cleanup.

### Automated Verification

- Rust tests: 202 passed.
- `git diff --check`: passed.

The stress executor is deterministic and in-process. It validates scheduler semantics, not real OpenCode/provider CPU, memory, network, child-process, or file-descriptor behavior.

## Production Blockers

### P0. Scheduler Completion and Cancellation Correctness

This must be solved first because it controls whether a task can execute more than once.

Current risks:

- Completion projection executes synchronously on the scheduler coordinator thread.
- A slow projection can delay dispatch, cancellation, worker completion, and lease renewal.
- Workers are released before durable completion is guaranteed.
- A persistence error can leave a session `running` without a process-local renewal handle.
- Cancellation can race structured completion: the session becomes `cancelling`, while completion staging only accepts `running`.
- Engine shutdown and command replies can wait indefinitely on a non-cooperative executor or blocked coordinator.

Required solution:

1. Move completion projection to a bounded projector worker or projector queue.
2. Keep assignment/worker ownership until staging succeeds or a durable failure state is recorded.
3. Make structured completion and cancellation resolution one atomic scheduler transition.
4. Add exponential backoff and a maximum retry policy for projection failures.
5. Expose scheduler health when projection, claim, renewal, or finalization fails.
6. Add bounded command and shutdown behavior.

Acceptance criteria:

- A projection blocked longer than one lease cannot expire unrelated running assignments.
- Failure at stage/finalize boundaries cannot orphan a session.
- Cancellation versus completion always reaches one terminal state without waiting for lease expiry.
- A permanent projection error does not busy-loop or block unrelated identities.

Primary files:

- `src-tauri/src/application/execution_engine.rs`
- `src-tauri/src/infrastructure/scheduler_store.rs`
- `src-tauri/src/infrastructure/execution_store.rs`

### P0. Backend-Authoritative Conversation Context

This must be solved before claiming conversation isolation.

Current risk:

- The latest Chat message is verified, but `session_context` and `terminal_context` are supplied by the renderer and passed into the model prompt.
- A malformed or compromised caller can attach context from another conversation to an otherwise valid conversation ID.

Required solution:

1. Build conversation context exclusively from durable backend messages.
2. Treat renderer context as an optional display hint, not model authority.
3. Bind context revision/digest to the exact durable message sequence used by the prompt.
4. Reject stale or non-contiguous conversation turns.

Acceptance criteria:

- A request cannot introduce text from another conversation.
- Concurrent conversations always resolve independent backend snapshots.
- Replaying the same immutable input reconstructs the same prompt context.

Primary files:

- `src-tauri/src/application/stored_agent_runtime_resolver.rs`
- `src-tauri/src/application/prompt_task_executor.rs`
- `src-tauri/src/infrastructure/execution_store.rs`
- `src-tauri/src/domain/task_session.rs`

### P0. Workspace and Repository Mutation Isolation

This must be solved before allowing multiple mutating Agent tasks in one workspace.

Current risks:

- Different cards in the same workspace can run concurrently when conversation, subject, and execution-run IDs differ.
- Concurrent file writes, checkout, stage, commit, merge, or rebase can interfere through the same worktree and Git index.
- Git subprocesses are blocking and do not poll cancellation or assignment authority while running.

Required solution:

1. Add a per-workspace or per-repository mutation lane.
2. Permit concurrent read-only tasks while serializing mutating tasks.
3. Run Git commands through a cancellable process-group executor.
4. Revalidate assignment authority immediately before every irreversible operation.
5. Add operation timeouts and cleanup for Git/network hangs.

Trade-off:

- Workspace-wide serialization is simplest and safest but reduces concurrency.
- Repository mutation locks plus file-level write ownership offer more concurrency but require a more complex conflict model.
- Recommended first implementation: one mutating lane per canonical repository/workspace, with concurrent read-only lanes.

Acceptance criteria:

- Two tasks cannot mutate the same repository concurrently.
- Cancellation terminates the Git process tree.
- A stale assignment cannot commit, push, merge, rebase, or rename a workspace file.

Primary files:

- `src-tauri/src/infrastructure/scheduler_store.rs`
- `src-tauri/src/infrastructure/task_tools.rs`
- `src-tauri/src/infrastructure/git.rs`
- `src-tauri/src/infrastructure/files.rs`

### P0. External Side-Effect Idempotency

Current risk:

- Crash recovery is at-least-once. Jira, MCP, Git push, or another external mutation may succeed before its receipt is persisted, causing retry to repeat the operation.

Required solution:

1. Assign a deterministic operation ID to every side effect.
2. Persist an operation intent before forwarding the effect.
3. Persist or reconcile a completion receipt after forwarding.
4. Use upstream idempotency keys where supported.
5. Block ambiguous operations for operator reconciliation when the upstream cannot prove their status.

Acceptance criteria:

- Restart at every pre/post-forward boundary cannot silently duplicate an effect.
- Ambiguous effects become an explicit blocked state, not an automatic retry.

Primary files:

- `src-tauri/src/infrastructure/task_tools.rs`
- `src-tauri/src/infrastructure/mcp.rs`
- `src-tauri/src/infrastructure/execution_store.rs`
- `src-tauri/src/infrastructure/scheduler_store.rs`

## High-Priority Reliability Work

### P1. Child-Process Lifecycle and Resource Cleanup

Current risks:

- Some MCP/proxy error paths return before child termination and `wait`.
- Detached reader threads are not consistently joined.
- Dropping an MCP client may kill only the direct child, not descendants.
- Shutdown can block on non-cooperative work.

Required solution:

1. Introduce RAII process-tree guards for MCP, OpenCode, shell, and Git children.
2. Create a process group before any fallible pipe setup.
3. Kill and reap the process group on every setup/runtime error.
4. Join reader threads with bounded shutdown behavior.
5. Add PID and file-descriptor leak tests.

### P1. Frontend Recovery and Waiter Ownership

Current risks:

- Recovered Chat tasks are watched without first being represented as active conversation runs.
- The user can submit a second turn for the same conversation during recovery.
- A running conversation can be evicted by the six-session display limit.
- Waiters do not accept an `AbortSignal` for component destruction or eviction.
- Observer/read failures can cancel otherwise healthy Agent work.

Required solution:

1. Hydrate active run ownership before enabling Chat submission.
2. Never evict a conversation with a queued/running/cancelling/committing task.
3. Add abortable observer handles separate from backend cancellation.
4. Treat observer failures as recoverable UI errors, not automatic execution cancellation.
5. Reconcile retained tasks through one shared lifecycle controller.

Primary files:

- `src/routes/+page.svelte`
- `src/lib/workspaceChatRuns.ts`
- `src/lib/agentTaskSessions.ts`
- `src/lib/promptTaskSessions.ts`

### P1. Shared Reconciliation Instead of Per-Task Polling

Current idle reconciliation cost:

- 10 tasks: approximately 20 IPC reads/second.
- 20 tasks: approximately 40 IPC reads/second.
- 50 tasks: approximately 100 IPC reads/second.

Required solution:

1. Replace per-task one-second timers with one shared reconciliation loop.
2. Batch snapshot/event reads where possible.
3. Reconcile only active tasks and selected terminal details.
4. Isolate subscriber exceptions so one handler cannot delay another.
5. Apply backpressure/coalescing to notifier updates.

### P1. Retention and Cleanup Policy

Current risks:

- Sessions, attempts, events, owner rows, notifier metadata, and cached OpenCode configurations grow without a complete retention policy.
- Frontend does not normally call `removeTaskSession`.

Required solution:

1. Define retention periods for successful, failed, blocked, and cancelled sessions.
2. Retain audit summaries separately from verbose runtime deltas.
3. Prune notifier metadata when sessions are removed.
4. Bound subscriber queues and runtime event payload rates.
5. Prune scheduler owner rows and stale caches.
6. Add database/WAL size observability.

## Performance and Operability Improvements

### P2. Scheduler Storage Contention

- Separate event ingestion from lease/dispatch control writes.
- Batch or coalesce text-delta events.
- Consider a small connection pool or dedicated writer queues.
- Ensure lease renewals have priority over timeline persistence.
- Add latency metrics for claim, event append, renewal, projection, and finalization.

### P2. Dispatch Efficiency

- Stop loading and decoding every retained payload after each claim.
- Publish only sessions whose projection changed.
- Add indexed filtered list APIs for workspace, state, kind, and ownership.
- Add admission limits and queue quotas.

### P2. Lease Time Source and Multi-Process Fairness

- Add tests for forward/backward wall-clock jumps.
- Persist conservative deadlines while using monotonic time for process-local scheduling.
- Add prompt capacity wake-ups across scheduler processes instead of relying on a ten-second tick.
- Validate fairness when two engines share one scheduler database.

## Recommended Solving Order

1. Scheduler completion/cancellation atomicity and non-blocking projection.
2. Backend-authoritative conversation context.
3. Per-workspace/repository mutation lane and cancellable Git.
4. External side-effect idempotency and ambiguity handling.
5. Child-process RAII cleanup and bounded shutdown.
6. Frontend retained-run hydration, non-eviction, and abortable observers.
7. Shared reconciliation loop and notifier backpressure.
8. Session/event/cache/owner retention policy.
9. Scheduler storage and dispatch performance optimization.
10. Real-runtime soak and fault-injection validation.

The first four items are release blockers. Items five through eight are required before sustained unattended operation. Performance tuning should follow correctness fixes so optimization does not encode unsafe lifecycle behavior.

## Required Final Validation

After remediation, run:

1. Deterministic 10/20/50 scheduler tests with cancellation and injected storage failures.
2. Two-engine fairness tests against one Scheduler database.
3. Real OpenCode/MCP soak tests with 10, 20, and 50 queued tasks.
4. Same-workspace mutation tests covering write, checkout, commit, merge, rebase, and cancellation.
5. Crash injection at intent, side-effect, staged, projected, and finalized boundaries.
6. Heap, RSS, child PID, thread, and file-descriptor measurements before and after cleanup.
7. SQLite/WAL growth measurement under high-rate activity streaming.
8. Frontend reload/unmount tests with running and retained sessions.

Production acceptance should require:

- No more than five running workers.
- Automatic queue drain without manual wake-up.
- No orphaned `running` or `cancelling` sessions.
- No duplicate external effects after injected crashes.
- No cross-conversation, MCP, or tool-state leakage.
- No surviving child processes, listeners, timers, or file descriptors after session cleanup.
- Bounded database, memory, and IPC growth under a sustained workload.

## Files Modified During This Review Phase

- `src-tauri/src/application/execution_engine.rs`: added deterministic 50-task stress coverage.
- `PRODUCTION_READINESS_REPORT.md`: added this report and prioritized remediation plan.

## New Components Introduced

- No production component was introduced.
- One deterministic stress-test scenario and this engineering report were added.

## Known Limitations

- Stress validation uses a deterministic mock executor rather than 50 real OpenCode/MCP processes.
- Real provider latency, memory usage, network behavior, process cleanup, and file-descriptor pressure remain unmeasured.
- The unrelated working-tree change in `src-tauri/src/infrastructure/global_environment.rs` was not modified during this phase.

## Suggested Next Phase

Implement P0 scheduler completion/cancellation correctness first. Do not begin performance tuning or increase worker capacity until durable completion cannot orphan assignments and projection work can no longer block lease renewal.
