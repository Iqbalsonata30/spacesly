# Spacesly Agent Intelligence Progress

Last updated: 2026-08-13

Branch: `feat/agent-task-v2-foundation`

This file is the operational ledger for the roadmap in [agent-intelligence-roadmap.md](./agent-intelligence-roadmap.md). Update it when a phase changes status, a relevant commit lands, verification changes, or a new known gap is discovered.

## Current Position

- Completed through Phase 9: resource-level idempotency foundation.
- Completed through Phase 10: structured Rules, deterministic scope resolution, connector preflight, verification receipts, and contradiction detection.
- Phase 11 is complete with independent Git terminal-state, Kubernetes Deployment-availability, exact Bamboo build-result, Rules-bound Jira issue/comment-state, and Confluence page-existence verifiers.
- Phase 12 is complete with a replayable, headless production-policy corpus, deterministic scorecard across all four categories, and mandatory CI/release gates.
- Phase 13 is complete with backend-authoritative terminal causes and one bounded operator action for every blocked or failed Task Session.
- Phase 14 is in progress. Nine vertical slices now cover uncertain-mutation recovery fencing, bounded read-only connector recreation, prepared subtask contracts, scheduler-owned dormant identities, staged budget enforcement, fail-closed subtask lifecycle recovery, deterministic per-objective external and built-in capability narrowing, and exact per-objective connector-operation authority. Multi-agent dispatch remains disabled.

## Phase 14 In Progress

Completed ninth vertical slice: exact per-objective connector-operation authority.

- A connector capability is retained only when at least one immutable capability-plan tool matches both the objective resource and its normalized operation class. Read/search/inspect, create, update/write, delete, trigger, restart, and promote remain provider-neutral operation classes; snake case, kebab/namespaced, and camelCase tool names are normalized deterministically.
- Each resulting connector-to-tool allowlist is sorted, bounded, parent-contained, and sealed into version-2 content-addressed subtask contracts. Version-1 dormant contracts remain readable after restart but cannot be activated because they do not carry exact connector-operation authority.
- Scheduler activation copies the immutable operation map into the short-lived subtask descriptor. Admission revalidates the current parent fence, current parent grant, exact contract capability set, exact operation map, lease, and budgets.
- The MCP forwarding boundary passes the actual requested tool name into scheduler admission. An unlisted connector operation is rejected before budget consumption and before connector forwarding, even when another operation from the same connector is allowed.
- Built-in workspace, shell, and Git calls retain their existing category-specific admission and deeper containment/operation policies; connector tool identities cannot be smuggled into that built-in path.
- Durable Activity uses policy identity `objective_tool_operations_v3`, reports only the aggregate number of exact connector-operation grants, and explains the fail-closed forwarding boundary. Objective text, connector/tool names, arguments, responses, paths, commands, and credentials are not emitted.
- Specialized Worker dispatch remains disabled. This slice strengthens staged authority and does not claim that a second Worker is launched or that independent evidence is aggregated.

Phase 14 ninth-increment regression evidence:

- Ten domain tests cover separate read/mutation providers, ambiguous routing, unknown camelCase providers, operation-class mismatch, parent containment, deterministic ordering, and exact persisted allowlists.
- Scheduler tests cover exact read/update maps, rejection of an unlisted same-connector mutation before budget admission, immutable contract mismatch, concurrent budget enforcement, restart, renewal, cancellation, and terminal lifecycle behavior.
- MCP integration proves the requested connector tool name reaches exact scheduler enforcement and is rejected before stale-authority lookup or upstream forwarding when it is not listed.
- Full Rust suite: 533 passed, 3 ignored, 0 failed in serial mode; `cargo check` and Rust formatting passed.
- Frontend unit tests: 7 passed, 0 failed; `svelte-check`: 0 errors and 0 warnings.
- Focused rendered operator suite: 6 passed, 0 failed. Full rendered browser suite: 12 passed, 0 failed. Browser-only native-theme mock warnings remain unchanged and non-failing.

Known limitations:

- Operation matching is intentionally conservative and depends on bounded semantic objective hints plus the immutable discovered capability plan. Missing or ambiguous evidence removes authority rather than asking the model to widen it.
- Exact file paths, shell commands, and Git sub-operations remain governed by their existing built-in containment and policy layers rather than this connector-operation map.
- Scheduler-owned Worker launch, heartbeat orchestration, independent evidence aggregation, and production enablement remain future Phase 14 increments.

Completed eighth vertical slice: deterministic per-objective built-in workspace, file, shell, and Git authority.

- Prepared objectives now begin with no built-in authority. `workspace_read` requires objective-local workspace/file/source/configuration scope; external-only objectives and ambiguous connector objectives no longer inherit it.
- `workspace_write` additionally requires `mutation_expected=true` and an explicit file mutation signal such as create, edit, modify, update, write, replace, patch, or apply. Read-only wording cannot retain file-write authority.
- Shell requires local scope, mutation classification, and an explicit command/script/test/build/compile/lint operation. The local-scope condition prevents an external Bamboo-style build objective from inheriting shell merely because both use the word `build`.
- Git requires local scope and a Git-specific status/branch/stage/checkout/commit/push/pull/merge/rebase signal. Its admitted operation is still independently classified read or mutation at the task-tool boundary, so a zero mutation budget cannot be bypassed by possessing Git read authority.
- The allocator intersects only with the canonical built-ins already granted to the parent. Unknown/non-parent capabilities are never introduced, and all normalized objective grants remain content-addressed in the existing prepared contract.
- Scheduler integration activates two real staged contracts from one parent with all four built-ins: the read-file contract contains only `workspace_read` and is rejected for `workspace_write`; the mutation file contract contains only read/write and consumes its bounded mutation budget successfully.
- The durable safe policy identity is now `objective_capability_signals_v2`. Activity reports deterministic per-objective capability subsets and explains the local-scope plus mutation gate without retaining matched terms, objectives, paths, commands, capability names, or credentials.
- Specialized Worker dispatch remains disabled, so this changes only dormant/staged subtask contracts and does not alter current single-Worker execution.

Phase 14 eighth-increment regression evidence:

- Nine domain tests cover local read, file update, repository test, Git commit, external Bamboo build isolation, read-only write/shell rejection, connector ambiguity, unknown connector naming, parent containment, and deterministic contract identity.
- Scheduler integration proves exact built-in activation/admission and mutation-budget enforcement; the existing three lifecycle tests continue to pass with narrowed contracts.
- Agent executor integration suite: 55 passed, 0 failed. Full Rust suite: 532 passed, 3 ignored, 0 failed in serial mode; `cargo check` and Rust formatting passed.
- Frontend unit tests: 7 passed, 0 failed; `svelte-check`: 0 errors and 0 warnings.
- Focused rendered operator suite: 6 passed, 0 failed. Full rendered browser suite: 12 passed, 0 failed.

Completed seventh vertical slice: deterministic per-objective external connector grant narrowing.

- Each prepared objective starts from the normalized parent grant set but retains an external connector only when its own bounded summary/evidence/hints share a non-generic signal with exactly one immutable connector-plan entry.
- Connector evidence is derived from connector IDs, matched domains, matched intents, and matched tool names. Signal normalization handles provider-neutral snake case, kebab/namespaced names, and camelCase without hard-coding Jira, Confluence, Bamboo, Kubernetes, or another provider.
- Common operation words do not establish authority. A signal shared by multiple planned connectors is ambiguous and grants neither; missing or malformed connector-plan evidence also grants no external authority.
- The compiler intersects only with capabilities already held by the parent Task Session. Model-produced semantic hints can narrow the set but cannot introduce an ungranted or unplanned connector.
- Non-connector built-in capabilities remain parent-bounded for compatibility in this slice. Connector grants are still connector-wide rather than exact per-tool operation allowlists.
- The normalized objective grant vector remains part of the content-addressed prepared contract. Existing scheduler activation, grant revalidation, atomic budget admission, exact renewal, and lifecycle recovery enforce that vector without widening it.
- The durable preparation event exposes only `objective_connector_signals_v1`, parent and aggregate grant counts, and the number of narrowed objectives. It contains no objective text, capability/connector/tool names, success evidence, arguments, responses, or credentials.
- Activity now explains `deterministic per-objective connector subset`, reports the safe narrowed-objective count, and states that ambiguous evidence grants no connector authority. Specialized Workers and the activation gate remain disabled.

