import type { ExecutionContract } from "$lib/ipc";

export const skillCategories = [
  "diagnostics",
  "deployment",
  "infrastructure",
  "git",
  "coding",
  "testing",
  "security",
  "database",
  "documentation",
  "custom",
] as const;

export const skillTriggers = ["automatic", "contextual", "manual", "disabled"] as const;

export type SkillCategory = (typeof skillCategories)[number];
export type SkillTrigger = (typeof skillTriggers)[number];

export type AgentSkillMetrics = {
  usage_count: number;
  last_used_at: string | null;
  success_rate: number | null;
  average_execution_latency_ms: number | null;
  favorite: boolean;
};

export type AgentSkill = {
  id: string;
  name: string;
  description: string;
  category: SkillCategory;
  custom_category: string;
  trigger: SkillTrigger;
  priority: number;
  enabled: boolean;
  instructions: string;
  notes: string;
  created_at: string;
  updated_at: string;
  metrics: AgentSkillMetrics;
  metadata: Record<string, string | number | boolean | null>;
};

export type SkillSelection = {
  skills: AgentSkill[];
  categories: SkillCategory[];
  reasons: Record<string, string[]>;
};

export type SkillSnapshotResolution = {
  contract: ExecutionContract;
  snapshot: string;
  selectedSkillIds: string[];
  reused: boolean;
};

const MAX_SKILLS = 64;
const MAX_SELECTED_SKILLS = 16;
const MAX_INSTRUCTIONS_BYTES = 8 * 1024;
const MAX_SELECTED_BYTES = 32 * 1024;

const categoryTerms: Record<Exclude<SkillCategory, "custom">, string[]> = {
  diagnostics: [
    "diagnose",
    "diagnostic",
    "troubleshoot",
    "failure",
    "failed",
    "error",
    "incident",
    "logs",
    "events",
    "crash",
    "timeout",
  ],
  deployment: ["deploy", "deployment", "release", "rollout", "bamboo", "build plan"],
  infrastructure: [
    "infrastructure",
    "kubernetes",
    "openshift",
    "ocp",
    "pod",
    "namespace",
    "cluster",
    "terraform",
    "helm",
  ],
  git: ["git", "commit", "branch", "merge", "pull request", "repository"],
  coding: ["code", "implement", "refactor", "function", "component", "module", "bug", "fix"],
  testing: ["test", "testing", "lint", "coverage", "verify", "validation"],
  security: [
    "security",
    "vulnerability",
    "credential",
    "secret",
    "permission",
    "authentication",
    "authorization",
  ],
  database: ["database", "sql", "postgres", "mysql", "sqlite", "migration", "schema"],
  documentation: ["documentation", "docs", "readme", "runbook", "guide"],
};

export function categoryLabel(category: SkillCategory, customCategory = ""): string {
  if (category === "custom") return customCategory.trim() || "Custom";
  return category[0].toUpperCase() + category.slice(1);
}

export function triggerLabel(trigger: SkillTrigger): string {
  if (trigger === "manual") return "Manual only";
  return trigger[0].toUpperCase() + trigger.slice(1);
}

export function createAgentSkill(now = new Date().toISOString()): AgentSkill {
  return {
    id: createSkillId("skill"),
    name: "New Skill",
    description: "",
    category: "diagnostics",
    custom_category: "",
    trigger: "contextual",
    priority: 50,
    enabled: true,
    instructions: "",
    notes: "",
    created_at: now,
    updated_at: now,
    metrics: emptyMetrics(),
    metadata: { source: "local", schema_version: 1 },
  };
}

