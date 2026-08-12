# Spacesly Agent Intelligence Progress

Last updated: 2026-08-12

Branch: `feat/agent-task-v2-foundation`

This file is the operational ledger for the roadmap in [agent-intelligence-roadmap.md](./agent-intelligence-roadmap.md). Update it when a phase changes status, a relevant commit lands, verification changes, or a new known gap is discovered.

## Current Position

- Completed through Phase 9: resource-level idempotency foundation.
- Completed through Phase 10: structured Rules, deterministic scope resolution, connector preflight, verification receipts, and contradiction detection.
- Phase 11 is in progress with independent Git terminal-state, Kubernetes Deployment-availability, and exact Bamboo build-result verifiers.
- Phase 11 now also includes a Rules-bound exact Jira issue-status verifier; Jira comment-state verification remains open.

## Phase 11 In Progress

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
- Exact Jira issue/comment state still requires a provider-specific response adapter.

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
- Rule facts compiler v6 persists typed polling plus external connector/read-operation bindings; retained v1–v5 task snapshots remain valid under their original immutable semantics.
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
- Rule facts compiler v6 persists Bamboo polling and Jira expected status; retained v1–v5 task snapshots remain valid under their original immutable semantics.
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
- Rule facts compiler v6 persists the Jira expected-status policy; retained v1–v5 snapshots remain valid under their original immutable semantics.
- Known limitation: the expected status is currently Rules-defined, so dynamic per-task transition targets require separate Rules scoped by labels. Exact Jira comment identity/content verification and Jira mutation idempotency remain future increments.

Phase 11 regression evidence:

- Focused Rules parser, label binding, missing-baseline, and unsupported-provider tests: 2 passed.
- Focused clean-worktree/new-commit state-transition and local-upstream containment tests: 2 passed.
- Focused Deployment predicate, namespace/identity fencing, Rules parsing, and workload/namespace binding tests: 4 passed.
- Focused polling success, timeout, cancellation, unavailable-read, request-budget, invalid-policy, and compiler compatibility tests: 7 passed.
- Focused Bamboo identity capture, receipt persistence/replay, Rules/binding, strict response parsing, bounded polling, cancellation, MCP read-only boundary/deadline, and redaction tests: 16 passed.
- Full Rust suite: 482 passed, 3 ignored, 0 failed in serial mode.
- Focused Jira Rules compilation/binding, strict identity/status parsing, mismatch/conflict handling, schema compatibility, and diagnostic redaction tests: 7 passed.
- `cargo check` and formatting: passed.
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