Phase 14 seventh-increment regression evidence:

- Domain tests cover separate Confluence/Bamboo-style objectives, parent-intersection enforcement, ambiguous connector signals, missing evidence, parent-order stability, content-addressed identity stability, and a provider-neutral unknown connector with a camelCase operation.
- Scheduler activation/lifecycle tests continue to prove that narrowed contract grants are the exact authority admitted at tool boundaries.
- The Agent executor integration suite passed all 55 tests. A real blocked/continued Task Session verifies safe aggregate narrowing evidence and confirms objective/success-evidence text is absent from the event.
- Full Rust suite: 529 passed, 3 ignored, 0 failed in serial mode; `cargo check` and Rust formatting passed.
- Frontend unit tests: 7 passed, 0 failed; `svelte-check`: 0 errors and 0 warnings.
- Focused rendered operator journey: 6 passed after replacing one stale assertion for the former parent-wide wording. Full rendered browser suite: 12 passed, 0 failed.

Completed sixth vertical slice: staged subtask dispatch lifecycle and fail-closed recovery.

- Exact lease renewal reopens the authority's scheduler instance, verifies the current parent attempt/fence/owner/session, revalidates the immutable subtask contract and current parent grants, and returns a new descriptor whose lease cannot exceed the parent lease. The old descriptor cannot renew or call tools afterward.
- Completion, cancellation, and failure are explicit fenced transitions. An identical exact-descriptor replay returns the persisted terminal result; a different outcome, forged identity, stale fence, or incompatible stored row fails closed.
- Successful completion requires an unexpired child lease and live exact parent authority. Cancellation and failure revoke without claiming completion. Terminal rows retain only fixed lifecycle reasons, timestamps, and consumed budget counters.
- Parent cancellation and result resolution revoke active subtasks atomically with the parent transition. Periodic lease recovery, claim-time recovery, and owner abandonment also recover child rows, using `lease_expired` when the child lease elapsed and `parent_inactive` when its exact parent is no longer live.
- Restart-safe status queries expose the bounded lifecycle projection without granting tool access. Migration adds nullable terminal timestamp/reason columns so existing dormant and active records remain compatible.
- The executor's preparation event and rendered Activity now report `Dispatch lifecycle: staged`, `Lease recovery: fail closed`, and that lease expiry, cancellation, or parent completion revokes staged authority. The existing `Activation gate: closed` statement remains visible.
- The private dispatch permit is still inaccessible to the application executor and Worker pool. This slice proves scheduler lifecycle behavior only; it does not start specialized Workers or advertise multi-agent execution.

Phase 14 sixth-increment regression evidence:

- Scheduler lifecycle tests cover first renewal, stale old descriptors, double-renew fencing, post-renew admission, exact completion replay, conflicting and forged terminal outcomes, explicit cancellation, post-terminal rejection, restart-safe status, expired-completion rejection, idempotent expiry recovery, parent cancellation/resolution, and owner abandonment.
- Existing activation tests continue to cover exact authority binding, parent-grant containment, persisted budget accounting, and concurrent admission limits. Legacy migration coverage verifies the terminal lifecycle columns.
- Full Rust suite: 526 passed, 3 ignored, 0 failed in serial mode; `cargo check` and Rust formatting passed.
- Frontend unit tests: 7 passed, 0 failed; `svelte-check`: 0 errors and 0 warnings.
- Full rendered browser suite: 12 passed, 0 failed, including the staged subtask lifecycle explanation.

Completed fifth vertical slice: scheduler-only subtask activation and atomic tool-budget admission.

- A private `SubtaskDispatchPermit` gates activation. Only scheduler-store code and focused tests can construct it; the application executor and Worker pool have no activation or subtask-dispatch call site.
- Activation is one immediate SQLite transaction over the current parent assignment fence and the exact dormant subtask tuple. It persists an independent authority identity/fencing token and a lease bounded to 30 seconds and the remaining parent lease.
- Identical live activation returns the same authority. Stale dormant tuples, expired/cancelled parents, a different parent attempt, invalid stored state, or an authority requiring recovery are rejected.
- Activation and every admission recheck that contract capabilities remain present in the current parent Task Session grant table. Retained contracts therefore cannot preserve a capability removed during continuation.
- `TaskToolAuthority` and `ExternalAssignmentAuthority` may carry a nested subtask descriptor. Workspace and MCP forwarding require that its scheduler, session, parent attempt, owner, and fence exactly match the containing parent authority.
- Workspace read/write, shell, Git and all proxied MCP calls run atomic budget admission before forwarding. Read calls consume one tool call; every other risk consumes one tool call and one mutation call. Git `info` and `status` are classified read-only; other Git operations are mutations.
- Admission uses an immediate transaction and fenced compare/update, charges before execution, survives reopen, and cannot exceed the immutable general or mutation budget under concurrent callers.
- Cancellation, parent lease expiry, scheduler-instance mismatch, stale nested fences, unbound nested descriptors, capability expansion, parent grant revocation, and budget exhaustion all fail before the external side effect.
- The operator Activity still states that no Worker or tool authority is active. It now also shows `Activation gate: closed` and explains that tool budgets will be charged atomically before forwarding after dispatch is enabled.

Phase 14 fifth-increment regression evidence:

- Scheduler tests cover idempotent activation, exact/stale dormant fencing, independent mutation and general budgets, reopen persistence, parent grant revocation, parent cancellation, stale authority fencing, capability expansion, and a concurrent admission race that stops exactly at the contract budget.
- Workspace and MCP integration tests attach an unbound nested descriptor and prove rejection before file or connector access; MCP additionally rejects a nested descriptor whose parent fence differs from its containing authority.
- Legacy migration testing confirms the new authority table is created alongside prepared-subtask tables.
- Full Rust suite: 524 passed, 3 ignored, 0 failed in serial mode; `cargo check` and Rust formatting passed.
- Frontend unit tests: 7 passed, 0 failed; `svelte-check`: 0 errors and 0 warnings.
- Focused rendered dormant-subtask journey: 1 passed. Full rendered browser suite: 12 passed, 0 failed.

Completed first vertical slice: uncertain-mutation recovery fencing.

- Lease-expiry and scheduler-owner recovery remain one transaction: the old attempt is interrupted, its authority becomes stale, unresolved mutation reservations become `uncertain`, and the Task Session transition is committed with its durable event.
- If recovery discovers any unresolved external mutation, the Task Session now becomes `blocked` before queue selection can issue replacement Worker authority. The retained event records only a safe recovery class and mutation count; it does not copy mutation arguments, provider responses, task content, runtime output, URLs, or credentials.
- The existing immutable request, capability grants, durable OpenCode identity, mutation ledger, and prior attempt evidence remain attached to the same Task Session. Operator guidance resolves the latest canonical uncertain mutation and offers only exact fence review/release.
- Recovery precedence is deterministic: cancellation stays terminal; otherwise an uncertain mutation requires operator reconciliation; otherwise a missing Agent runtime identity requires Retry Fresh; mutation-free recovery remains eligible to resume under a new assignment fence.
- The Activity timeline presents this state as `Recovery Paused Safely`, explicitly states that the old authority was revoked and no replacement Worker resumed, and keeps the exact reconciliation action in the authoritative attention panel.