export function defaultAgentSkills(now = new Date().toISOString()): AgentSkill[] {
  const definitions: Array<
    Pick<AgentSkill, "name" | "description" | "category" | "trigger" | "priority" | "instructions">
  > = [
    {
      name: "Production Task Execution",
      description: "Execute repository work using concrete evidence and verification.",
      category: "coding",
      trigger: "automatic",
      priority: 80,
      instructions:
        "Use concrete evidence. For file changes, verify with shell commands or file reads. For Jira tasks, summarize exact status and comment actions.",
    },
    {
      name: "Operational Troubleshooting",
      description: "Inspect runtime evidence before proposing operational fixes.",
      category: "diagnostics",
      trigger: "contextual",
      priority: 70,
      instructions:
        "Prefer checking logs, events, build output, and recent changes before guessing. Report blockers with the next evidence needed.",
    },
    {
      name: "Architecture Compliance",
      description: "Keep domain logic independent from UI and infrastructure concerns.",
      category: "coding",
      trigger: "contextual",
      priority: 60,
      instructions:
        "Keep domain logic independent from UI, Tauri, and provider details. Treat every model provider and external service as replaceable infrastructure.",
    },
  ];
  return definitions.map((definition, index) => ({
    ...createAgentSkill(now),
    ...definition,
    id: stableLegacyId(definition.name, definition.instructions, index),
    metadata: { source: "builtin", schema_version: 1 },
  }));
}

export function duplicateAgentSkill(skill: AgentSkill, now = new Date().toISOString()): AgentSkill {
  return {
    ...skill,
    id: createSkillId(skill.name),
    name: `${skill.name} Copy`,
    created_at: now,
    updated_at: now,
    metrics: emptyMetrics(),
    metadata: { ...skill.metadata, source: "local", duplicated_from: skill.id },
  };
}

export function normalizeAgentSkills(value: unknown, now = new Date().toISOString()): AgentSkill[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry, index) => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) return [];
    const candidate = entry as Partial<AgentSkill>;
    const rawCategory = String(candidate.category ?? "").trim();
    const category = skillCategories.includes(rawCategory as SkillCategory)
      ? (rawCategory as SkillCategory)
      : "custom";
    const trigger = skillTriggers.includes(candidate.trigger as SkillTrigger)
      ? (candidate.trigger as SkillTrigger)
      : "contextual";
    const name = String(candidate.name ?? "Untitled Skill").trim() || "Untitled Skill";
    const instructions = String(candidate.instructions ?? "");
    return [
      {
        id: String(candidate.id ?? "").trim() || stableLegacyId(name, instructions, index),
        name,
        description: String(candidate.description ?? ""),
        category,
        custom_category: String(
          candidate.custom_category ?? (category === "custom" ? rawCategory : ""),
        ),
        trigger,
        priority: boundedPriority(candidate.priority),
        enabled: trigger === "disabled" ? false : candidate.enabled !== false,
        instructions,
        notes: String(candidate.notes ?? ""),
        created_at: validTimestamp(candidate.created_at) ? candidate.created_at! : now,
        updated_at: validTimestamp(candidate.updated_at) ? candidate.updated_at! : now,
        metrics: normalizeMetrics(candidate.metrics),
        metadata: normalizeMetadata(candidate.metadata),
      },
    ];
  });
}

export function migrateLegacyAgentSkills(
  legacy: string,
  now = new Date().toISOString(),
): AgentSkill[] {
  if (!legacy.trim()) return [];
  const lines = legacy.split("\n");
  const sections: Array<{ name: string; lines: string[] }> = [];
  let current: { name: string; lines: string[] } | null = null;
  const preamble: string[] = [];

  for (const line of lines) {
    const heading = line.match(/^\s*Skill:\s*(.+?)\s*$/i);
    if (heading) {
      if (current) sections.push(current);
      current = { name: heading[1].trim(), lines: [] };
    } else if (current) {
      current.lines.push(line);
    } else {
      preamble.push(line);
    }
  }
  if (current) sections.push(current);

  if (sections.length === 0) {
    sections.push({ name: "Imported Legacy Skill", lines: [legacy] });
  } else if (preamble.some((line) => line.trim())) {
    sections.unshift({ name: "Imported Legacy Guidance", lines: preamble });
  }

  return sections.map((section, index) => {
    const legacyInstructions = section.lines.join("\n").trim();
    const instructions =
      legacyInstructions || "No instructions were configured in this legacy skill.";
    const categories = classifySkillText(`${section.name} ${instructions}`);
    const category = categories[0] ?? "custom";
    return {
      ...createAgentSkill(now),
      id: stableLegacyId(section.name, instructions, index),
      name: section.name || `Imported Skill ${index + 1}`,
      description: firstSentence(instructions),
      category,
      custom_category: category === "custom" ? "Legacy" : "",
      trigger: "automatic",
      enabled: Boolean(legacyInstructions),
      priority: Math.max(0, 50 - index),
      instructions,
      metadata: { source: "legacy", schema_version: 1 },
    };
  });
}

