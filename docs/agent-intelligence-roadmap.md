# Spacesly Agent Intelligence Roadmap

## Goal

Make Spacesly reliably execute changing DevOps tasks across Jira, Confluence, Bamboo, Git, local workspaces, and Kubernetes/OpenShift without depending on prompt luck.

“Smart enough” means that the model handles semantic interpretation and bounded execution choices, while Spacesly itself owns authority, discovery, state, safety, evidence, recovery, and replay prevention.

## Target Architecture

```text
Incoming task
    |
    v
Task intake and immutable execution contract
    |
    v
Deterministic examination and live capability discovery
    |
    v
Semantic objective planner (AI, bounded output)
    |
    v
Authority compiler and preflight checks
    |
    v
AI executor through fenced workspace/MCP tools
    |
    +--> durable events and objective checkpoints
    +--> tool receipts and mutation replay fences
    +--> bounded recovery and capability repair
    |
    v
Evidence validation, terminal projection, and operator activity
```

The AI model remains part of the executor. It should not own credentials, permissions, durable truth, retry policy, or the definition of success.

## Design Principles

- Discover capabilities from live tools instead of hard-coding Jira, Bamboo, or future systems.
- Convert rules into structured constraints and facts; prompt text alone is advisory.
- Keep execution contracts, runtime profiles, grants, attempts, and evidence immutable or fenced.
- Treat every external mutation as potentially non-idempotent.
- Resume from durable evidence rather than asking the model to remember earlier work.
- Fail with an actionable diagnosis before mutation when configuration is inconsistent.
- Prefer safe, bounded repair over broad automatic retries.
- Add intelligence through policies and evaluated mechanisms, not uncontrolled self-modification.

## Delivery Phases

### Phase 1 — Deterministic Task Examination

Status: completed.

Examine objectives, resources, mutations, approvals, rules, and requested capabilities before starting the worker. Discover the current MCP connector inventory.

Benefits:

- Unknown future connectors can participate without product-specific executor code.
- Missing authority becomes visible before execution.

Trade-offs:

- Examination is conservative and may block ambiguous tasks.
- Connector discovery adds bounded startup latency.

Exit criteria:

- Every Agent Task Session has a validated, secret-free examination record.
- Unavailable requested connectors block before model execution.

### Phase 2 — Durable Capability Catalog and Routing

Status: completed.

Persist connector operations and risk metadata, then map semantic task needs to verified live tools.

Benefits:

- Routing survives process restarts.
- Spacesly distinguishes connector intent from tool availability.

Trade-offs:

- Catalog revisions must be invalidated when connectors change.

Exit criteria:

- Planned tools are checked against the exact live connector revision.
- Stale or invented tools cannot reach the executor.

### Phase 3 — Bounded Semantic Planning

Status: completed.

Use the selected model to convert dynamic requests into bounded semantic objectives, evidence requirements, resources, and mutation expectations. Keep the resulting plan inside the immutable execution contract.

Benefits:

- Dynamic tasks are understood without hard-coded workflows.
- The executor receives explicit success conditions.

Trade-offs:

- Model planning can still be incomplete; deterministic validation must remain authoritative.

Exit criteria:

- Objective IDs and evidence requirements are stable across retries and continuation.
- Invalid or oversized planner output fails closed.

### Phase 4 — Objective Evidence Enforcement

Status: completed.

Require an objective-level terminal result. A task cannot complete if an objective is omitted, blocked, duplicated, unknown, or lacks evidence.

Benefits:

- A fluent summary cannot hide unfinished work.
- Operator-visible outcomes align with the immutable plan.

Trade-offs:

- More structured output is required from the executor model.

Exit criteria:

- Completion covers every semantic objective exactly once.

### Phase 5 — Runtime Recovery and Capability Repair

Status: completed.

Classify failures, retry transient read failures once, prevent retries after uncertain mutations, and repair missing read capabilities only from the same connector’s live inventory.

Benefits:

- Temporary transport failures do not immediately fail safe tasks.
- Renamed connector read operations can be repaired without expanding authority.

Trade-offs:

- Conservative mutation handling requires operator review more often.

Exit criteria:

- No automatic mutation replay after a successful or uncertain mutation.
- Repair never changes connector scope or mutation authority.

### Phase 6 — Durable Continuation Memory

Status: completed.

Checkpoint completed objectives during execution and inject them into blocked-session continuation.

Benefits:

- A successful Bamboo build, deployment, or verified edit does not depend on conversation memory.
- Continuations focus on remaining objectives.

Trade-offs:

- Workers must emit checkpoint markers promptly after verification.

Exit criteria:

- Objective checkpoints survive blocked continuation and process-level durable storage.

### Phase 7 — Tool-Receipt Binding and Exact Replay Fencing

Status: completed.

Bind checkpoints to successful tool-call identities, risks, and argument digests. Reject an identical completed mutation before dispatch.

Benefits:

- Model text alone cannot establish mutation provenance.
- Duplicate Bamboo triggers, deployments, comments, and patches are fenced when their operation identity is identical.

Trade-offs:

- Deliberately different arguments are not considered the same operation.
- Read operations remain repeatable to allow fresh diagnostics.

Exit criteria:

- Mutation checkpoints require a successful mutation receipt.
- Identical retained mutation calls fail before tool execution.