Phase 14 first-increment regression evidence:

- Focused scheduler recovery tests cover owner shutdown, lease expiry, preserved runtime identity and capability grants, retained mutation identity, blocked reassignment, stale-attempt rejection, mutation-free resume, and missing-runtime Retry Fresh behavior.
- Frontend projection tests verify that operator reconciliation remains visible in the user-facing timeline instead of collapsing into a generic failure.
- The rendered browser journey creates a representative interrupted deployment task, confirms that Continue and Retry Fresh are unavailable, opens the exact Jira mutation fence, supplies an operator reason, and verifies that releasing the fence does not retry the task.
- Browser testing found and fixed a generic Activity presentation that hid the recovery cause and an incoherent fixture that displayed mutually exclusive approval and reconciliation states together.
- Full Rust suite: 511 passed, 3 ignored, 0 failed; `cargo check` and Rust formatting passed.
- Frontend unit tests: 7 passed, 0 failed; `svelte-check`: 0 errors and 0 warnings.
- Focused rendered operator-guidance suite: 4 passed, 0 failed. The unchanged broader browser scenarios executed through test 9 before this environment dropped the reporter process; no full-suite pass is claimed for this increment.

Completed second vertical slice: bounded read-only connector-session recovery.

- The provider-neutral MCP evidence boundary can recreate a transport-invalidated stdio connector process once and replay the exact read-only request within the caller's original deadline. The tool name, arguments, connector binding, and authorization are unchanged between attempts.
- Tool Broker risk classification rejects mutation tools before connector spawn. Provider errors, authentication/authorization failures, invalid requests, and exhausted deadlines are not connector-session retries.
- The result exposes only `connector_attempts` and `session_recovered` beside the validated response. Environment values remain redacted from errors, and command/environment configuration is absent from durable recovery evidence.
- Exact Confluence page verification is the first end-to-end adapter. After successful recreation it emits a durable `connector_session_recovered` event before parsing and verifying the same immutable page ID.
- The execution trace indexes this event and projects only `connector_session_recreated`; connector configuration and raw provider responses are not copied into the trace.
- The Activity timeline presents `Connector Session Recovered`, explains that the read-only verification resumed safely, and states that no mutation authority was replayed or expanded.

Phase 14 second-increment regression evidence:

- MCP process simulation verifies immediate stdout loss, one fresh-process recreation, exact Confluence read replay, success on attempt two, no retry for a provider validation error, bounded deadline behavior, mutation rejection before spawn, and secret redaction.
- Execution-trace tests verify durable indexing and ensure connector configuration, prompt text, tool errors, and runtime deltas are excluded from the safe trace projection.
- The rendered browser journey shows the completed Confluence verification, visible recovery activity, two connector attempts, read-only risk, and the non-expansion of mutation authority.
- Focused rendered operator-guidance and connector-recovery suite: 5 passed, 0 failed.
- Full Rust suite: 513 passed, 3 ignored, 0 failed in serial mode. One initial parallel run had a timing-only governance performance assertion; its focused rerun passed and the serial suite passed.
- Frontend unit tests: 7 passed, 0 failed; `svelte-check`: 0 errors and 0 warnings.

Completed third vertical slice: prepared, non-executable subtask authority contracts.

- Every immutable semantic objective is deterministically projected into one versioned subtask contract before the Worker starts. Contract identity binds the parent contract digest, objective ID, normalized capability grant set, budget, evidence-requirement digest, delegation depth, and execution state.
- A subtask may retain only capabilities already present in the parent Task Session's durable grants. Compilation rejects added capabilities, malformed authority, excessive budgets, duplicate objective identities, and any request for delegable authority.
- Each contract receives independent wall-clock, tool-call, and mutation-call budgets from bounded aggregate limits. Read-only objectives receive no mutation-call budget.
- Success-evidence text is hashed rather than duplicated into the prepared authority record. The existing immutable execution contract remains the semantic source, while the subtask record retains only its evidence digest and source class.
- Prepared contracts persist inside the fenced Task Examination/Execution Manifest. Retained-manifest validation rechecks parent-grant containment, unique identities, aggregate budgets, non-delegation, and the disabled execution state.
- Retained Agent contracts created before semantic planning prepare no subtask authority and continue through the existing single-Worker path unchanged.
- A durable `subtask_contracts_prepared` event and safe execution-trace entry expose counts and aggregate budgets without contract content, capability names, objective text, evidence text, or credentials.
- The Activity timeline shows `Subtask Authority Prepared`, explicitly says execution remains disabled, and confirms that no additional Worker was started.

This increment does not execute subtasks. Current prepared contracts conservatively retain the parent grant set, which is a valid non-expanding subset but not yet per-objective least privilege. Operation-level budget enforcement, independent scheduler attempts/fences, deterministic per-objective capability narrowing, evidence aggregation, and concurrent dispatch remain required before multi-agent execution can be enabled.

Phase 14 third-increment regression evidence:

- Domain tests cover deterministic contract identity, independent objective contracts, bounded mutation budgets, capability-expansion rejection, delegable-authority rejection, stable capability normalization, and evidence-text redaction.
- Retained Task Examination validation rejects a prepared contract whose capability set is altered beyond the parent grant catalog.
- Execution-trace tests retain the safe `subtask_authority_prepared` status while excluding private contract content.
- Frontend projection tests verify the non-delegable and non-executing user explanation.
- The rendered browser journey shows two prepared subtask contracts, aggregate budgets, parent-bounded authority, non-delegation, disabled execution, and a still-running parent task. The final focused browser suite passed all 6 scenarios; an earlier run had one existing approval startup race that passed immediately in isolation and in the final suite.
- The first full regression run exposed and then drove a compatibility fix for retained contracts without `semantic_plan`; they now produce no synthetic subtask contracts instead of failing preflight.
- Final full Rust suite after the compatibility fix: 518 passed, 3 ignored, 0 failed in serial mode.
- Frontend unit tests: 7 passed, 0 failed; `svelte-check`: 0 errors and 0 warnings.
- Production frontend build, Rust compilation, Rust formatting, and focused Prettier checks passed.

Completed fourth vertical slice: scheduler-owned dormant subtask records and independent fence identities.

- Execution Manifest binding now persists prepared contracts into normalized scheduler tables in the same immediate SQLite transaction. A failed subtask allocation cannot leave a committed manifest, and the user-visible preparation event is emitted only after the committed records are read back.
- Each semantic objective owns a stable scheduler subtask identity and an independent dormant attempt tuple: subtask ID, subtask-attempt ID, attempt number, and fencing token. Exact tuple lookup rejects a stale token and a fence assembled from two different subtask identities.
- Repeating the same manifest binding returns the same records without inserting duplicate subtasks or attempts. Query-only reopen restores the same records after process restart.
- Rebinding fails closed if the contract/objective set or the stored dormant allocation changes. Loaded rows must match the contract budgets, remain in `prepared`/`dormant` state, have zero usage, and keep authority inactive.
- Database constraints keep prepared execution and dormant authority disabled. The runtime has no activation or dispatch path for these records, so a valid dormant fence is audit identity only and cannot authorize a tool call.
- Only digested success-evidence requirements are stored in subtask contracts. Tests inspect the database file and confirm that sensitive source evidence is absent.
- Activity now reports `Scheduler state: dormant`, the number of dormant fencing identities, and `Tool authority active: no`, alongside the existing parent-bound, non-delegable budget explanation.