export function validateAgentSkill(skill: AgentSkill, catalog: AgentSkill[] = []): string | null {
  if (!skill.id.trim()) return "Skill ID is required.";
  if (!skill.name.trim()) return "Skill name is required.";
  if (skill.name.trim().length > 160) return "Skill name must be 160 characters or fewer.";
  if (!skill.description.trim()) return "Skill description is required.";
  if (skill.description.trim().length > 500) {
    return "Skill description must be 500 characters or fewer.";
  }
  if (!skill.instructions.trim()) return "Skill instructions are required.";
  if (new TextEncoder().encode(skill.instructions).length > MAX_INSTRUCTIONS_BYTES) {
    return "Skill instructions must be 8 KiB or smaller.";
  }
  if (skill.category === "custom" && !skill.custom_category.trim()) {
    return "Custom category name is required.";
  }
  if (skill.custom_category.trim().length > 120) {
    return "Custom category must be 120 characters or fewer.";
  }
  if (new TextEncoder().encode(skill.notes).length > 4 * 1024) {
    return "Skill notes must be 4 KiB or smaller.";
  }
  if (!Number.isInteger(skill.priority) || skill.priority < 0 || skill.priority > 100) {
    return "Skill priority must be a whole number from 0 to 100.";
  }
  if (skill.enabled && skill.trigger === "disabled") {
    return "A skill with the Disabled trigger cannot also be enabled.";
  }
  const duplicate = catalog.find(
    (candidate) =>
      candidate.id !== skill.id &&
      candidate.name.trim().toLowerCase() === skill.name.trim().toLowerCase(),
  );
  if (duplicate) return `Another skill is already named “${skill.name.trim()}”.`;
  return null;
}

export function validateAgentSkillCatalog(skills: AgentSkill[]): string | null {
  if (skills.length > MAX_SKILLS) return `A maximum of ${MAX_SKILLS} skills is supported.`;
  const ids = new Set<string>();
  for (const skill of skills) {
    if (ids.has(skill.id)) return `Duplicate skill ID: ${skill.id}`;
    ids.add(skill.id);
    const error = validateAgentSkill(skill, skills);
    if (error) return `${skill.name || "Untitled Skill"}: ${error}`;
  }
  return null;
}

