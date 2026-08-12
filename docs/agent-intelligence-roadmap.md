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

Status: planned.

Add connector-aware verification policies: build state from Bamboo, deployment health from Kubernetes, commit/upstream state from Git, and exact issue/comment state from Jira.

Benefits:

- Evidence becomes independently checkable rather than descriptive text.
- Terminal success reflects current external state.

Trade-offs:

- Verification may add connector calls and latency.
- External eventual consistency requires bounded polling policies.

Exit criteria:

- High-risk objective types have an authoritative verifier.
- Verification failures retain the mutation receipt and block safely.

### Phase 12 — Evaluation and Regression Harness

Status: planned.

Create replayable task fixtures covering dynamic connectors, missing tools, malformed rules, workspace escapes, approval pauses, partial mutations, continuation, and model non-compliance.

Benefits:

- “Smarter” becomes measurable.
- Model or prompt upgrades can be compared without production experimentation.

Trade-offs:

- Realistic connector simulators require maintenance.

Exit criteria:

- A release has published pass rates for planning, safe execution, recovery, and evidence quality.
- Safety regressions block release.

### Phase 13 — Operator Explainability and Control

Status: planned.

Expose why a connector, tool, repository, environment, approval, recovery action, or replay fence was selected. Provide safe operator actions such as continue, approve, retry fresh, or supersede an idempotency key.

Benefits:

- Operators can trust and correct Spacesly without reading raw model logs.
- Support incidents become diagnosable from durable projections.

Trade-offs:

- More backend states require careful UI wording.

Exit criteria:

- Every block has one authoritative cause and one bounded next action.

### Phase 14 — Long-Running and Multi-Agent Readiness

Status: planned.

Harden lease recovery, connector session resumption, task decomposition, and authority isolation before allowing specialized agents to collaborate.

Benefits:

- Long deployments and process restarts retain progress safely.
- Specialized planning, execution, and verification agents can operate without sharing mutation authority.

Trade-offs:

- Coordination adds scheduling and observability complexity.
- Multi-agent execution should not be enabled until single-agent evidence and idempotency invariants are proven.

Exit criteria:

- Subtasks have independent contracts, grants, fences, budgets, and evidence.
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