Phase 14 fourth-increment regression evidence:

- Scheduler tests cover atomic first bind and rollback, identical retry, independent identities, exact and stale/mixed fence matching, incompatible stored allocation rejection, sensitive-evidence redaction, and query-store reopen/restart compatibility.
- Legacy-schema migration testing confirms both normalized subtask tables are created without losing prior scheduler data.
- Agent-executor suite: 55 passed, 0 failed. Focused scheduler/migration tests: 4 passed, 0 failed.
- Full Rust suite: 521 passed, 3 ignored, 0 failed in serial mode; `cargo check` and Rust formatting passed.
- Frontend unit tests: 7 passed, 0 failed; `svelte-check`: 0 errors and 0 warnings.
- The focused rendered dormant-subtask journey passed, then the complete rendered browser suite passed all 12 scenarios.

Remaining Phase 14 work:

- Add a scheduler-owned specialized-Worker dispatch and heartbeat path only after its end-to-end authority transfer, cancellation, recovery, and shutdown behavior are proven. Until then the private activation permit remains closed.
- Narrow connector authority to exact per-objective tool operations; connector-level and built-in-category narrowing are now implemented.
- Independently verify and aggregate subtask evidence into the parent result before accepting parent completion.
- Keep multi-agent dispatch disabled until authority, budget, fence, cancellation, recovery, and evidence isolation pass end-to-end tests.
- Expand the long-running corpus beyond operations already protected by the resource-mutation ledger.

## Phase 13 Completed

Implemented authoritative operator guidance:

- `get_task_session_operator_guidance` combines the scheduler snapshot with the latest valid uncertain resource-mutation fence and returns no guidance for non-blocked sessions.
- Stable causes are `approval_required`, `mutation_outcome_uncertain`, `retry_fresh_required`, `execution_interrupted`, and `execution_blocked`.
- Each cause maps to exactly one action: `approve`, `supersede_mutation`, `retry_fresh`, or `continue`. Approval takes precedence, followed by an uncertain mutation fence, missing resumable identity, and ordinary interrupted/blocked execution.
- The projection contains only schema/session identity, cause code, a fixed safe summary, source class, action label/confirmation requirement, and an optional canonical mutation ID/key/revision. It never returns the raw scheduler error, provider response, connector configuration, arguments, URLs, task content, or credentials.
- The Agent console loads this projection only for attention states, shows its cause and source, and offers the backend-selected action. When authoritative guidance exists, the legacy manual-completion shortcut is hidden.
- Approval still uses exact operation/argument UI approval. Mutation supersede still requires an operator reason plus expected operation key and revision. Continue and Retry Fresh invoke the existing guarded task actions so ownership and capability checks remain authoritative.
- Connector/tool choice and repository/environment selection remain inspectable through the existing immutable Task Examination, manifest, context, MCP, trace, and mutation projections.

Phase 13 regression evidence:

- Focused cause priority, approval redaction, uncertain-fence identity, continuation/retry selection, and non-terminal omission tests: 5 passed.
- Browser user-flow coverage creates a representative blocked deployment task and drives all four authoritative actions through the rendered Agent console: structured approval, same-session continuation, Retry Fresh, and exact uncertain-fence review/release.
- The browser diagnosis found and fixed duplicate approval actions, Continue/Retry Fresh buttons that only opened the task instead of performing their named action, missing visual emphasis for the backend-selected mutation fence, and stale Deployment-scaling-only ledger copy.
- Exact fence release is tested with its session ID, mutation ID, operation key, expected revision, and operator reason; the UI confirms the fence was superseded and that no task was retried.
- Full Playwright suite: 10 passed, 0 failed in deterministic serial mode. Serial execution prevents Vite/browser startup saturation from producing unrelated navigation timeouts on constrained CI runners.
- Full Rust suite: 510 passed, 3 ignored, 0 failed in serial mode.
- Frontend unit tests: 7 passed, 0 failed; Rust compilation and Svelte type checking passed.

## Phase 12 Completed

Implemented recovery, result-evidence, planning-proposal, Rules-compilation, deployment/repository/connector preflight, and task-tool containment corpus slices:

- `src-tauri/evaluation-fixtures/runtime-recovery-v1.json` is the first versioned, provider-neutral corpus. Its fifty-four cases cover bounded recovery policy, production model-result evidence enforcement, semantic-planning proposal validation, dynamic Rules compilation, Rules-bound deployment/repository/connector preflight, and task-tool containment.
- The evaluator invokes the production recovery decision function directly and compares its failure class, recovery action, and retryability with each fixture's expected terminal policy.
- Reports contain stable corpus/fixture identities, mismatch field names, exact totals, and basis-point pass rates. They never reproduce fixture errors, task text, diagnostics, or secrets.
- Model-result fixtures invoke the production structured-response and objective-coverage validator. Valid complete evidence succeeds, while malformed JSON, omitted objectives, duplicate objectives, evidence-free objectives, and sensitive completion without approval block.
- Planning fixtures invoke the production semantic-planning proposal parser. They verify bounded objective decomposition, stable objective IDs, normalized non-authoritative hints, mutation classification, and fail-closed handling of malformed, empty, or evidence-free proposals.
- Rules fixtures invoke the production domain compiler with generic user-defined repositories and environments. They verify repository identity, missing-local-checkout warnings, protected-branch approval policy, deployment targets, connector/verifier policy identities, and preservation of conflicting target rows for later preflight diagnostics.
- Deployment-preflight fixtures invoke the production resolver after compiling user Rules. Exact labels, explicit targets, and agreeing combined selectors resolve to their bound branch and namespace; conflicts, unknown targets, ambiguous labels, and invalid Rules targets block with retained selector status.
- Repository fixtures create isolated temporary Git layouts, compile generic user Rules, and invoke the production repository resolver. Exact Rules paths and unique bounded discovery resolve; ambiguous checkouts, outside-workspace paths, contract/Rules conflicts, missing checkouts, and unselected multiple Rules block. Reports retain normalized status only and temporary layouts are removed after each fixture.
- Connector-preflight fixtures compile generic Connector Rules and invoke the production configuration/capability resolver with sanitized connector configuration and live inventory snapshots. They cover matching URL and operations, missing Rules/configuration, URL mismatch, unavailable discovery, missing operations, and ambiguous operation names.
- Connector-preflight reports retain only fixture identity and mismatched normalized field names. Rules text, configured URLs, inventory names, connector environment, diagnostics, and credentials are never copied into a failure report.
- Task-tool fixtures invoke production workspace-read, shell-workdir, and Git-file path resolution inside isolated temporary layouts. Contained relative/absolute paths succeed while parent traversal, absolute escape, and symlink escape are rejected. Non-Unix evaluation uses an equivalent absolute outside path because symlink creation is not portable.
- All roadmap score categories are represented. The current run reports planning 4/4, safe execution 42/42, recovery 3/3, and evidence quality 5/5.
- Run the release-safe headless command with `spacesly --spacesly-evaluate-agent`; it prints JSON and returns a non-zero exit code if parsing, validation, or any fixture fails.
- Pull-request/main CI runs the evaluator after the Rust suite, and release workflows evaluate the exact tag/ref before any release build begins. Both publish the JSON scorecard as a workflow artifact while preserving non-zero evaluator exits.
- Duplicate or malformed fixture IDs, unsupported schema versions, empty/oversized errors, excessive retry policy, and empty/oversized corpora fail closed.

Phase 12 regression evidence:

- Focused corpus parsing, scoring, uncovered-category, tampered-expectation, duplicate-fixture, production-validator, containment, connector-preflight, and report-redaction tests: 10 passed.
- Headless embedded corpus: 54 passed, 0 failed; planning, safe execution, recovery, and evidence quality 100%.
- Full Rust suite: 510 passed, 3 ignored, 0 failed in serial mode; `cargo check` and formatting passed.
- Frontend unit tests: 7 passed, 0 failed; `svelte-check`: 0 errors and 0 warnings.
- Clippy introduced no finding; the same three pre-existing warnings remain.

Future Phase 12 corpus expansion:

- Planning coverage currently measures deterministic proposal-schema enforcement, not whether a live model semantically decomposes an arbitrary task correctly; curated task-to-plan model fixtures remain future work.
- Rules compilation, deployment-target resolution, repository checkout containment, and deterministic connector configuration/capability preflight are covered. The corpus uses sanitized snapshots and does not spawn real connector processes or exercise discovery transport failures.
- Task-tool read/workdir/Git-file containment is covered; mutation-time directory replacement races remain covered by focused file-service tests rather than the headless corpus.
- Existing production tests cover approval continuation, mutation-ledger replay/uncertainty, and provider evidence adapters; adding those paths to the headless JSON corpus remains useful expansion rather than an exit blocker.
- Add connector-process simulators and curated live-model fixtures as the corpus matures.

## Phase 11 Completed

Implemented Git evidence-verifier vertical slice:

```markdown
## Evidence Verifier: git-release-state

- Provider: git
- Applies to labels:
  - RELEASE
- Required states:
  - clean_worktree
  - new_commit
  - pushed_upstream
```

- Evidence Verifier Rules are compiled with source provenance and bound only when their provider and optional labels apply to the immutable task.
- The initial provider adapter is Git because Spacesly can independently inspect the exact repository already resolved inside workspace authority.
- `clean_worktree` checks porcelain status, `new_commit` compares `HEAD` with immutable `repository.head_commit`, and `pushed_upstream` verifies that the configured upstream contains `HEAD`.
- Terminal evidence contains only state names and `satisfied`, `unsatisfied`, or `unavailable` status. Command output, file content, remote URLs, and credentials are never retained.
- Missing repository authority, missing commit baseline, invalid states, duplicate verifier IDs, and applicable unsupported providers block before execution.
- Unsatisfied terminal states retain prior mutation/tool events and block acceptance of the model's completed result.

Known limitation of this slice:

- `new_commit` proves that repository `HEAD` changed from the immutable task baseline, but does not yet attribute the commit contents to a specific semantic objective.

Implemented Kubernetes Deployment availability vertical slice:

```markdown
## Evidence Verifier: deployment-health

- Provider: kubernetes
- Required states:
  - deployment_available
- Poll interval seconds: 5
- Timeout seconds: 120
```

```json
{
  "deployment": {
    "target": "prerelease",
    "workload": "payroll-api"
  }
}
```

- The immutable contract identifies one canonical Deployment workload, while deployment-target Rules resolve and bind its namespace.
- The verifier reuses the trusted embedded OCP connector configuration after task authority is applied; it does not consume model-controlled MCP output.
- Namespace equality is checked before credential/configuration loading or API access, and unsafe resource identities are rejected before I/O.
- Availability requires `status.observedGeneration >= metadata.generation` and updated, ready, and available replicas all to equal desired replicas.
- Durable evidence contains only resource kind, replica counters, generation-observed status, and satisfied/unsatisfied/unavailable state.
- Missing target/workload authority, unavailable reads, stale generations, and incomplete replicas fail closed.
- Rules may optionally provide both a poll interval and rollout timeout. Values are bounded to 1–30 seconds and 1–600 seconds respectively, with the interval no greater than the timeout; invalid or partial policies block during preflight.
- Rule facts compiler v7 persists typed polling plus external connector/read-operation bindings; retained v1–v6 task snapshots remain valid under their original immutable semantics.
- A progressing Deployment is reread until it becomes available or reaches the deadline. Connector/RBAC failures are not blindly retried, every wait rechecks assignment authority, and each API request is capped by the remaining deadline.
- Omitting both polling fields preserves the one-snapshot verifier behavior.

Implemented Bamboo exact build-result vertical slice:

```markdown
## Evidence Verifier: bamboo-build-state

- Provider: bamboo
- Connector: corporate-bamboo
- Read operation: get_build
- Required states:
  - successful_build
- Poll interval seconds: 5
- Timeout seconds: 120
```

```json
{
  "build": {
    "provider": "bamboo"
  }
}
```

- The verifier binds one exact user-defined Bamboo connector, one live-discovered read-only MCP tool, and one supported result-key argument before execution.
- `build.provider=bamboo` is required. An optional immutable `build.result_key` can pin a known build; otherwise Spacesly obtains the identity from a trusted trigger receipt.
- Completed canonical and MCP-namespaced `bamboo_trigger_build` calls must contain one canonical result key, or a canonical plan key plus build number, in structured JSON. Prose-only output and successful responses without identity become failed tool events.
- The secret-free `{provider, resource_kind, resource_id}` reference is carried through the runtime event, successful receipt, objective checkpoint, and exact checkpoint replay.
- The verifier resolves the build from current or retained receipts. Multiple captured builds, or disagreement between a receipt and immutable contract key, block deterministically.
- After the worker returns, Spacesly performs a separate MCP read and accepts success only when structured JSON contains the resolved result key and a normalized successful terminal state.
- Optional Rules-controlled polling rereads only an in-progress build. Interval and timeout use the existing 1–30 and 1–600 second bounds, every wait rechecks assignment authority, and every MCP request is capped by the remaining deadline.
- Failed builds, timeouts, mismatched identities, unknown/prose responses, mutation-classified read tools, missing connector capability, and transport errors block completion. Connector errors are not blindly retried.
- Durable evidence contains only connector ID, exact build result key, identity source, attempt count, and normalized state. Raw MCP output, connector diagnostics, credentials, and unrelated response fields are not retained.
- Rule facts compiler v7 persists Bamboo polling and Jira state predicates; retained v1–v6 task snapshots remain valid under their original immutable semantics.
- Known limitation: trigger recognition currently covers the canonical `bamboo_trigger_build` adapter name and MCP namespace prefixes. The event is durable immediately, but its identity becomes authoritative continuation input only when an objective checkpoint is committed; this increment does not yet provide pre-trigger lookup or an uncertain-mutation fence if execution is interrupted before that checkpoint.

Implemented Jira exact issue-status vertical slice:

```markdown
## Evidence Verifier: jira-in-progress

- Provider: jira
- Connector: corporate-jira
- Read operation: get_issue
- Required states: expected_status
- Expected status: In Progress
```

- The immutable contract must identify one canonical Jira ticket key; issue authority is not extracted from free-form task prose.
- Rules own the expected status and exact connector/read-operation binding. Live discovery must yield one read-only tool with exactly one supported `issue_key`, `issueKey`, or `key` argument.
- Spacesly reads the issue independently after the worker returns and accepts completion only when structured JSON contains the exact issue key and one unambiguous status matching the Rules value case-insensitively.
- Different issue identities, multiple statuses, mismatches, prose-only output, unknown shapes, missing resources, invalid Rules, and connector errors block safely.
- Durable evidence contains only connector ID, issue key, expected status, normalized observed status, and satisfied/blocked state. Raw responses and connector diagnostics are discarded.
- Rule facts compiler v7 persists Jira issue-status and comment-state policies; retained v1–v6 snapshots remain valid under their original immutable semantics.
- Known limitation: the expected issue status is currently Rules-defined, so dynamic per-task transition targets require separate Rules scoped by labels.

Implemented Jira exact comment-state vertical slice:

```markdown
## Evidence Verifier: jira-comment

- Provider: jira
- Connector: corporate-jira
- Read operation: get_issue
- Required states: comment_matches
```