export function selectAgentSkills(
  skills: AgentSkill[],
  contract: ExecutionContract,
  requestedSkillIds: string[] = [],
): SkillSelection {
  const catalogError = validateAgentSkillCatalog(skills);
  if (catalogError) throw new Error(`Cannot resolve Agent Skills: ${catalogError}`);
  const categories = classifyExecutionContract(contract);
  const requested = new Set(requestedSkillIds);
  const reasons: Record<string, string[]> = {};
  const selected = skills.filter((skill) => {
    if (!skill.enabled || skill.trigger === "disabled") return false;
    const skillReasons: string[] = [];
    if (skill.trigger === "automatic") skillReasons.push("automatic");
    if (skill.trigger === "contextual") {
      if (skill.category === "custom") {
        const term = normalizeText(skill.custom_category);
        if (term && includesPhrase(normalizedContractText(contract), term)) {
          skillReasons.push(`category:${term}`);
        }
      } else if (categories.includes(skill.category)) {
        skillReasons.push(`category:${skill.category}`);
      }
    }
    if (skill.trigger === "manual" && requested.has(skill.id)) skillReasons.push("manual");
    if (skill.trigger === "manual" && !requested.has(skill.id)) return false;
    if (skillReasons.length === 0) return false;
    reasons[skill.id] = skillReasons;
    return true;
  });

  selected.sort((left, right) => {
    const leftManual = reasons[left.id]?.includes("manual") ? 1 : 0;
    const rightManual = reasons[right.id]?.includes("manual") ? 1 : 0;
    return (
      rightManual - leftManual ||
      right.priority - left.priority ||
      skills.indexOf(left) - skills.indexOf(right)
    );
  });
  if (selected.length > MAX_SELECTED_SKILLS) {
    throw new Error(
      `Cannot resolve Agent Skills: ${selected.length} skills matched this task, exceeding the ${MAX_SELECTED_SKILLS}-skill execution limit. Disable or narrow lower-priority Skills.`,
    );
  }
  return { skills: selected, categories, reasons };
}

export function serializeSelectedSkills(selection: SkillSelection): string {
  return serializeSkillSelection(selection).snapshot;
}

function serializeSkillSelection(selection: SkillSelection): {
  snapshot: string;
  selectedSkillIds: string[];
} {
  const sections: string[] = [];
  const selectedSkillIds: string[] = [];
  let bytes = 0;
  for (const skill of selection.skills) {
    const section = [
      `Skill: ${skill.name}`,
      `Skill ID: ${skill.id}`,
      `Category: ${categoryLabel(skill.category, skill.custom_category)}`,
      `Description: ${skill.description.trim()}`,
      "Instructions:",
      skill.instructions.trim(),
    ].join("\n");
    const sectionBytes = new TextEncoder().encode(section).length;
    const separatorBytes = sections.length === 0 ? 0 : 2;
    if (bytes + separatorBytes + sectionBytes > MAX_SELECTED_BYTES) {
      throw new Error(
        `Cannot resolve Agent Skills: selected Skills exceed the ${MAX_SELECTED_BYTES / 1024} KiB prompt limit at “${skill.name}”. Shorten or disable Skills before starting the task.`,
      );
    }
    sections.push(section);
    selectedSkillIds.push(skill.id);
    bytes += separatorBytes + sectionBytes;
  }
  return { snapshot: sections.join("\n\n"), selectedSkillIds };
}

export function resolveAgentSkillSnapshot(
  skills: AgentSkill[],
  contract: ExecutionContract,
  requestedSkillIds: string[] = [],
): SkillSnapshotResolution {
  const retained = contract.runtime_inputs.selected_skills_snapshot;
  if (retained !== undefined) {
    return {
      contract,
      snapshot: retained,
      selectedSkillIds: contract.runtime_inputs.selected_skill_ids ?? [],
      reused: true,
    };
  }
  const selection = selectAgentSkills(skills, contract, requestedSkillIds);
  const { snapshot, selectedSkillIds } = serializeSkillSelection(selection);
  return {
    contract: {
      ...contract,
      runtime_inputs: {
        ...contract.runtime_inputs,
        selected_skill_ids: selectedSkillIds,
        selected_skills_snapshot: snapshot,
      },
    },
    snapshot,
    selectedSkillIds,
    reused: false,
  };
}

export function classifyExecutionContract(contract: ExecutionContract): SkillCategory[] {
  return classifySkillText(normalizedContractText(contract));
}

export function skillMatchesSearch(skill: AgentSkill, search: string): boolean {
  const query = normalizeText(search);
  if (!query) return true;
  const status = skill.enabled && skill.trigger !== "disabled" ? "enabled" : "disabled";
  const searchable = normalizeText(
    [
      skill.name,
      skill.description,
      categoryLabel(skill.category, skill.custom_category),
      triggerLabel(skill.trigger),
      status,
    ].join(" "),
  );
  return query.split(" ").every((term) => searchable.includes(term));
}

