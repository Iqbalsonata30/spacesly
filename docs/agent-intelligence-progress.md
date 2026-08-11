# Spacesly Agent Intelligence Progress

Last updated: 2026-08-11

Branch: `feat/agent-task-v2-foundation`

This file is the operational ledger for the roadmap in [agent-intelligence-roadmap.md](./agent-intelligence-roadmap.md). Update it when a phase changes status, a relevant commit lands, verification changes, or a new known gap is discovered.

## Current Position

- Completed through Phase 8: portable workspace and Git resolution.
- Next implementation phase: Phase 9, resource-level idempotency.

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

1. Replay fencing is exact tool name plus argument digest, not yet a connector-aware semantic idempotency key.
2. Rules can identify a local repository, but repository selection is not yet compiled into an authoritative task-tool default.
3. Some connector configuration errors are discovered only when the connector is called.
4. Objective evidence is structurally required, but not every connector has an independent state verifier.
5. Dynamic tasks lack a release-grade evaluation corpus and safety scorecard.
6. The operator UI does not yet expose every decision’s structured provenance or idempotency fence.

## Next Phase — Resource-Level Idempotency

Planned scope:

1. Define a versioned `OperationIdentity` containing connector, operation, resource, environment, and canonical argument digest.
2. Let connector adapters derive identities without exposing secrets.
3. Persist mutation identities transactionally with successful tool receipts.
4. Fence equivalent retained mutations before dispatch.
5. Provide an explicit, audited operator supersede path instead of silent bypass.
6. Add Bamboo, Jira, Kubernetes/OpenShift, Git, and generic fallback fixtures.

Required safety properties:

- Identity derivation cannot expand task authority.
- Failed and read-only calls do not create mutation fences.
- An uncertain mutation remains blocked for review.
- Superseding a fence requires exact task/session ownership and explicit operator intent.

## Verification Checklist

- [x] Focused nested Git repository test
- [x] Rust compile check
- [x] Full Rust test suite
- [x] Frontend unit tests
- [x] Svelte type and diagnostic check
- [x] Changed-file lint
- [x] Formatting and diff checks
- [x] Clean phase commit with unrelated worktree changes kept separate

## Maintenance Rule

Do not mark a phase completed based only on implementation. A completed phase requires:

1. Durable behavior implemented behind existing authority boundaries.
2. Positive, failure, and escape/replay regression coverage as applicable.
3. Full relevant backend and frontend verification.
4. A separate commit with the progress ledger updated.
