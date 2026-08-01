# Spacesly UI Performance Audit

## Scope and Method

This audit targets perceived responsiveness without changing Spacesly's design, colors, layout, or feature set.

Measurements used:

- Linux AT-SPI accessibility events for click-to-visible-tree response in the real Tauri/WebKitGTK window.
- Bun microbenchmarks for repeated timeline projection and reactive Chat run replacement.
- SQLite measurements against the live Spacesly databases for Task Session list size and query time.
- Process-tree sampling through `/proc` for current idle CPU and memory.
- Static tracing of every requested interaction from event handler through state mutation, rendering, IPC, and completion.

AT-SPI records the first externally observable UI-tree update. It does not expose WebKit's internal style, layout, paint, or compositor timestamps. WebKitGTK in this environment does not expose a Chrome DevTools Protocol endpoint, so exact paint-stage duration and dropped-frame counts are not available without adding production runtime telemetry. Those values are marked as unavailable rather than inferred.

## Interaction Audit

| Interaction | First feedback | Blocking work and render path | Result |
| --- | --- | --- | --- |
| Open Agent Console | Local state update | Console bundle, board grid update, panel render | Bundle now preloads when a session exists; the 180 ms full-board grid transition was removed. |
| Close Agent Console | Local state update | Panel removal and one grid layout | No backend wait. |
| Start Task | Immediate card state | Trust, reservation, grants, persistence, and execution IPC continue in the background | Existing immediate feedback retained. A distinct `starting` semantic state remains an opportunity. |
| Stop Task | Previously waited for cancellation IPC | Cancellation IPC and runtime cleanup | Now displays `Stopping...` and disables duplicate cancellation immediately; failed cancellation rolls state back. |
| Retry Task | Immediate card state | Same startup path as Start Task | Existing immediate feedback retained. |
| Expand/Collapse Activity | Local state update | Activity detail DOM insertion/removal | Timeline parsing is now one derived projection rather than two full projections per render. |
| Switch Task | Local selected ID | Visible card selection and detail render | No IPC wait. Lists remain bounded by default. |
| Switch Session | Local session state | Bounded copies: 6 sessions, 80 messages, 120 activities | No IPC wait; virtualization is not justified at current bounds. |
| Open Settings | Local state update | Settings form mount | Baseline p50 48.32 ms; after p50 46.08 ms. |
| Close Settings | Local state update | Settings form unmount | Baseline mean 8.44 ms; after mean 11.70 ms. Both remain below one 16.7 ms frame; the 3.26 ms difference is debug-run variance. |
| Save Settings | Previously no feedback and serial IPC | Secret/profile writes and three status reads | Immediate `Saving...` state added. Independent writes and status reads now run concurrently. |
| Task Progress Update | Runtime event | Session state, card projection, console render | Reduced from up to three session mutations per event to one. |
| Activity Timeline Update | Log event | Parse up to 120 logs, render latest 10 | Projection CPU reduced 44.7% in the controlled benchmark. |
| MCP Test Connection | Immediate `Testing...` state already existed | MCP subprocess/IPC in background | No change needed. |
| Workspace Change | Immediate mode state | Cold dynamic import, directory/terminal initialization | Existing loading states preserve feedback. Terminal resize work is now dimension-deduplicated and frame-coalesced. |

## Before and After

| Metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| Settings open, AT-SPI p50 | 48.32 ms | 46.08 ms | -4.6% |
| Settings close, AT-SPI mean | 8.44 ms | 11.70 ms | +3.26 ms, still under 16.7 ms |
| Latest Activity projection, 120 logs | 1.649 ms/render | 0.912 ms/render | -44.7% |
| Chat 120-delta burst state-update CPU | 0.0215 ms | 0.0017 ms | -92.1% |
| Chat 120-delta reactive replacements | 240 | 1 frame commit | -99.6% |
| Agent event session mutations | Up to 3 | 1 | Up to -66.7% |
| Board layout animation on console toggle | 180 ms layout animation | None | Removed |
| Resize reactive updates | Pointer-event frequency | At most once per animation frame | Bounded |
| Task Session update IPC hints | One per committed event | Latest sequence per session per 8 ms window | Burst-coalesced |
| Current idle CPU, debug process tree | Not collected on the baseline commit | 9.64% | Current value only |
| Current idle proportional memory | Not collected on the baseline commit | 383.4 MiB mean PSS | Current value only |
| Exact average frame time | Unavailable from AT-SPI | Unavailable from AT-SPI | Requires WebKit paint telemetry |
| Exact dropped frames | Unavailable from AT-SPI | Unavailable from AT-SPI | Requires WebKit paint telemetry |