function normalizedContractText(contract: ExecutionContract): string {
  return normalizeText(
    [
      contract.objective.summary,
      ...contract.objective.success_criteria,
      contract.task_context.description,
      contract.task_context.execution_detail,
      contract.ticket.title,
      ...contract.ticket.labels,
      ...contract.workflow
        .filter((step) => step.status !== "completed")
        .flatMap((step) => [step.title, step.type]),
      contract.runtime_inputs.operator_notes ?? "",
    ].join(" "),
  );
}

function classifySkillText(value: string): SkillCategory[] {
  const normalized = normalizeText(value);
  return (Object.entries(categoryTerms) as Array<[Exclude<SkillCategory, "custom">, string[]]>)
    .filter(([, terms]) => terms.some((term) => includesPhrase(normalized, normalizeText(term))))
    .map(([category]) => category);
}

function normalizeText(value: string): string {
  return value
    .toLowerCase()
    .replace(/[^\p{L}\p{N}+#._/-]+/gu, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function includesPhrase(text: string, phrase: string): boolean {
  return Boolean(phrase) && ` ${text} `.includes(` ${phrase} `);
}

function createSkillId(name: string): string {
  const suffix =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
  return `skill-${slug(name)}-${suffix}`;
}

function stableLegacyId(name: string, instructions: string, index: number): string {
  let hash = 2166136261;
  for (const character of `${index}:${name}:${instructions}`) {
    hash ^= character.charCodeAt(0);
    hash = Math.imul(hash, 16777619);
  }
  return `skill-legacy-${slug(name)}-${(hash >>> 0).toString(36)}`;
}

function slug(value: string): string {
  return (
    value
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "")
      .slice(0, 40) || "untitled"
  );
}

function boundedPriority(value: unknown): number {
  const number = Number(value);
  return Number.isFinite(number) ? Math.min(100, Math.max(0, Math.round(number))) : 50;
}

function validTimestamp(value: unknown): value is string {
  return typeof value === "string" && value.trim() !== "" && Number.isFinite(Date.parse(value));
}

function emptyMetrics(): AgentSkillMetrics {
  return {
    usage_count: 0,
    last_used_at: null,
    success_rate: null,
    average_execution_latency_ms: null,
    favorite: false,
  };
}

function normalizeMetrics(value: unknown): AgentSkillMetrics {
  const metrics = value && typeof value === "object" ? (value as Partial<AgentSkillMetrics>) : {};
  return {
    usage_count: Math.max(0, Math.floor(Number(metrics.usage_count) || 0)),
    last_used_at: validTimestamp(metrics.last_used_at) ? metrics.last_used_at : null,
    success_rate:
      metrics.success_rate === null || metrics.success_rate === undefined
        ? null
        : Math.min(1, Math.max(0, Number(metrics.success_rate) || 0)),
    average_execution_latency_ms:
      metrics.average_execution_latency_ms === null ||
      metrics.average_execution_latency_ms === undefined
        ? null
        : Math.max(0, Number(metrics.average_execution_latency_ms) || 0),
    favorite: metrics.favorite === true,
  };
}

function normalizeMetadata(value: unknown): AgentSkill["metadata"] {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { source: "local", schema_version: 1 };
  }
  return {
    source: "local",
    schema_version: 1,
    ...Object.fromEntries(
      Object.entries(value).filter((entry): entry is [string, string | number | boolean | null] => {
        const candidate = entry[1];
        return (
          candidate === null ||
          typeof candidate === "string" ||
          typeof candidate === "number" ||
          typeof candidate === "boolean"
        );
      }),
    ),
  };
}

function firstSentence(value: string): string {
  const first = value
    .split(/(?<=[.!?])\s+|\n+/)
    .map((line) => line.trim())
    .find(Boolean);
  return (first ?? "Imported from the legacy Skills configuration.").slice(0, 240);
}