### Phase 8 — Portable Workspace and Git Resolution

Status: completed.

Resolve Git dynamically from PATH and common package-manager profiles, propagate it into shell/OpenCode PATH, and allow a Git tool `workdir` for repositories nested inside the assigned workspace.

Benefits:

- Nix, apt, Homebrew, and other PATH-based installations work consistently.
- A trusted parent workspace can safely contain multiple repositories.

Trade-offs:

- The worker must identify the correct nested repository.
- Containment checks intentionally reject repositories reached through escaping paths or symlinks.

Exit criteria:

- Installing Git after Spacesly starts does not permanently cache failure.
- Git operations work in a nested repository but cannot escape the assigned workspace.

### Phase 9 — Resource-Level Idempotency

Status: completed.

Compile a canonical operation identity from the objective, connector, operation, target resource, environment, and normalized arguments. Persist idempotency keys separately from model-generated evidence.

Implemented foundation:

- A versioned provider-neutral operation identity records the connector family, normalized operation, resource identity, environment fingerprint, mutation fingerprint, and canonical key.
- Mutation evidence records the lookup/precondition result, execution result, and retry/resume disposition without retaining raw environment or desired-state values.
- `ocp_scale_deployment` is the first supported vertical slice. It reads the Deployment before mutation, skips an already-satisfied desired replica count, and applies drift with the observed Kubernetes `resourceVersion`.
- `ocp_restart_deployment` reuses the same model with an explicit UUIDv4 semantic restart token. The connector stores only its fingerprint on the Deployment, so an identical retry is observable and does not issue a second restart patch.
- Successful, skipped, blocked, and conflicting scale/restart outcomes retain secret-free evidence in the private OCP audit log.
- The scheduler now owns a transactional mutation ledger with globally unique active operation keys, fenced reservation and resolution, conservative uncertainty on interrupted assignments, and exact-session/key/revision audited supersede semantics.
- The fenced MCP proxy independently derives trusted scale and restart identities, reserves before dispatch, correlates responses by JSON-RPC ID, and resolves the ledger before forwarding results to the runtime.
- Confirmed prior success starts a single fresh state reconciliation so Kubernetes is read again; reserved and uncertain outcomes remain hard replay fences.
- Session-scoped IPC and the Agent technical console expose secret-free mutation history and exact session/key/revision supersede controls. Supersede records an operator reason but never retries, approves, or expands authority.
- Valid successful scale or restart evidence contributes its trusted operation key to the tool receipt. Objective checkpoint persistence atomically binds the matching succeeded session/attempt ledger row to the immutable objective and tool call, while malformed, mismatched, or differently bound receipts fail closed. Bound successes remain hard fences and exact checkpoint replay does not re-resolve a potentially reused key.
- The proxy subprocess harness exercises malformed connector output, clean and live-process stdout EOF, malformed client request termination, and connector-stdin backpressure. Shutdown durably resolves visible reservations before bounded client notification and terminates the connector process tree.

Current limitations:

- Kubernetes mutations other than Deployment scale/restart, Git, Jira, Confluence, Bamboo, and generic MCP connectors still use their existing replay behavior and do not have resource-level identities.
- Other connector operations still require their own trusted identity and response adapters.

Benefits:

- Detect semantically duplicate mutations even when superficial arguments differ.
- Provide stronger protection for Bamboo builds, deployments, Jira comments, Git pushes, and Kubernetes changes.

Trade-offs:

- Connector-specific canonicalization is required for high-confidence identities.
- Overly broad keys could block a legitimate second operation.

Exit criteria:

- Supported mutation connectors expose deterministic operation identities.
- Operators can inspect and explicitly supersede a retained idempotency fence.

### Phase 10 — Structured Rule Compiler and Preflight Resolver

Status: completed.

Expand rule compilation into repository mappings, environment tables, branch protections, approval boundaries, required verification, and connector configuration checks. Resolve repository and environment scope before starting the worker.

Implemented repository-resolution increment:

- Repository facts use the versioned v2 compiler and retain their Rules source plus line provenance while v1 retained facts remain readable.
- Git tasks select a unique repository from the immutable execution contract and compiled Rules before the worker starts.
- When Rules omit a local checkout, bounded contained discovery can resolve a unique Git repository whose directory matches the Bitbucket repository ID.
- The resolved exact repository root and Helm backend/frontend subpaths are persisted in Task Examination and installed as the assignment-local Git tool default.
- Missing, ambiguous, conflicting, non-repository, and workspace-escaping checkout selections block before model execution with corrective guidance.
- Deployment table rows retain Rules provenance and exact ticket-label matching binds the environment, Git branch, and OpenShift namespace before execution.
- Git mutations require the bound deployment branch. The trusted embedded OCP connector uses the bound namespace as its task-local default and rejects mutation payloads targeting a different namespace.
- Multiple conflicting deployment labels block preflight rather than allowing the model to choose an environment.
- User-defined Connector blocks bind a configured connector ID and type to a sanitized base URL plus required operations. Once Connector Rules are present, every requested connector must have a matching block.
- Preflight compares the authoritative base URL with secret-backed connector configuration and verifies required operations against the live MCP inventory using exact, type-qualified, ID-qualified, then unique-suffix matching.
- Missing/duplicate Rules, malformed or mismatched URLs, unavailable connectors, missing operations, and ambiguous tool matches block before model execution with secret-free diagnostics.
- User-defined `Verification` blocks bind required successful connector operations globally or to exact ticket labels. The bound operation names are resolved against the live inventory and retained with Rules provenance.
- A runtime `completed` claim is accepted only when every bound operation has a successful tool receipt from the current attempt, an automatic retry, or a durable objective checkpoint from an earlier assignment.
- Missing evidence fails closed as an explicit blocked result; the worker summary cannot override the receipt check.
- Task-scoped contradiction analysis preserves conflicting deployment rows and detects duplicate authoritative repository IDs, deployment labels, connector IDs, and applicable verification-policy IDs. It persists secret-free source references and blocks before worker execution.
- Tasks without a mapped ticket label may declare the immutable structured selector `deployment.target`. Spacesly resolves it only by exact target-name matching against the user Rules table, then applies the same Git branch and OCP namespace bindings as a label-derived target.
- Deployment table recognition is header-driven and does not require an organization-specific Jira-label prefix.
- When both a ticket label and `deployment.target` are present, both must resolve to the same branch and namespace. Unknown, malformed, ambiguous, or conflicting selectors block before model execution.

