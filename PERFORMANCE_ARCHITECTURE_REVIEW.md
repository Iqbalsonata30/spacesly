# Spacesly Performance Architecture Review

Date: 2026-08-01

## Executive Summary

Spacesly's primary responsiveness risk was not raw IPC query time or bounded list size. It was state amplification between durable Task Session events and Svelte presentation state:

```text
Task Session event
  -> immediate root session mutation
  -> rebuild every retained card projection
  -> invalidate board and visible console consumers
  -> re-project the full Activity Log
```

The UI did this even for token-level runtime events whose progress value had not changed. The implemented architecture now follows:

```text
Runtime events
  -> non-reactive per-card accumulator
  -> semantic progress/lifecycle/log projection
  -> at most one publication per 16 ms window
  -> compact per-card business projection
  -> board and visible console
```

This separates execution frequency from presentation frequency without changing Task Session durability, event generation, runtime behavior, or backend execution semantics.

## Method

The review combined:

- Static tracing from provider/runtime events through scheduler replay, page state, Svelte derivations, component props, and DOM bindings.
- Bun microbenchmarks for Activity projection, progress-event projection, session switching, and serialization.
- SQLite measurements against the live Spacesly stores.
- Existing real-window AT-SPI measurements from `UI_PERFORMANCE_AUDIT.md`.
- Production and debug build inspection.

WebKitGTK does not expose Chrome DevTools Protocol in this environment. Exact style/layout/paint duration, long animation frames, and dropped-frame counts remain unavailable. Suspected paint/layout issues are not reported as measured facts.

## Performance Profile

### Current data scale

| Metric | Measured value |
| --- | ---: |
| Retained Task Sessions | 31 |
| Task Session request payload | 50,591 bytes |
| Retained Task Session events | 290 |
| Task Session event payload | 53,855 bytes |
| Full Task Session list query | 0.123 ms mean |
| Conversations | 8 |
| Conversation messages | 18 |

Task Session event distribution:

| Event kind | Count | Average payload | Maximum payload |
| --- | ---: | ---: | ---: |
| Lifecycle | 124 | 44.5 bytes | 166 bytes |
| Progress | 70 | 89.9 bytes | 96 bytes |
| Runtime | 57 | 496.8 bytes | 5,938 bytes |
| Tool | 39 | 351.9 bytes | 415 bytes |

The current database is small. IPC data volume is not the dominant latency source today, although retention is unbounded and is a long-term architectural risk.

### Frontend rendering

Spacesly uses Svelte 5, not React. There is no whole-tree React render pass. `$state` writes invalidate dependent effects and DOM bindings, while `$derived` recomputes from changed dependencies. The relevant issue was broad reactive ownership and object replacement, not missing React memoization.

| Path | Measurement | Conclusion |
| --- | ---: | --- |
| Activity projection, 120 retained logs | 0.895 ms mean before; 0.734 ms current rerun | Bounded and below one frame; do not virtualize |
| Synthetic session switch, 6 sessions | 0.005 ms mean at small messages | Data selection is not the source of switching delay |
| Serialize 6 sessions, 80 messages each, 4.62 MiB | 2.79 ms mean in Bun | Main-thread localStorage may matter only with unusually large message text |
| Unchanged progress burst, 10,000 events and 50 sessions | 200.82 ms before | Severe state amplification |
| Same burst through accumulator | 2.90 ms mean, zero reactive commits | 98.6% projection CPU reduction |
| Changing progress burst, 10,000 events | 31.52 ms accumulator CPU, one log, one commit | UI update count is independent of event count |

### Interaction traces

#### Open Agent Console

Path: local state change in `src/routes/+page.svelte`, preloaded `AgentConsolePanel`, one workspace grid layout, then bounded console DOM insertion.

- The console bundle is preloaded whenever a session exists.
- Activity rendering is capped at 10 items; raw logs are capped at 120.
- Opening still changes `workspace-body` from one to three grid columns, which can relayout the visible board.
- Exact layout/paint time is unavailable. Changing the panel to an overlay would alter UX and is not justified without a packaged WebKit trace.

#### Switch Task Session

Path: update `agentConsoleCardId`, perform O(1) session lookup, project at most 120 logs, reconcile at most 10 activity cards and four execution-plan rows.

Pure data selection measured 0.005 ms. Occasional stutter is more plausibly caused by concurrent event publications than lookup or cloning. The new accumulator removes that contention.

#### Progress and Latest Activity

Previously, every progress-bearing text delta could replace an Agent session and rebuild projections for all retained sessions. Activity logs were also reparsed after every appended raw event.

Now:

- Text deltas create no user-facing log.
- Repeated progress values preserve session identity.
- Progress/lifecycle/tool events merge for 16 ms before one state publication.
- Same-frame progress logs collapse to the latest event.
- Board projection updates only the changed card key.

#### Start Task

Start already projects immediate local feedback: the running-card set changes and the card moves to In Progress before trust, reservation, and capability setup complete. Backend setup remains asynchronous with rollback. No additional speculative optimization was justified.

## Findings and Priority

