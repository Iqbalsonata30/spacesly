# Spacesly Agent Intelligence Progress

Last updated: 2026-08-11

Branch: `feat/agent-task-v2-foundation`

This file is the operational ledger for the roadmap in [agent-intelligence-roadmap.md](./agent-intelligence-roadmap.md). Update it when a phase changes status, a relevant commit lands, verification changes, or a new known gap is discovered.

## Current Position

- Completed through Phase 9: resource-level idempotency foundation.
- Phase 10, structured rule compilation and preflight resolution, is the next roadmap milestone.

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
2. Rules can identify a local repository, but repository selection is not yet compiled into an authoritative task-tool default.
3. Some connector configuration errors are discovered only when the connector is called.
4. Objective evidence is structurally required, but not every connector has an independent state verifier.
5. Dynamic tasks lack a release-grade evaluation corpus and safety scorecard.
6. Resource mutation objective/receipt binding is limited to the supported Deployment scale/restart slices; broader connector coverage remains incomplete.

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