Current limitations:

- This increment verifies successful operation execution, not yet the semantic resource identity or returned external state; those connector-aware checks remain Phase 11 scope.
- Environment selection is intentionally limited to exact ticket labels or the structured `deployment.target`; environment names inferred from free-form task prose remain unsupported.

Benefits:

- Rules such as “Helm repository is qcash-deployment” become enforceable facts.
- Configuration errors such as a quoted Confluence base URL or wrong repository root fail early with a precise correction.

Trade-offs:

- Rule syntax needs versioning and diagnostics.
- Free-form rules will retain an advisory fallback.

Exit criteria:

- Core repository/environment/approval rules have structured provenance.
- Contradictory or unresolved authoritative facts block before mutation.

### Phase 11 — Evidence Verifiers

Status: completed.

Add connector-aware verification policies: build state from Bamboo, deployment health from Kubernetes, commit/upstream state from Git, and exact issue/comment state from Jira.

Implemented Git evidence-verifier increment:

- User Rules can bind a Git evidence verifier globally or by exact task label and require `clean_worktree`, `new_commit`, and/or `pushed_upstream`.
- Spacesly evaluates those states directly in the resolved trusted repository after the worker returns but before accepting completion.
- `new_commit` compares current `HEAD` with the immutable contract `repository.head_commit`; `pushed_upstream` requires an upstream and proves that upstream contains `HEAD`.
- Results persist as secret-free state/status evidence. Unsatisfied or unavailable required states block completion independently of the model summary.
- Unsupported providers fail closed when an applicable rule is requested; Jira semantic adapters remain planned.

Implemented Kubernetes Deployment availability increment:

- The immutable contract declares `deployment.workload`, while the existing deployment-target Rules bind its namespace.
- Spacesly independently reads that exact Deployment through the trusted embedded OCP connector configuration rather than trusting model-controlled MCP output.
- Availability requires the controller to have observed the current generation and updated, ready, and available replicas all to equal desired replicas.
- Namespace mismatch and unsafe resource identities fail before credentials are loaded or cluster I/O begins.
- Optional Rules-controlled polling uses bounded interval/timeout values, rechecks task authority between reads, caps each request by the remaining deadline, and retries progressing state without retrying unavailable connector reads.

Implemented Bamboo exact build-result increment:

- User Rules select the Bamboo connector and semantic read operation; live MCP discovery must resolve it to exactly one read-only tool with one supported result-key argument.
- The immutable contract supplies `build.provider=bamboo`; `build.result_key` is optional when the worker triggers a build during the task.
- A successful canonical or MCP-namespaced `bamboo_trigger_build` call must return one exact result identity in structured connector output. Spacesly captures that identity in a secret-free tool receipt and rejects prose-only or identity-free successful responses.
- On continuation, the verifier resolves the identity from current and retained objective-checkpoint receipts. A conflicting contract key or multiple captured keys blocks instead of guessing.
- Spacesly independently calls the connector after execution and strictly normalizes structured build identity/state without retaining raw output or connector errors.
- Successful state for a different build, prose-only claims, failed states, and unknown response shapes block completion.
- Optional Rules-controlled polling follows an in-progress build with bounded intervals, a bounded overall deadline, assignment-fence checks between reads, and per-request timeouts capped by the remaining deadline. Connector errors are not blindly retried.

Implemented Jira exact issue-status increment:

- User Rules select one Jira connector, one semantic read operation, the `expected_status` terminal predicate, and a bounded `Expected status` value.
- The immutable task supplies `ticket.provider=jira` and one canonical Jira issue key; Spacesly never infers issue authority from task prose.
- Live MCP discovery must resolve the Rules operation to exactly one read-only tool with exactly one supported issue-key argument.
- After execution, Spacesly independently reads that exact issue and strictly accepts only structured JSON containing the same key and one unambiguous status equal to the Rules value.
- A different issue key, multiple statuses, status mismatch, prose-only output, unknown response shape, mutation-classified tool, or connector failure blocks completion without retaining raw output or diagnostics.

Implemented Jira exact comment-state increment:

- User Rules select `comment_matches` with one Jira connector and exact issue-read operation; the immutable ticket key remains the parent-resource authority.
- Successful canonical or MCP-namespaced Jira add/create-comment calls must return one structured comment ID. Spacesly combines it with the issue key from trusted tool arguments and a normalized SHA-256 fingerprint of the requested comment body.
- The receipt persists only provider, resource kind, comment ID, parent issue key, and content fingerprint. The comment body, raw tool arguments, connector response, and diagnostics are not retained.
- Current and retained objective-checkpoint receipts can supply the dynamic comment identity on continuation. Missing, malformed, cross-issue, or multiple different comment receipts block instead of guessing.
- After execution, Spacesly independently rereads the exact issue and requires the exact comment ID to exist with matching normalized plain-text or Atlassian Document Format content.
- The fenced MCP proxy derives a stable pre-dispatch identity from the connector binding, canonical issue key, and normalized-content fingerprint, then reserves it in the scheduler mutation ledger before connector dispatch.
- Confirmed creation persists the connector-returned comment ID and trusted mutation evidence before the result reaches the worker. An identical retry before objective checkpointing adopts that retained success without another create call, and the later checkpoint binds the retained mutation to the objective.
- Connector EOF, malformed output, lost response, assignment recovery, or process interruption leaves the pre-dispatch reservation `uncertain`. That state is a hard replay fence: Spacesly blocks instead of risking a duplicate.
- UI-level final-result writeback now uses a separate durable backend intent keyed by execution run, issue key, and normalized-content fingerprint. Confirmed retries return the retained comment ID; an unconfirmed prior REST request blocks automatic replay.
- Neither ledger stores the comment body, connector environment, credentials, or raw Jira response. Only hashes, canonical resource identity, state, and confirmed comment ID are retained.

Current Jira comment limitation:

- Jira REST and the supported MCP comment operations do not expose a provider-native idempotency key through this integration. An `uncertain` outcome therefore prevents duplicates but still requires operator reconciliation; Spacesly does not yet search paginated Jira comments by a durable marker and automatically convert that fence to confirmed success.
- Automatic Jira status transition remains a separate idempotent state transition with its existing conservative recovery boundary.

Benefits:

- Evidence becomes independently checkable rather than descriptive text.
- Terminal success reflects current external state.

Trade-offs:

- Verification may add connector calls and latency.
- Kubernetes and Bamboo polling occupy the task worker until success, cancellation, connector failure, or the Rules deadline; asynchronous verification remains future work.

Exit criteria:

- Git, Kubernetes Deployment, Bamboo build, Jira issue-status, Jira comment-state, and Confluence page-existence slices have authoritative verifiers; broader connector coverage remains incremental.
- Verification failures retain the mutation receipt and block safely.

### Phase 12 — Evaluation and Regression Harness

Status: completed.

Create replayable task fixtures covering dynamic connectors, missing tools, malformed rules, workspace escapes, approval pauses, partial mutations, continuation, and model non-compliance.

Implemented recovery-policy corpus increment:

- A versioned, provider-neutral JSON fixture schema replays production `decide_runtime_recovery` policy rather than duplicating its logic in a test-only evaluator.
- The initial corpus covers bounded transient retries, retry exhaustion, rate limiting, mutation-followed-by-transport-failure, approval pauses, missing capabilities, authorization denial, and operator cancellation.
- A deterministic report publishes total and per-category counts plus pass rates in basis points. Planning, safe execution, recovery, and evidence quality are always present; categories without fixtures are explicitly `evaluated: false` with no pass rate.
- `spacesly --spacesly-evaluate-agent` runs the embedded corpus without starting the desktop UI, prints machine-readable JSON, and exits unsuccessfully when any fixture fails.
- Failure reports retain fixture IDs and mismatched output field names only. Fixture error text, connector diagnostics, credentials, and task content are not copied into the report.
- Model-result fixtures execute the production structured-response and objective-coverage validator. They cover valid evidence, malformed non-JSON completion claims, omitted and duplicate objectives, evidence-free completed objectives, and sensitive completion claims without operator approval.
- Planning-proposal fixtures execute the production semantic-plan parser. They cover bounded multi-objective decomposition, stable objective IDs, normalized non-authoritative hints, mutation classification, malformed output, empty plans, and missing success-evidence contracts.
- Rules-compilation fixtures execute the production domain compiler using generic repositories and environments. They cover dynamic repository identity, unresolved local checkouts, protected-branch approval policy, deployment targets, connector/verifier identities, and preserved conflicting rows.
- Deployment-target fixtures execute the production Rules-bound preflight resolver. They cover ticket labels, explicit targets, agreeing combined selectors, conflicts, unresolved targets, ambiguous labels, invalid branch/namespace Rules, and secret-free diagnostics.
- Repository fixtures create isolated Git layouts and execute the production Rules-bound repository resolver. They cover exact contained checkouts, bounded discovery, ambiguity, missing checkouts, contract/Rules conflicts, outside-workspace paths, multiple unselected Rules, cleanup, and secret-free normalized reports.
- Task-tool fixtures execute production workspace-read, shell-workdir, and Git-file path resolution in isolated layouts. They cover contained paths, parent traversal, absolute escapes, Unix symlink escapes, portable non-Unix outside-path equivalents, cleanup, and path-free reports.
- Connector-preflight fixtures execute the production Rules-bound configuration and capability resolver against sanitized MCP server configuration and live inventory snapshots. They cover matching configuration, missing Rules/configuration, URL mismatch, unavailable discovery, and missing or ambiguous operations without exposing URLs or inventory data in reports.