The current idle process-tree CPU is dominated by the WebKit process at 7.75%, with the Spacesly process at 0.69% and WebKit networking at 1.21%. Development runs force software rendering through `LIBGL_ALWAYS_SOFTWARE=1`, so this is not representative of a hardware-accelerated packaged build.

## Render Profiling

### Highest-frequency render invalidation

Agent Task Session events previously updated progress, lifecycle state, and logs through separate root session mutations. Each mutation could rebuild all Agent card projections and invalidate board consumers. The event is now projected in one mutation, and retention scanning no longer runs when updating an existing session.

### Latest Activity

`AgentConsolePanel` previously called `timelineActivities(logs, 10)` twice in one render. At the 120-log retention limit, a same-process benchmark measured 1.649 ms for the two-call render path and 0.912 ms for the single derived projection.

### Chat streaming

Every text delta previously replaced the complete Chat runs record once for generic event state and again for stream buffering. Buffering is now non-reactive, while visible text commits once per animation frame. Lifecycle/progress state changes still update immediately.

### Large collections and virtualization

- Board lanes default to 40 visible cards and reveal another 40 progressively.
- Chat is capped at 80 messages and 6 sessions.
- Agent logs and transcripts are capped at 120 entries; Latest Activity renders 10.

Virtualization was not added because the default paths are bounded and current workspace data did not provide evidence that virtualization overhead would improve latency. The explicit `Show all` board path remains potentially unbounded and is the first candidate if real boards routinely exceed 100-200 visible cards.

## Main-Thread Blocking

Ranked by observed or structurally bounded impact:

1. **High: full-board grid layout animation.** Opening or closing the Agent Console animated `grid-template-columns` and `gap` for 180 ms, repeatedly laying out every visible lane and card. Removed.
2. **High: high-frequency reactive Chat replacement.** Two root-record replacements occurred for each text delta. Reduced to one visible commit per frame.
3. **High: Agent event fan-out.** One event could produce three session mutations and repeated collection-wide projection. Consolidated to one mutation.
4. **Medium: repeated timeline parsing.** The same 120 retained logs were parsed twice per render. Reduced to one derived projection.
5. **Medium: unthrottled layout resizing.** Pointer events cloned layout state at device event frequency. Coalesced to animation frames.
6. **Medium: terminal resize feedback.** `ResizeObserver` read `offsetHeight`, called `fit`, and issued resize IPC for every callback. It now uses `contentRect`, skips identical dimensions, and schedules one fit per frame.
7. **Medium: serial Settings persistence.** Independent keyring/profile writes and status reads were awaited sequentially with no visual saving state. Parallelized with immediate feedback.
8. **Medium: Task Session notification bursts.** Every durable runtime event emitted a separate Tauri event. Hints are now coalesced by session over 8 ms; durable journal semantics are unchanged.
9. **Low at current scale: Task Session full-list IPC.** The live database contains 26 retained sessions, 46,474 payload bytes, and a direct full-list query mean of 0.062 ms. Payload growth is a long-term concern, not a current query bottleneck.

No synchronous SQLite or filesystem operation was found running directly on the Svelte main thread. Those operations cross Tauri IPC. The dominant frontend costs were state amplification, parsing, DOM/layout work, and repaint-sensitive animation.

## State Management

The root page owns most application state. Svelte's fine-grained reactivity limits some damage, but replacing large records still invalidates broad projections.

Implemented changes:

- Existing Agent sessions update in place without running retention and reassigning the entire session record.
- One Task Session event produces one session mutation.
- Chat stream buffers are intentionally non-reactive.
- Timeline projection is a single derived value.

Remaining architectural opportunity:

- Move Agent sessions and card projections to keyed `SvelteMap` ownership so one session update only invalidates its card and visible console.
- Extract task detail and large Settings tab bodies into component boundaries if render profiling on large real boards shows broad invalidation.

## Animation and Transition Analysis

Implemented:

- Removed layout animation of Agent Console grid columns and gap.
- Changed progress animation from `width` to compositor-friendly `transform: scaleX()`.
- Capped drag resize state updates to one per animation frame.

Remaining:

- Large static dialog shadows and backdrop filters can increase paint cost, but were not changed because no paint trace proved they were material bottlenecks and visual styling must remain unchanged.
- Continuous status pulse and spinner animations are transform/opacity based and low risk individually.
- A packaged hardware-accelerated build should be profiled before changing any visual effects.

## IPC Analysis

Implemented:

- Task Session update hints now collapse queued events to the latest sequence per session within an 8 ms window.
- Buffered prompt text deltas are coalesced before durable journal insertion where event boundaries permit it; only the latest cumulative usage event is retained within a text group.
- Older terminal Chat sessions are no longer replayed on startup. Sessions that terminalize during hydration are still reconciled to close the conversation-load race.
- Settings secret/profile writes and subsequent status reads run concurrently.

Preserved invariants:

- Runtime events remain fenced and durable.
- Prompt output is still withheld until the durable Chat snapshot is revalidated, preventing stale output from becoming visible.
- Update events remain hints; the frontend still reads authoritative journal data.

Remaining IPC opportunities:

- Add a lightweight paginated Task Session index that excludes opaque request envelopes.
- Combine event pages with lightweight current-session metadata to remove the extra snapshot read after each replay pass.
- Replace 50 ms cancellation polling with the existing update subscription.
- Coalesce or batch durable runtime events at the producer when production traces show token-level event fragmentation.

## Progressive Rendering

The application already uses immediate loading shells for lazily loaded workspaces and the Agent Console. The Agent Console bundle now starts loading as soon as a session exists instead of waiting for the open click.

Prompt Chat output remains buffered until backend snapshot revalidation completes. This is a correctness boundary, not accidental frontend blocking. Live pre-validation streaming would expose potentially stale output and was therefore not enabled. A future implementation needs a conversation reservation or revocable speculative stream before this can change safely.

## Validation and Remaining Work

Validated:

- Rust tests: 234 passed.
- Frontend tests: passed.
- Svelte diagnostics: 0 errors and 0 warnings.
- ESLint: passed.
- Git whitespace validation: passed.

Recommended next measurements on representative production data:

1. Capture a WebKitGTK Sysprof trace in a packaged hardware-accelerated build with 100+ task cards.
2. Add development-only Event Timing and Long Animation Frame telemetry if exact click, render, paint, and dropped-frame segmentation is required continuously.
3. Reassess board virtualization only when traces show more than 100-200 simultaneously rendered cards.
4. Measure retained Task Session payload growth after several weeks of normal use before introducing a new paginated index contract.
5. Introduce safe speculative Chat streaming only with a revocation/reservation protocol that preserves durable conversation authority.

## Board Interaction Follow-up

Task creation and Start were subsequently traced to broad board invalidation and delayed phase projection:

- Active-board updates replaced the complete workspace root, invalidating unrelated workspace-derived state.
- Card moves recreated and filtered every lane, including lanes that did not contain the card.
- Start waited for workspace trust and Git discovery before projecting the card into In Progress.

The board now replaces only its active board, preserves untouched lane identities, and indexes each card's exact source column. Start projects an approved card into In Progress before asynchronous preflight work, with rollback if configuration, trust, reservation, or capability setup fails.

In a controlled 800-card board benchmark, moving one card improved from 0.0275 ms to 0.0164 ms of pure projection work, a 40.4% reduction. More importantly, lane objects recreated per move fell from four to two, preventing unrelated lanes and cards from receiving changed props.