- Canonical and MCP-namespaced `jira_add_comment` or `jira_create_comment` tool results must provide one structured comment ID. Prose-only, missing, or multiple IDs make the completed mutation event fail closed.
- Spacesly derives the parent issue key and normalized desired-content fingerprint from trusted structured tool arguments. Only the SHA-256 fingerprint is retained; the comment body is absent from events, receipts, checkpoints, and verifier evidence.
- The provider-neutral external-resource receipt now supports an optional parent resource and state fingerprint. Scheduler validation permits that shape only for a trusted Jira comment tool and rejects raw content, malformed IDs/fingerprints, mismatched providers, or missing evidence.
- Objective checkpoints durably retain the issue/comment/fingerprint evidence and exact replay remains idempotent. Continuations resolve the same reference from current or retained receipts; multiple comments or a cross-issue receipt block.
- Terminal verification rereads the exact Jira issue through the Rules-bound read-only tool and requires the exact comment ID plus matching normalized plain-text or Atlassian Document Format content.
- Durable result evidence contains only connector ID, issue key, comment ID, and satisfied/content-mismatch/conflict/unavailable state. Raw connector responses and errors are discarded.
- Worker-issued Jira add/create-comment calls now derive a deterministic resource mutation identity before connector dispatch from the connector binding, canonical issue key, and normalized-content fingerprint.
- The fenced MCP proxy reserves that identity in the scheduler ledger before forwarding the call. It resolves confirmed structured comment IDs before returning output to the worker and injects trusted mutation evidence into the structured connector result.
- An identical retry before objective checkpointing receives the retained comment ID as `already_complete` without a second connector mutation. The resulting receipt carries both the exact Jira resource reference and scheduler operation key, and checkpoint persistence binds the retained success to the objective.
- If the connector response is missing, malformed, lost, or interrupted, the reservation becomes or remains `uncertain`; subsequent identical calls are hard-fenced and require operator reconciliation rather than risking a duplicate.
- UI-level final-result comments now call a backend-owned idempotent IPC. The execution database reserves an intent keyed by the durable execution run, issue key, and content fingerprint before Jira REST dispatch, persists the returned comment ID on confirmation, and reuses it on identical retry.
- The frontend recognizes a distinct `jira_comment_started` durable boundary. Recovery skips the already-completed Jira status transition and re-enters the backend comment fence; confirmed attempts complete idempotently and ambiguous attempts block.
- Comment text, raw arguments, raw connector/REST responses, connector environments, and credentials are absent from both ledgers. Only canonical identity, fingerprints, state, and confirmed comment IDs persist.
- Known limitation: Jira does not receive a provider-native idempotency key in these supported calls. A crash after Jira accepts a comment but before Spacesly durably confirms its response can no longer create a duplicate automatically, but it remains an `uncertain` operator-reconciliation case rather than being automatically adopted from Jira. Connector-side pagination must still expose the exact comment to the terminal verifier.

Implemented Confluence exact page-existence vertical slice:

```markdown
## Evidence Verifier: confluence-page

- Provider: confluence
- Connector: corporate-confluence
- Read operation: get_page
- Required states: page_exists
```

```json
{
  "document": {
    "provider": "confluence",
    "page_id": "1997894022"
  }
}
```

- The immutable contract owns one canonical numeric page ID; page authority is never extracted from a URL, title, or free-form task prose during verification.
- Rules bind one selected Confluence connector and one live-discovered read-only tool with exactly one supported `page_id`, `pageId`, or `id` argument.
- After worker execution, Spacesly independently reads the exact page and accepts completion only when structured connector output returns the same page ID and a bounded non-empty title.
- Mismatched identities, prose-only output, nested unrelated references, multiple decoded objects, missing titles, connector errors, missing tools, and invalid contract IDs block safely.
- Durable evidence contains only connector ID, page ID, `page_exists`, and satisfied/conflict/unavailable state. Page title, body, raw connector output, diagnostics, environment, credentials, and tokens are discarded.
- Known limitation: this slice proves that the exact page exists, not that its body matches a desired revision or contains task-specific deployment environments. Content fingerprint and structured requirement verification remain future provider adapters.

Phase 11 regression evidence:

- Focused Rules parser, label binding, missing-baseline, and unsupported-provider tests: 2 passed.
- Focused clean-worktree/new-commit state-transition and local-upstream containment tests: 2 passed.
- Focused Deployment predicate, namespace/identity fencing, Rules parsing, and workload/namespace binding tests: 4 passed.
- Focused polling success, timeout, cancellation, unavailable-read, request-budget, invalid-policy, and compiler compatibility tests: 7 passed.
- Focused Bamboo identity capture, receipt persistence/replay, Rules/binding, strict response parsing, bounded polling, cancellation, MCP read-only boundary/deadline, and redaction tests: 16 passed.
- Full Rust suite: 510 passed, 3 ignored, 0 failed in serial mode.
- Focused Jira Rules compilation/binding, strict identity/status parsing, mismatch/conflict handling, schema compatibility, and diagnostic redaction tests: 7 passed.
- Focused Jira comment capture, ambiguity, content drift, ADF normalization, checkpoint replay, continuation resolution, and redaction tests: 6 passed.
- Focused Jira identity, mutation-ledger replay/uncertainty, proxy response enrichment, final-writeback durability, recovery, and redaction tests: 12 passed.
- Focused Confluence Rules binding, exact structured-page parsing, mismatch/ambiguity rejection, and body-redaction tests: 3 passed.
- `cargo check` and formatting: passed.
- Frontend unit tests: 7 passed, 0 failed; `svelte-check`: 0 errors and 0 warnings.
- Clippy: 0 errors; the same 3 pre-existing warnings remain.

## Phase 10 Completed

Implemented repository-resolution increment:

- Rule facts schema/compiler v2 records repository source and line provenance; retained v1 facts remain valid.
- Repository selection combines the immutable execution contract with compiled repository Rules.
- A declared checkout is canonicalized and must be an exact Git root contained by the trusted workspace.
- When no checkout is declared, discovery is bounded to four directory levels and 4,096 directories, does not follow symlinks, and accepts only a unique repository directory matching the compiled repository ID.
- The secret-free resolution record includes repository identity, sanitized remote URL, canonical local path, Helm backend/frontend paths, provenance, status, and reason.
- The assignment-local Git authority receives the resolved repository as its default; an explicit Git `workdir` must resolve to that same root and cannot redirect the task to another contained repository.
- Ambiguous repository Rules or checkout matches, conflicting contract/Rules paths, missing/non-Git paths, and workspace escapes block before OpenCode starts.

Implemented deployment-target increment:

- Deployment table rows now retain their Rules source and line provenance.
- Exact case-insensitive ticket-label matching selects one environment, Git branch, and OpenShift namespace.
- Conflicting matching labels produce an `ambiguous` resolution, while unsafe Git branch or Kubernetes namespace values produce `invalid`; both block before OpenCode starts.
- The selected branch is attached to task-tool authority: checkout cannot select another branch, and stage/pull/commit/push/merge/rebase operations require the repository to be on the bound branch.
- The trusted embedded OCP connector receives the selected namespace through its assignment-specific environment and uses it as the kubeconfig/API default.
- OCP mutation arguments, manifests, and patches that explicitly name another namespace fail with `task_namespace_conflict` before approval consumption or cluster access.
- The binding and its provenance are retained in Task Examination and emitted as a secret-free `deployment_target_preflight` event.

Implemented connector-configuration increment:

```markdown
## Connector: corporate-confluence

- Type: confluence
- Base URL: https://confluence.bri.co.id
- Required operations:
  - search
  - get_page
```