Future expansion beyond the completed phase:

- All four score categories now have initial deterministic coverage, but none is yet a release-grade corpus.
- Planning currently scores production proposal parsing and normalization, not the semantic quality of a live model's task decomposition.
- Connector process simulation, curated live-model task-to-plan fixtures, and broader replay coverage for approval continuation, partial external mutations, and provider evidence adapters remain incremental hardening work.
- The headless evaluator is a required CI and release prerequisite; any fixture mismatch returns a non-zero status and blocks the workflow, while every run publishes its JSON scorecard as an artifact.

Benefits:

- “Smarter” becomes measurable.
- Model or prompt upgrades can be compared without production experimentation.

Trade-offs:

- Realistic connector simulators require maintenance.

Exit criteria:

- A release has published pass rates for planning, safe execution, recovery, and evidence quality.
- Safety regressions block release.

### Phase 13 — Operator Explainability and Control

Status: completed.

Expose why a connector, tool, repository, environment, approval, recovery action, or replay fence was selected. Provide safe operator actions such as continue, approve, retry fresh, or supersede an idempotency key.

Implemented operator-guidance increment:

- A backend-owned projection derives terminal guidance only from scheduler state, durable runtime identity, and the resource-mutation ledger.
- Every blocked or failed session receives one stable cause and exactly one bounded next action: approve, continue, retry fresh, or review and supersede an uncertain mutation fence.
- Approval markers and canonical mutation-fence identity are checked before an action is offered. Invalid mutation identity and raw scheduler/provider errors are never copied into guidance.
- The Agent console displays the authoritative cause/source and action. Approval uses the structured approval path, uncertain mutations open the existing reason-and-revision-fenced supersede control, and Continue/Retry Fresh invoke their existing guarded task actions directly.
- Browser-level user flows verify that approval resumes through the structured action, Continue resumes the retained task, Retry Fresh starts a new attempt, and uncertain guidance highlights and releases only the exact backend-selected fence after an operator reason.
- Existing Task Examination, execution manifest, context inspection, MCP context, resource-mutation history, and execution trace projections explain connector, tool, repository, environment, policy, and recovery selections without relying on model-authored summaries.

Benefits:

- Operators can trust and correct Spacesly without reading raw model logs.
- Support incidents become diagnosable from durable projections.

Trade-offs:

- More backend states require careful UI wording.

Exit criteria:

- Every block has one authoritative cause and one bounded next action.

### Phase 14 — Long-Running and Multi-Agent Readiness

Status: in progress.

Harden lease recovery, connector session resumption, task decomposition, and authority isolation before allowing specialized agents to collaborate.

Completed first increment: fail closed when lease or owner recovery discovers an unresolved external mutation.

- Recovery transactionally interrupts the old assignment, revokes its effective authority through the existing fence, marks unresolved reservations uncertain, and blocks the same Task Session before a replacement Worker can claim it.
- The immutable task request, grants, durable runtime identity, mutation evidence, and event history remain retained. Mutation-free interruption can still resume with a new assignment fence; missing runtime identity still requires Retry Fresh; cancellation remains terminal.
- Backend-authoritative guidance selects the exact retained mutation fence. The rendered Activity timeline explains that recovery paused safely and that no replacement Worker resumed, while the operator must provide a reason before releasing the fence.
- Browser-level coverage drives the interrupted-deployment reconciliation journey and verifies that fence release neither retries nor silently authorizes the task.

The first increment does not enable multi-agent execution. Independent subtask contracts/grants/fences/budgets/evidence and non-delegable specialized-agent authority remain Phase 14 work.

Completed second increment: bounded connector-session recreation for strict read-only evidence calls.

- A transport-invalidated MCP process may be replaced once within the original request deadline. The exact authorized connector binding, read tool, and arguments are retained; provider errors and mutation calls remain outside this recovery path.
- Exact Confluence page verification is the first end-to-end adapter. A recovered process must still return the immutable page ID and a bounded non-empty title before completion is accepted.
- Durable events and the execution trace retain only provider class, read risk, bounded attempt count, and recovery status. Connector command/environment, credentials, raw responses, page title, and page body are excluded.
- The rendered task Activity explains the successful recovery and explicitly confirms that no mutation authority was replayed or expanded.

This increment recreates an MCP transport session; it does not recover provider-side conversational state. Stateful connectors will require an explicit provider-neutral resume token contract before they can use automatic recovery.

Completed third increment: prepared subtask authority contracts without multi-agent dispatch.

- Each semantic objective receives a deterministic contract identity, parent-bounded capability grants, an independent wall-clock/tool/mutation budget, an evidence-requirement digest, and an explicit non-delegable authority depth.
- Compilation and retained-manifest validation both reject capability expansion. A planner or future specialized agent cannot mark a prepared contract delegable, and the contract remains `execution_enabled=false`.
- Prepared contracts are retained in the Task Examination/Execution Manifest. Durable events and execution traces expose only safe counts, aggregate budgets, and preparation state.
- Retained pre-semantic-plan contracts remain on the existing single-Worker path and receive no synthetic subtask authority.
- The rendered Activity view explains that subtask authority was prepared while no additional Worker was started.