| Priority | Issue | User impact | Complexity | Expected gain | Action |
| --- | --- | --- | --- | --- | --- |
| Critical | Raw progress events amplified into broad reactive publications | Visible progress lag, console and board contention | Medium | Very high | Fixed |
| High | Board card projections rebuilt for every retained session | Unrelated cards become reactive consumers | Medium | High during bursts | Fixed |
| High | Agent Console opening relayouts the full board | Possible open/close delay on large boards | Medium | Unknown without paint trace | Measure in packaged build |
| High, latent | Task Session/event retention is unbounded | Startup and replay cost grows over weeks | High | High at future scale | Add server-side retention/index |
| High, latent | Agent text deltas may be persisted transaction-by-transaction | Backend write amplification | High | Depends on provider rate | Instrument transaction rate first |
| Medium | Full Activity projection reparses 120 logs | About 0.7-0.9 ms per semantic log update | Medium | Low at current bounds | Keep bounded; no rewrite |
| Medium | Full Task Session snapshots include immutable request payload | Repeated serialization during replay | High | Low at current 50 KiB scale | Introduce summary/detail APIs |
| Medium | Chat has three overlapping active-session states | Extra allocations and synchronization code | High | Moderate | Consolidate in dedicated store later |
| Medium | Hidden board/chat trees remain mounted after first use | Background DOM updates | Medium | Workload dependent | Add visibility-gated projections |
| Low | Board and message virtualization are absent | None at current caps | High | Negative/uncertain now | Do not implement |

## Root Causes

1. The root page owns execution detail, board summaries, visible console state, and persistence state together.
2. Low-level events were converted directly into `$state` writes.
3. A derived record reconstructed compact card state from every retained full session.
4. Unchanged progress values still produced new session object identities.
5. Notification coalescing occurs after durable backend event creation, so backend write volume and frontend presentation volume were coupled indirectly.

## Implemented Improvements

### Event aggregation layer

`src/lib/agentEventProjection.ts` now owns conversion from raw `TaskSessionEvent` values into a bounded presentation patch.

- Runtime text deltas are ignored unless they carry meaningful progress.
- Latest progress is retained monotonically.
- Lifecycle state is retained independently.
- Repetitive same-frame progress logs are replaced rather than appended.
- Applying an unchanged patch returns the original session object.

`src/routes/+page.svelte` holds pending projections in a non-reactive `Map` and publishes at most once every 16 ms per card.

### Compact board state

Board-facing `AgentTaskCardProjection` state is now maintained per card. Updating one Agent session no longer reconstructs projection objects for every retained session.

### Identity preservation

`setAgentProgressForCard` and projection application preserve object identity when the visible value is unchanged. This prevents downstream Svelte invalidation rather than merely making the invalidation cheaper.

## Before and After

| Metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| 10,000 unchanged events, projection CPU | 200.82 ms | 2.90 ms | -98.6% |
| Reactive session publications | 10,000 | 0 | -100% |
| Card projection objects rebuilt per publication at 50 sessions | 50 | 0 for unchanged; 1 for changed card | Up to -100% |
| 10,000 changing progress events in one burst | 10,000 commits/log candidates | 1 commit, 1 progress log | -99.99% publications |
| Activity projection at 120-log cap | 0.895 ms | 0.734 ms rerun | Within benchmark variance; frequency reduced instead |

The important result is not the reducer's raw CPU time. It is that renderer publication frequency is now bounded by presentation cadence rather than runtime-event cadence.

## IPC and Backend Architecture

Current safeguards:

- Task Session update hints coalesce to the latest sequence per session over 8 ms.
- Replay remains authoritative and sequence-based.
- Conversation startup hydration is one delayed batched IPC.
- PTY output already follows a strong 25 ms/32 KiB batching model.

Remaining backend opportunities, in order:

1. Add event-rate, payload-byte, SQLite lock, transaction, and notification-queue metrics.
2. Split lightweight Task Session summaries from full immutable envelopes.
3. Add age/count retention for terminal Task Sessions and journals.
4. Coalesce Agent text fragments before durable persistence only if production traces show token-level transaction amplification.
5. Return replay events with compact current state to remove a separate snapshot IPC.
6. Replace cancellation polling with update-subscription completion.

No backend event-generation change was made in this pass because the live store contains only 46 text-delta events and 53.9 KiB of total event payload. That does not prove backend persistence is causing the current UI lag.

## Remaining Measurement Plan

Capture a packaged, hardware-accelerated WebKitGTK Sysprof trace using representative data:

- 100-200 visible cards.
- 50 retained Agent sessions.
- 30-60 runtime events per second.
- Agent Console open/close and five rapid session switches.
- 1 MiB Chat output.

Record:

- input-to-next-paint latency;
- style/layout/paint duration when the console grid changes;
- long animation frames and dropped frames;
- DOM mutations per presentation commit;
- scheduler transactions and lock wait per second;
- IPC payload bytes and replay calls per second.

Only after that trace should Spacesly consider an overlay console, board virtualization, incremental DOM stream chunks, or backend token-event persistence changes.

## Validation

- Frontend tests include unchanged-progress identity and same-frame progress coalescing.
- Frontend tests: passed.
- Svelte diagnostics: 0 errors and 0 warnings.
- ESLint: passed.
- Production frontend build: passed.
- Rust tests: 235 passed.
- Rust build: passed with two existing dead-code warnings.
- Git whitespace validation: passed.
- No runtime, Task Session, scheduler, or backend behavior was changed by the implemented presentation aggregation.
