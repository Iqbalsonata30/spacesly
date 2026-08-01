# Structured Agent Skills

## Overview

Spacesly stores Agent Skills as structured entities and selects a bounded relevant subset before an Agent task begins. The application no longer sends the complete skill library to every execution or asks the model to classify skills.

```text
Structured Skill Catalog
        +
Immutable Execution Contract
        |
        v
Deterministic Task Classification
        |
        v
Trigger + Category + Priority Selection
        |
        v
Selected Skill Snapshot
        |
        v
Immutable Runtime Profile -> Agent Prompt
```

The renderer owns the editable catalog. Existing runtime profiles continue storing the selected instruction snapshot and its SHA-256 revision, preserving durable replay and audit compatibility.

## Data Model

`src/lib/agentSkills.ts` defines each `AgentSkill`:

- `id`: stable entity identity
- `name`: display and prompt heading
- `description`: searchable explanation
- `category`: one of Diagnostics, Deployment, Infrastructure, Git, Coding, Testing, Security, Database, Documentation, or Custom
- `custom_category`: category label when Category is Custom
- `trigger`: Automatic, Contextual, Manual only, or Disabled
- `priority`: deterministic selection ordering from 0 to 100
- `enabled`: master availability switch
- `instructions`: Markdown or plain-text procedure
- `notes`: private maintenance notes that never enter prompts
- `created_at` and `updated_at`: lifecycle metadata
- `metrics`: reserved usage count, last-used, success-rate, latency, and favorite fields
- `metadata`: extensible primitive metadata for imports, marketplace identity, versions, and provenance

Metrics and extensible metadata are present now so usage analytics, favorites, import/export, and community sources can be added without replacing the entity format.

## Storage

The catalog is persisted as `aiWorker.skills` inside the existing secret-free `spacesly.settings.v1` JSON document in WebView `localStorage`. Skills contain no credentials and do not use secret storage.

Runtime execution remains immutable:

1. Spacesly selects relevant structured entities.
2. Selected entities are serialized into a deterministic procedure document.
3. The existing runtime-profile code hashes and stores that selected snapshot.
4. In-flight and retained Task Sessions continue using their original selected Skill snapshot even if the editable catalog later changes. Other runtime settings are rebuilt from current Settings on continuation.

The selected IDs and serialized procedure snapshot are also recorded in the execution contract runtime inputs. A blocked or timed-out continuation reuses that immutable snapshot instead of reclassifying against a newer catalog.

## Legacy Migration

`normalizeSettings()` detects the former `aiWorker.agentSkills` string when `aiWorker.skills` is absent.

- Every `Skill: Name` section becomes an individual entity.
- Instructions beneath each heading are preserved.
- Text before the first heading becomes an imported legacy-guidance entity.
- A blob without headings becomes one imported legacy skill.
- An explicitly empty legacy value remains an empty catalog.
- Migrated entities use deterministic IDs and `metadata.source = "legacy"`.
- Migrated entities use Automatic activation to preserve their previous always-available behavior until users categorize them.

The normalized structured document is written back by the existing settings loader, making migration one-way and idempotent.

## Trigger Semantics

Selection applies these rules in order:

1. `enabled = false` never selects.
2. Disabled trigger never selects.
3. Automatic selects for every Agent task.
4. Contextual selects only when its category matches classified immutable task signals.
5. Manual only selects only when its exact ID is queued for the next run.
6. A skill is included once even if multiple reasons match.
7. Selected skills are ordered by descending priority, then catalog order.

The Skills library exposes **Use next run** for enabled Manual-only skills. The first new Agent run consumes the queued IDs and records them in its immutable contract snapshot.

## Task Classification

Classification uses only authoritative execution-contract fields:

- Objective and success criteria
- Task description and execution detail
- Ticket title and labels
- Active workflow steps
- Operator notes

Previous Agent output and source-file contents are intentionally excluded. Matching is lowercase whole-phrase lexical matching against a closed category term map. It is deterministic, dependency-free, and cannot grant capabilities or load MCP connectors.

Future rule-based matching can extend the category classifier and selection reason map without changing callers, storage, profiles, or prompt rendering.

## Prompt Boundary

`buildAiWorkerConfig()` starts with no skills. After Spacesly creates the immutable `ExecutionContract`, it calls:

1. `selectAgentSkills()`
2. `serializeSelectedSkills()`
3. Agent Task Session profile creation or direct Agent execution

Chat and AI Edit retain their existing behavior and do not receive Agent Skills. Rust prompt construction describes the supplied procedures as preselected and explicitly forbids runtime inference of additional skills.

Selection limits protect prompt size:

- 64 catalog entities
- 8 KiB instructions per skill
- 16 selected skills per execution
- 32 KiB selected serialized instructions

## Settings Experience

The Skills page provides:

- New Skill action
- Search across name, description, category, trigger, and effective status
- Category, trigger, and status filters
- Enabled/disabled state and metadata badges
- Edit, duplicate, enable/disable, and delete actions
- Manual-only queuing for the next Agent run
- Dedicated responsive editor dialog
- Validation for required fields, duplicate names, custom categories, priority bounds, and instruction size
- Private notes separated from runtime instructions

Skill editor Save, duplicate, enable/disable, and delete operations persist the catalog immediately. Other Settings fields retain their existing Save Settings behavior; a skill operation does not accidentally persist unrelated drafts.

## Tests

`tests/agentSkills.ts` covers:

- Sectioned and unstructured legacy migration
- Empty legacy preservation
- Settings-level migration
- Structured normalization and future metadata preservation
- Automatic, contextual, manual-only, and disabled behavior
- Deterministic category classification and priority ordering
- Exclusion of unrelated skills and private notes from prompts
- Search metadata, catalog validation, duplicate-name rejection, and entity duplication

Rust prompt tests continue validating the Agent/Chat governance boundary and immutable runtime-profile tests continue validating selected snapshot revisions.