This increment prepares authority but does not execute it. Prepared contracts currently retain the parent grant set rather than a narrower per-objective allocation. Scheduler-owned subtask attempts/fences, enforced operation budgets, least-privilege capability allocation, cancellation/recovery, and parent evidence aggregation remain mandatory before concurrent execution.

Completed fourth increment: scheduler-owned dormant subtask records and independent fence identities.

- The fenced Execution Manifest transaction now creates one durable scheduler subtask and one dormant attempt for each prepared semantic objective. Manifest persistence and scheduler allocation either commit together or fail together.
- Every dormant attempt has its own subtask ID, subtask-attempt ID, attempt number, and fencing token. Exact tuple checks reject stale tokens and identities assembled from different subtasks.
- Identical manifest binding is idempotent. Reopen/restart reads restore the same allocations, while changed contracts, objective sets, budgets, usage, state, or authority fail closed.
- Dormant records retain bounded wall-clock, tool-call, and mutation-call budgets with zero usage. Both scheduler state and schema constraints keep `authority_active=false`; these identities cannot authorize tools or dispatch a Worker.
- The preparation event is emitted only after allocations are read back and their exact dormant fences are verified. The rendered Activity shows dormant state, fencing-identity count, and inactive tool authority without exposing objective text, capability names, evidence text, or credentials.

This increment establishes scheduler identity and persistence, not execution authority. The next increment must define an explicit activation transaction, enforce capability and operation budgets at every tool boundary, and prove cancellation/recovery before any subtask dispatch is enabled.

Completed fifth increment: staged scheduler activation and atomic tool-budget admission.

- A module-private scheduler dispatch permit is now required to convert one exact dormant fence into a short-lived subtask tool authority. No application or Worker path can construct the permit, so multi-agent dispatch remains disabled.
- Activation binds the current parent assignment attempt/fence, the exact dormant identity, an independent authority ID/fencing token, a lease capped at 30 seconds and by the parent lease, the objective identity, and the immutable contract capability set.
- Activation is idempotent only for the same live parent/dormant identity. Stale dormant fences, expired parents, changed allocations, revoked parent grants, and prior authority requiring recovery fail closed.
- Workspace and MCP authority descriptors can carry an optional nested subtask authority. Both forwarding boundaries require exact parent-descriptor equality and then atomically charge the durable tool budget before a request reaches the filesystem, shell, Git, or connector.
- Every admitted call consumes the general tool-call budget; non-read calls also consume the separate mutation-call budget. Admission persists at the start of the call, so uncertain transport outcomes cannot create free retries. Concurrent admissions cannot exceed either hard limit.
- Parent cancellation, lease expiry, scheduler-instance mismatch, stale authority fencing, capability expansion, or removal from the current parent grant table invalidates future admissions.
- The rendered Activity continues to report dormant state and inactive authority, and now explicitly shows `Activation gate: closed` plus the staged atomic budget behavior.

This increment deliberately does not create subtask Workers, activate authority from the application, renew/recover subtask leases, or aggregate subtask evidence. Those lifecycle paths must be completed and tested before the private dispatch permit can be exposed.

Completed sixth increment: staged subtask dispatch lifecycle and fail-closed recovery.

- Exact renewal revalidates the current parent assignment, immutable subtask contract, parent grants, and authority fence. The returned descriptor carries a new lease capped by the parent lease; the prior descriptor immediately becomes stale.
- Explicit completion, cancellation, and failure transitions persist a terminal timestamp and fixed, secret-free reason. Replaying the same exact terminal outcome is idempotent, while a different outcome or forged descriptor conflicts.
- Completion is accepted only while the child and exact parent authority remain live. Cancellation and failure can safely revoke an authority after work stops without asserting successful evidence.
- Parent cancellation and parent resolution revoke active child authority in the same immediate transaction as the parent transition. Lease expiry, owner loss, and parent recovery revoke remaining children through the scheduler recovery transaction.
- Recovery distinguishes only bounded lifecycle reasons: `lease_expired`, `parent_inactive`, `parent_cancelled`, and `parent_resolved`. It does not retain runtime errors, task text, tool arguments, connector output, or credentials.
- Restart-safe status lookup exposes lifecycle state, terminal reason, lease or completion time, and consumed tool/mutation budgets. It cannot be used as tool authority.
- The Activity view labels this as a staged dispatch lifecycle, states that lease and parent transitions revoke authority, and continues to show the activation gate as closed.

This increment completes the scheduler lifecycle API behind the module-private dispatch permit. No application path creates a specialized Worker, sends it a subtask descriptor, or heartbeats that descriptor, so multi-agent execution remains disabled. Least-privilege per-objective grants, independent evidence verification/aggregation, and an end-to-end scheduler-owned dispatch path remain required before activation can be exposed.

Completed seventh increment: deterministic per-objective external connector grants.