- Rules support generic `## Connector: <configured-id>` blocks with `Type`, `Base URL`, and `Required operations` fields plus source-line provenance.
- Connector Rules are opt-in for retained configurations; when at least one exists, every connector requested by the task must have exactly one matching Rule.
- Base URLs must be absolute HTTP(S), contain no credentials/query/fragment, and match a valid URL-valued setting from the secret-backed MCP connector environment after safe normalization.
- Required operations are checked against the live connector inventory by exact tool name, type-qualified name, connector-ID-qualified name, then unique suffix fallback.
- Missing or duplicate Rules, malformed/quoted/mismatched configuration URLs, unavailable inventories, absent operations, and ambiguous fallback matches block before OpenCode starts.
- Task Examination and runtime events retain only the sanitized authoritative URL, required operation names, verified tool names, provenance, status, and reason—never environment values or credentials.

Implemented structured verification-policy increment:

```markdown
## Verification: confluence-source-read

- Connector: corporate-confluence
- Applies to labels:
  - NQLA_PRESTAGE
- Required successful operations:
  - search
  - get_page
```

- Verification Rules are provider-neutral across MCP connectors and may apply to every task using a connector or only tasks carrying an exact ticket label.
- Preflight requires a valid referenced Connector Rule and resolves each required operation against the connector's live tool inventory using the same deterministic matching precedence as connector configuration.
- The immutable Task Examination records the applicable policy, matched labels, required operation names, verified live tools, source provenance, status, and reason without connector arguments, responses, or secrets.
- Successful receipts are retained across checkpoints and automatic retries. Durable objective-checkpoint receipts also satisfy resumed executions.
- A worker `completed` result is changed to an explicit blocked outcome when any bound successful-operation receipt is absent; model-authored summaries are not accepted as proof.
- Current limitation: a receipt proves that the bound tool succeeded, but does not yet prove it read the intended resource or that returned external state satisfies a semantic predicate. Phase 11 adds connector-aware state verifiers.

Implemented cross-rule contradiction increment:

- Conflicting deployment table rows are preserved instead of being silently collapsed by label; exact duplicate rows remain deduplicated.
- Before worker execution, task-scoped analysis detects duplicate authoritative repository IDs, selected deployment labels, requested connector IDs, and applicable verification-policy IDs.
- Contradiction records contain only domain, logical key, Rules source-line references, and a corrective reason. URLs, connector environment values, arguments, responses, and secrets are excluded.
- Contradictions are retained in Task Examination, emitted as structured runtime events, and block before OpenCode can choose between competing facts.
- Unrelated connector, environment, verification, and repository definitions do not block a task that does not select them.

Implemented explicit deployment-selector increment:

```json
{
  "deployment": {
    "target": "prerelease"
  }
}
```

- The immutable Execution Contract accepts optional `deployment.target` for local or externally-created tasks without a mapped Jira label.
- Deployment tables are recognized by their structured headers rather than a hard-coded Jira-label prefix, so user-defined label schemes remain valid.
- The selector uses exact case-insensitive target-name matching against the user-defined deployment Rules table; task prose is never interpreted as environment authority.
- Label-only, explicit-only, and agreeing combined selectors resolve to one Rules row with selector provenance retained in Task Examination.
- The resolved row enters the existing authority path, binding the Git branch and trusted OCP namespace exactly as a Jira-label selection does.
- Malformed, unknown, ambiguous, and label-conflicting selectors block before OpenCode. Duplicate explicit target names are also surfaced by task-scoped contradiction diagnostics.
- The structured target is included in task resource examination and governance matching context.

Regression evidence:

- Focused repository discovery, redaction, ambiguity, conflict, and containment tests: 3 passed.
- Focused Git default-workdir test: passed.
- Focused Rules v2 compilation/provenance and v1 compatibility tests: 2 passed.
- Focused deployment target selection and ambiguity tests: 2 passed.
- Focused Git branch and OCP namespace enforcement tests: 2 passed.
- Focused connector Rule compilation/provenance test: passed.
- Focused connector URL, live-operation, missing, mismatch, and ambiguity tests: 2 passed.
- Focused verification Rule parsing, label binding, missing-receipt, and checkpoint/resume receipt tests: 4 passed.
- Focused conflicting-row preservation and task-scoped contradiction tests: 2 passed.
- Focused explicit target, combined-selector, conflict, invalid, unresolved, ambiguity, and authority-binding tests: 5 passed.
- Full Rust suite: 445 passed, 3 ignored, 0 failed in the final serial run. An earlier loaded run produced unrelated scheduler/chat timeouts and one later parallel process abort; every named timeout passed individually and the serial suite completed cleanly.
- Frontend unit tests: 7 passed, 0 failed.
- `svelte-check`: 0 errors and 0 warnings.

Next roadmap scope:

1. Phase 11 connector-aware semantic state verification beyond successful-operation receipts.

## Phase 9 Completed

Implementation scope:

1. Define versioned, secret-free operation identity and mutation evidence records.
2. Normalize the OpenShift/Kubernetes Deployment scale resource and desired replica state.
3. Read the Deployment before mutation, skip an already-satisfied scale, and use `resourceVersion` when applying detected drift.
4. Preserve one-use approval, RBAC, connector, and retry boundaries.
5. Persist redacted lookup and execution evidence in the existing private OCP audit log.
6. Test first execution, equivalent retry, partial-completion reconciliation, conflict, unauthorized mutation, and redaction.
7. Extend the same trusted identity and reconciliation contract to tokenized Deployment restarts without broadening generic Kubernetes mutation support.

This increment intentionally does not claim resource-level idempotency for Kubernetes mutations other than Deployment scale/restart, Git, Jira, Confluence, Bamboo, or generic MCP connectors.

Implemented behavior:

- `ResourceOperationIdentity` canonicalizes a connector family, operation, resource, environment fingerprint, desired-state fingerprint, and stable key.
- `ResourceMutationEvidence` captures lookup status, observed fingerprint/version, execution status, resulting fingerprint/version, and retry/resume status.
- Raw cluster URLs and desired-state values are hashed and are not serialized into identity or audit evidence.
- `ocp_scale_deployment` normalizes omitted and explicit default namespaces to the same identity.
- `ocp_restart_deployment` requires an explicit lowercase UUIDv4 semantic token, fingerprints it before persistence, and reuses the same token to reconcile an identical retry without a second patch.
- Deployment scaling performs GET before PATCH and includes the observed `resourceVersion` in the merge patch.
- A matching replica count returns `already_satisfied` without a PATCH, including after a prior attempt applied the change but lost its response.
- Missing or incompatible Deployment state returns an explicit conflict without mutation.
- Kubernetes 409 and RBAC failures retain redacted lookup/execution evidence and are not retried as mutations.
- Missing approval returns a secret-free operation identity and reaches no cluster mutation path.
- Deployment scale/restart identity and outcome evidence are persisted in the private OCP audit log independently of model-generated objective evidence.
- The scheduler mutation ledger persists `reserved`, `succeeded`, `failed`, `uncertain`, and `superseded` records separately from objective evidence.
- Active `reserved`, `succeeded`, and `uncertain` operation keys are globally unique; failed and explicitly superseded records permit a new reservation.
- Reservation validates the exact live assignment, lease, fencing token, connector capability grant, and supported operation in one immediate transaction.
- Resolution requires the exact reservation authority and identity. Assignment completion, cancellation, lease recovery, or owner shutdown changes unresolved reservations to `uncertain` rather than making them replayable.
- Supersede requires exact session ownership, operation key, revision, bounded reason, and a retained succeeded/uncertain state; the transition is recorded in the scheduler event journal.
- The MCP proxy recognizes only the trusted embedded OCP connector command for identity derivation; generic or model-provided identities cannot reserve the ledger.
- Supported scale and restart calls reserve before upstream dispatch and require string or numeric JSON-RPC IDs with one pending mutation per ID.
- Connector responses are correlated and resolved durably before they are forwarded to OpenCode.
- Valid executed/skipped evidence succeeds; matching approval and definitive precondition/conflict rejection release the reservation; malformed, mismatched, transport, write, and EOF outcomes become uncertain.
- A confirmed previous success starts one new reconciliation reservation instead of returning stale success, preserving GET-before-PATCH behavior while concurrent and uncertain calls remain fenced.
- OpenCode ledger-blocked and uncertain content results are projected as failed tool events rather than successful mutations.
- Session-scoped Tauri IPC exposes the ordered secret-free ledger projection and routes supersede through the scheduler command thread.
- The Agent technical console displays supported operation/resource/outcome metadata and offers release only for succeeded or uncertain records.
- Fence release requires a reason plus the exact session, operation key, and revision. It updates the returned row and explicitly does not retry, approve, undo, or authorize a task.
- OpenCode accepts a resource operation key only from valid successful Deployment scale/restart evidence; malformed successful output becomes a failed tool event.
- Objective checkpoints atomically bind trusted operation-key receipts to a matching succeeded ledger row with the same session, attempt, fence, and tool. Exact replay is idempotent; cross-session, non-succeeded, or differently bound receipts roll back the checkpoint.
- Completion events must match their started tool name and argument digest. Objective-bound successes remain hard replay fences, and exact checkpoint replay verifies stored content without resolving the operation key again.
- Ledger projections and the Agent technical console expose the secret-free objective and tool-call binding.
- The proxy core accepts injectable client I/O for subprocess testing while production still uses process stdin/stdout and environment-derived authority.
- Malformed connector output, connector EOF, live connectors after stdout EOF, malformed client request termination, and blocked connector-stdin writes all retain pending mutations as uncertain and terminate/reap the connector process tree.
- Shutdown fences request reservation publication, commits uncertainty before bounded best-effort client notification, and propagates every durable ledger resolution failure instead of claiming uncertainty prematurely.

Regression evidence:

- Focused resource identity tests: 3 passed.
- Focused Deployment scale tests: 6 passed.
- Focused audit redaction/persistence test: passed.
- Structured mutation approval regression: passed.
- `cargo check`: passed.
- Focused scheduler mutation ledger tests: 6 passed.
- Focused proxy identity/response tests: 3 passed, with existing proxy authority and protocol tests retained.
- Focused operation-key parsing, receipt identity, and objective-checkpoint tests: 5 passed.
- Focused proxy subprocess lifecycle tests: 5 passed.
- Full Rust suite: 425 passed, 3 ignored, 0 failed.
- Frontend unit tests: 7 passed, 0 failed.
- `svelte-check`: 0 errors and 0 warnings.
- Scoped frontend lint reaches only the pre-existing mutable `Map` finding in `AgentConsolePanel.svelte`; the resource mutation controls introduce no lint finding.
- Full clippy reaches only three pre-existing findings in `governance.rs` and `task_examination.rs`; Phase 9 introduces no clippy finding.

## Completed Increments

| Roadmap phase | Commit               | Delivered capability                                                                                        |
| ------------- | -------------------- | ----------------------------------------------------------------------------------------------------------- |
| Phase 1       | `ce4af38`            | Deterministic task examination and live MCP discovery                                                       |
| Phase 2       | `e847dad`, `de80f93` | Durable capability catalogs and live tool-plan verification                                                 |
| Phase 3       | `3448390`            | Bounded model-assisted semantic planning                                                                    |
| Phase 4       | `e7315c9`            | Objective-level completion and evidence enforcement                                                         |
| Phase 5       | `19eb621`, `577d107` | Bounded runtime recovery and safe read-capability repair                                                    |
| Phase 6       | `df786ea`            | Durable objective checkpoints across continuation                                                           |
| Phase 7       | `89d02b3`            | Tool-receipt binding and identical mutation replay fencing                                                  |
| Phase 8       | `310a11e`            | Refreshable Git discovery, shell PATH propagation, nested repository support, and Git preflight diagnostics |
| Phase 9       | `17b50b3`–`bcce9d0`  | Resource identities/evidence, scheduler mutation ledger, fenced OCP proxy, and operator controls            |

## Phase 8 Evidence

Implemented behavior:

- Git discovery caches only an executable path that still exists and remains executable.
- A failed lookup is retried, so installing Git after Spacesly starts does not permanently retain “not found.”
- Search covers process PATH, Nix user/system profiles, standard Unix locations, Homebrew, and MacPorts.
- The resolved Git directory is prepended to the sanitized shell/OpenCode PATH.
- The workspace Git MCP tool accepts a repository `workdir` contained inside its assigned workspace.
- Canonicalization and containment checks reject parent traversal and symlink escape.
- Agent preflight emits `workspace_git_preflight` with actionable repository-scope guidance.

Regression evidence recorded so far:

- `cargo check`: passed.
- `cargo test git_tool_operates_on_nested_repository_within_assigned_workspace`: passed.
- `cargo test`: 389 passed, 3 ignored, 0 failed.
- Frontend unit tests: 7 passed, 0 failed.
- `svelte-check`: 0 errors and 0 warnings.

## Known Gaps

1. Replay fencing remains exact tool name plus argument digest except for state-reconciling Deployment scale/restart operations.
2. Environment binding currently requires an exact mapped ticket label; tasks without one need an explicit structured selector.
3. Some connector configuration errors are discovered only when the connector is called.
4. Objective evidence is structurally required, but not every connector has an independent state verifier.
5. Dynamic tasks lack a release-grade evaluation corpus and safety scorecard.
6. Resource mutation objective/receipt binding is limited to the supported Deployment scale/restart slices; broader connector coverage remains incomplete.
7. Bamboo trigger-result identities become authoritative continuation input only after an objective checkpoint; interruption before checkpointing still lacks provider-level uncertain-mutation reconciliation.

## Phase 9 Follow-On Coverage

Scheduler ledger foundation delivered:

- Added the scheduler-owned resource mutation ledger and its fenced lifecycle.
- Unresolved reservations become uncertain when an assignment terminates or is recovered.
- Exact-session, exact-key, revision-checked supersede is audited.
- Trusted OCP scale/restart proxy reservation, response correlation, and conservative EOF/protocol uncertainty are now connected to the ledger.
- Trusted successful scale/restart receipts are atomically bound to immutable objective checkpoints and surfaced in secret-free ledger projections.
- Subprocess malformed/EOF/request-termination/backpressure behavior is covered with durable uncertainty and process-tree cleanup.

Planned scope:

1. Add connector-specific adapters incrementally for operations with high-confidence lookup and resource identity semantics.

Required safety properties:

- Identity derivation cannot expand task authority.
- Failed and read-only calls do not create mutation fences.
- An uncertain mutation remains blocked for review.
- Superseding a fence requires exact task/session ownership and explicit operator intent.

## Verification Checklist

- [x] Focused resource identity, scale reconciliation, conflict, authorization, and redaction tests
- [x] Rust compile check
- [x] Full Rust test suite
- [x] Frontend unit tests
- [x] Svelte type and diagnostic check
- [x] Phase 9 code has no clippy finding
- [x] Formatting and diff checks
- [x] Clean phase commit with unrelated worktree changes kept separate

## Maintenance Rule

Do not mark a phase completed based only on implementation. A completed phase requires:

1. Durable behavior implemented behind existing authority boundaries.
2. Positive, failure, and escape/replay regression coverage as applicable.
3. Full relevant backend and frontend verification.
4. A separate commit with the progress ledger updated.
