# Spacesly Agent Intelligence Progress

Last updated: 2026-08-11

Branch: `feat/agent-task-v2-foundation`

This file is the operational ledger for the roadmap in [agent-intelligence-roadmap.md](./agent-intelligence-roadmap.md). Update it when a phase changes status, a relevant commit lands, verification changes, or a new known gap is discovered.

## Current Position

- Completed through Phase 8: portable workspace and Git resolution.
- Phase 9 is in progress with a provider-neutral identity/evidence model and an initial `ocp_scale_deployment` vertical slice.

## Phase 9 In Progress

Implementation scope:

1. Define versioned, secret-free operation identity and mutation evidence records.
2. Normalize the OpenShift/Kubernetes Deployment scale resource and desired replica state.
3. Read the Deployment before mutation, skip an already-satisfied scale, and use `resourceVersion` when applying detected drift.
4. Preserve one-use approval, RBAC, connector, and retry boundaries.
5. Persist redacted lookup and execution evidence in the existing private OCP audit log.
6. Test first execution, equivalent retry, partial-completion reconciliation, conflict, unauthorized mutation, and redaction.

This increment intentionally does not claim resource-level idempotency for other Kubernetes mutations, Git, Jira, Confluence, Bamboo, or generic MCP connectors.

Implemented behavior:

- `ResourceOperationIdentity` canonicalizes a connector family, operation, resource, environment fingerprint, desired-state fingerprint, and stable key.
- `ResourceMutationEvidence` captures lookup status, observed fingerprint/version, execution status, resulting fingerprint/version, and retry/resume status.
- Raw cluster URLs and desired-state values are hashed and are not serialized into identity or audit evidence.
- `ocp_scale_deployment` normalizes omitted and explicit default namespaces to the same identity.
- Deployment scaling performs GET before PATCH and includes the observed `resourceVersion` in the merge patch.
- A matching replica count returns `already_satisfied` without a PATCH, including after a prior attempt applied the change but lost its response.
- Missing or incompatible Deployment state returns an explicit conflict without mutation.
- Kubernetes 409 and RBAC failures retain redacted lookup/execution evidence and are not retried as mutations.
- Missing approval returns a secret-free operation identity and reaches no cluster mutation path.
- Scale identity and outcome evidence are persisted in the private OCP audit log independently of model-generated objective evidence.
- The scheduler mutation ledger persists `reserved`, `succeeded`, `failed`, `uncertain`, and `superseded` records separately from objective evidence.
- Active `reserved`, `succeeded`, and `uncertain` operation keys are globally unique; failed and explicitly superseded records permit a new reservation.
- Reservation validates the exact live assignment, lease, fencing token, connector capability grant, and supported operation in one immediate transaction.
- Resolution requires the exact reservation authority and identity. Assignment completion, cancellation, lease recovery, or owner shutdown changes unresolved reservations to `uncertain` rather than making them replayable.
- Supersede requires exact session ownership, operation key, revision, bounded reason, and a retained succeeded/uncertain state; the transition is recorded in the scheduler event journal.
- The MCP proxy recognizes only the trusted embedded OCP connector command for identity derivation; generic or model-provided identities cannot reserve the ledger.
- Scale calls reserve before upstream dispatch and require string or numeric JSON-RPC IDs with one pending mutation per ID.
- Connector responses are correlated and resolved durably before they are forwarded to OpenCode.
- Valid executed/skipped evidence succeeds; matching approval and definitive precondition/conflict rejection release the reservation; malformed, mismatched, transport, write, and EOF outcomes become uncertain.
- A confirmed previous success starts one new reconciliation reservation instead of returning stale success, preserving GET-before-PATCH behavior while concurrent and uncertain calls remain fenced.
- OpenCode ledger-blocked and uncertain content results are projected as failed tool events rather than successful mutations.

Regression evidence:

- Focused resource identity tests: 3 passed.
- Focused Deployment scale tests: 6 passed.
- Focused audit redaction/persistence test: passed.
- Structured mutation approval regression: passed.
- `cargo check`: passed.
- Focused scheduler mutation ledger tests: 6 passed.
- Focused proxy identity/response tests: 3 passed, with existing proxy authority and protocol tests retained.
- Full Rust suite: 409 passed, 3 ignored, 0 failed.
- Frontend unit tests: 7 passed, 0 failed.
- `svelte-check`: 0 errors and 0 warnings.
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

1. Replay fencing remains exact tool name plus argument digest except for the state-reconciling `ocp_scale_deployment` slice.
2. Rules can identify a local repository, but repository selection is not yet compiled into an authoritative task-tool default.
3. Some connector configuration errors are discovered only when the connector is called.
4. Objective evidence is structurally required, but not every connector has an independent state verifier.
5. Dynamic tasks lack a release-grade evaluation corpus and safety scorecard.
6. The operator UI does not yet expose resource mutation evidence or an explicit idempotency supersede action.

## Phase 9 Remaining Work

Scheduler ledger foundation delivered:

- Added the scheduler-owned resource mutation ledger and its fenced lifecycle.
- Unresolved reservations become uncertain when an assignment terminates or is recovered.
- Exact-session, exact-key, revision-checked supersede is audited.
- Trusted OCP scale proxy reservation, response correlation, and conservative EOF/protocol uncertainty are now connected to the ledger.

Planned scope:

1. Bind durable scheduler identities to immutable objectives and transactionally to successful tool receipts.
2. Expose inspectable identity, outcome evidence, and exact supersede controls through IPC and the operator UI.
3. Add a subprocess-level proxy harness for request-thread termination and malformed/EOF transport cases.
4. Add connector-specific adapters incrementally for operations with high-confidence lookup and resource identity semantics.

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