- Prepared contracts no longer copy every parent connector grant into every semantic objective. The authority compiler compares one objective's bounded summary/evidence/hints with the immutable connector plan's connector ID, matched domains, matched intents, and matched tool names.
- Matching is provider-neutral and deterministic across lowercase, snake/kebab/namespaced, and camelCase signals. Generic operation words are discarded, and only a non-generic signal owned by exactly one planned connector can retain that parent connector capability.
- Missing, malformed, or ambiguous connector evidence grants no subtask connector authority. A model hint can therefore remove authority but cannot add a capability absent from the parent Task Session.
- Built-in capabilities remain parent-bounded in this increment. The resulting normalized grant vector is included in the existing content-addressed subtask contract and enforced unchanged by activation, workspace/MCP forwarding, renewal, and lifecycle validation.
- Durable Activity evidence reports only the grant-policy identity, parent/aggregate counts, and number of narrowed objectives. It excludes objective text, capability names, connector names, tool names, evidence text, and credentials.
- The rendered Activity explains deterministic per-objective connector narrowing and the fail-closed ambiguity rule while continuing to state that specialized Workers and activation remain disabled.

This is connector-level least privilege, not tool-level authority within one connector. Built-in workspace/file/shell/Git narrowing, exact per-objective connector-operation allowlists, independent evidence aggregation, and scheduler-owned Worker dispatch remain required before multi-agent execution can be enabled.

Completed eighth increment: deterministic per-objective built-in authority.

- The authority compiler now starts every objective with no built-in grants. `workspace_read` requires explicit local workspace/file/source/configuration scope; file-write additionally requires a mutation-classified objective and an explicit create/edit/update/write/patch-style operation.
- Shell authority requires local scope, `mutation_expected=true`, and an explicit command/script/test/build/compile/lint operation. An external Bamboo build objective therefore does not receive local shell authority merely because it contains the word `build`.
- Git authority requires local scope plus a Git-specific branch/status/stage/commit/push/pull/merge/rebase/checkout operation. Read-only Git calls remain possible when granted, while the existing independent mutation-call budget still blocks Git mutations from read-only objectives.
- The allocator only intersects with the parent Task Session's four canonical built-ins. Unknown capability names are ignored, and a read-only objective cannot gain write or shell authority from model wording.
- Scheduler activation and tool admission enforce the resulting exact content-addressed grant vector. A staged read-file subtask can consume `workspace_read` but is rejected for `workspace_write`; a mutation-classified file objective receives only its bounded read/write pair.
- The durable preparation policy advances to `objective_capability_signals_v2`. Activity explains that built-in mutation authority requires both local scope and a mutation objective, without exposing the matched words or capability names.

This increment narrows built-in tool categories, not individual shell commands, Git operations, file paths, or connector tools. Existing workspace containment, Git operation allowlists, shell cancellation/timeouts, parent grants, mutation budgets, and approvals remain the enforcement layers inside an admitted category. Exact connector-operation allowlists, independent evidence aggregation, and scheduler-owned Worker dispatch remain disabled work.

Completed ninth increment: exact per-objective connector-operation authority.

- External connector capabilities now carry a bounded, sorted allowlist of exact tool operations selected from the immutable capability plan. A tool must match the objective's resource signal and normalized operation class; connector-level similarity alone is insufficient.
- The provider-neutral normalizer supports conventional snake case, kebab/namespaced names, and camelCase without hard-coding Jira, Confluence, Bamboo, Kubernetes, or another provider. Unknown providers therefore use the same fail-closed rules.
- The operation map is part of version-2 content-addressed subtask identity and is retained in Task Examination, the Execution Manifest, scheduler storage, and active authority descriptors. Legacy version-1 dormant contracts remain readable but cannot activate without exact operation authority.
- MCP admission supplies the requested tool name to the scheduler. Exact map validation occurs before durable budget consumption and upstream forwarding; a subtask allowed one connector read cannot call another read or mutation tool from that connector unless it is independently listed.
- Activation, renewal, resolution, and admission revalidate the immutable capability and operation maps alongside parent grants and fences. Forged expansion, stale authority, revoked parents, exhausted budgets, and contract drift continue to fail closed.
- Activity exposes only policy identity `objective_tool_operations_v3` and aggregate counts. It does not retain objective text, connector/tool names, arguments, responses, or credentials.

This increment completes exact connector-tool authority for staged subtasks, not dispatch. Built-in calls continue through their existing path/file/command/Git policy layers. Independent evidence verification and aggregation plus a scheduler-owned Worker launch/heartbeat path remain required before multi-agent execution can be enabled.

Completed tenth increment: independent subtask evidence attestations and parent aggregation.

- Independent evidence is persisted separately from Worker/model objective checkpoints. Only code holding a module-private verifier permit can attest evidence for an exact live subtask authority; the permit is not serialized or included in Worker tool authority.
- Each immutable attestation binds the prepared subtask, active authority, content-addressed contract, exact evidence-requirement digest, bounded verifier identity/method, verified or rejected verdict, observation count, and digest of the verifier's evidence.
- The ledger retains no raw evidence payload, model text, tool arguments/results, paths, commands, connector configuration, or credentials. Exact replay returns the retained record; a changed replay conflicts.
- Successful subtask resolution now requires a matching verified attestation in addition to the existing live lease, parent fence, current grants, immutable capabilities, and exact connector-operation map. Missing or rejected evidence cannot be represented as completion.
- A restart-safe aggregate counts total, pending, verified, rejected, and completed subtasks. Parent readiness requires every prepared subtask to be both verified and completed; one rejected, pending, or unfinished subtask keeps the aggregate closed.
- Activity explains this evidence and aggregation boundary at preparation time while continuing to state that dispatch, authority activation, and additional Workers are disabled.

This increment builds the independent evidence gate behind private permits. It does not launch a verifier, bind provider-specific verifier adapters to objectives, dispatch a Worker, or complete a parent automatically. Those production orchestration paths remain required before multi-agent execution can be enabled.

Completed eleventh increment: exact Git verifier-to-objective binding.

- New version-3 prepared subtask contracts carry a bounded, sorted verifier assignment list inside their content-addressed identity. Each assignment retains only verifier identity, provider class, deterministic method, and a digest of required terminal states.
- The existing Git terminal-state adapter is the first vertical slice. A ready Rules-bound Git verifier is assigned only when exactly one semantic objective contains Git or the required clean-worktree/new-commit/pushed-upstream signals and that objective independently received Git capability authority.
- Zero matches, multiple matches, unsupported providers, missing Git authority, malformed candidates, and duplicate verifier identities stay closed. They do not guess an objective or widen authority.
- Scheduler evidence admission for version-3 contracts requires the exact assigned verifier identity and verification method in addition to the existing requirement digest, live authority, parent fence, and private evidence permit. Version-2 retained contracts preserve their earlier evidence behavior.
- Activity reports only assigned and unassigned aggregate counts and explains the exact admission boundary. Objective text, required states, repository paths, observations, tool arguments/results, and credentials are not emitted.
- Dispatch remains disabled. This increment binds policy identity; it does not launch the Git verifier, activate a specialized Worker, heartbeat authority, or complete the parent automatically.

This established the first provider-neutral verifier assignment boundary. Additional adapters still require exact resource and read-authority binding before production permits can be opened.

Completed twelfth increment: exact Kubernetes Deployment verifier-to-objective binding.

- Prepared subtask contract schema version 4 extends each verifier assignment with explicit `read_only` mode and a digest of the exact parent-bounded capability it requires. Kubernetes assignments additionally bind a digest of the Rules-resolved namespace, resource kind, and Deployment name.
- The trusted Deployment-availability adapter is assigned only when one semantic objective names the exact workload and contains Kubernetes/Deployment scope, and that objective independently received the trusted connector capability through existing per-objective narrowing.
- Workload and namespace values are used only while compiling identity. Durable subtask authority and Activity retain their digests or aggregate counts, never the raw resource identity.
- Missing workload signals, multiple matching objectives, missing trusted connector identity, absent objective connector authority, malformed resource identity, and unsupported providers remain unassigned.
- Scheduler evidence admission accepts schema versions 2 through 4 for compatibility. Version-4 attestations must match an assigned verifier identity/method whose immutable authority mode is `read_only`.
- Production dispatch remains closed. This increment does not invoke Kubernetes, activate verifier authority, or aggregate a verifier result into parent completion.

This established resource-bound verifier authority for the trusted Kubernetes adapter. Connector-backed providers additionally require an exact read-operation binding.

Completed thirteenth increment: exact Bamboo build-result verifier-to-objective binding.

- Prepared subtask contract schema version 5 adds canonical read tool and argument identities to connector-backed verifier assignments. Compilation and retained validation require that exact tool to exist in the assigned objective's already-narrowed connector allowlist.
- A ready Bamboo verifier binds only when `build.result_key` is immutable, exactly one Bamboo/build objective contains that full normalized result key, and the objective retained the Rules-selected connector plus read operation.
- Bamboo assignments persist the canonical read tool and result-key argument, `read_only` mode, capability digest, terminal-state digest, and a digest of connector/build/result identity. The raw build key is not retained in subtask verifier authority or Activity.
- Full identity matching preserves numeric suffixes and token boundaries, preventing similar result keys such as `...-42` and `...-43` from cross-binding.
- Missing or ambiguous objectives, dynamic trigger-derived result identities, absent read-tool authority, malformed candidates, and mismatched build keys remain unassigned.
- Production dispatch remains closed. The existing parent Worker still resolves trusted trigger receipts and performs Bamboo terminal verification.

The next binding increments should add Jira and Confluence. Dynamic Bamboo trigger-result handoff requires a later scheduler-owned, receipt-bound contract revision rather than model inference.

Benefits:

- Long deployments and process restarts retain progress safely.
- Specialized planning, execution, and verification agents can operate without sharing mutation authority.

Trade-offs:

- Coordination adds scheduling and observability complexity.
- Multi-agent execution should not be enabled until single-agent evidence and idempotency invariants are proven.

Exit criteria:

- Subtasks have independent contracts, least-privilege grants, active fences, enforced budgets, and independently verified evidence.
- No agent can delegate authority it does not possess.

## Success Metrics

- Preflight detection rate for configuration and capability failures.
- Percentage of completed objectives with authoritative tool receipts and verifier evidence.
- Duplicate mutation prevention rate with false-positive review.
- Safe automatic recovery rate, separated by read and mutation tasks.
- Continuation completion rate without repeated side effects.
- Model non-compliance caught by deterministic validation.
- Median time from failure to actionable operator diagnosis.

## Explicit Non-Goals

- Giving the model unrestricted shell, filesystem, network, or connector access.
- Letting prompt text override backend authority.
- Automatically learning production mutation policies from unreviewed task history.
- Retrying uncertain mutations merely because a model requests it.
- Adding multiple agents before idempotency, evidence verification, and evaluation are mature.
