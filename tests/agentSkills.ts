import {
  createAgentSkill,
  duplicateAgentSkill,
  migrateLegacyAgentSkills,
  normalizeAgentSkills,
  skillMatchesSearch,
  validateAgentSkillCatalog,
  type AgentSkill,
} from "../src/lib/agentSkills";
import { normalizeSettings, settingsWithoutSecrets } from "../src/lib/settings";

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `${message}\nExpected: ${JSON.stringify(expected)}\nActual: ${JSON.stringify(actual)}`,
    );
  }
}

const migrated = migrateLegacyAgentSkills(
  [
    "Skill: Bamboo Diagnostics",
    "Inspect the failed build and deployment logs.",
    "",
    "Skill: OCP Troubleshooting",
    "Inspect Kubernetes pods, events, and logs.",
  ].join("\n"),
  "2026-08-01T00:00:00.000Z",
);
assertEqual(
  migrated.map((skill) => [skill.name, skill.instructions, skill.trigger, skill.metadata.source]),
  [
    ["Bamboo Diagnostics", "Inspect the failed build and deployment logs.", "automatic", "legacy"],
    ["OCP Troubleshooting", "Inspect Kubernetes pods, events, and logs.", "automatic", "legacy"],
  ],
  "legacy headings should migrate into separate automatic skills without losing instructions",
);
assertEqual(
  migrateLegacyAgentSkills("Always inspect evidence first.", "2026-08-01T00:00:00.000Z")[0]
    .instructions,
  "Always inspect evidence first.",
  "unstructured legacy guidance should remain intact in one imported skill",
);
const emptyLegacySkill = migrateLegacyAgentSkills(
  "Skill: Empty Legacy Skill",
  "2026-08-01T00:00:00.000Z",
)[0];
assertEqual(
  {
    enabled: emptyLegacySkill.enabled,
    valid: validateAgentSkillCatalog([emptyLegacySkill]),
  },
  { enabled: false, valid: null },
  "empty legacy sections should remain preserved but disabled without blocking future saves",
);

const migratedSettings = normalizeSettings({
  aiWorker: { agentSkills: "Skill: Existing Skill\nPreserve this procedure." },
});
assertEqual(
  migratedSettings.aiWorker.skills.map((skill) => [skill.name, skill.instructions]),
  [["Existing Skill", "Preserve this procedure."]],
  "settings normalization should migrate the legacy text field",
);
assertEqual(
  normalizeSettings({ aiWorker: { agentSkills: "" } }).aiWorker.skills,
  [],
  "an intentionally empty legacy field should remain empty",
);
const structuredRoundTrip = normalizeSettings(
  JSON.parse(JSON.stringify(settingsWithoutSecrets(migratedSettings))),
);
assertEqual(
  structuredRoundTrip.aiWorker.skills,
  migratedSettings.aiWorker.skills,
  "structured skill entities should survive secret-free settings persistence",
);

const normalized = normalizeAgentSkills([
  {
    id: "skill-normalized",
    name: " Normalized ",
    description: "Description",
    category: "testing",
    trigger: "contextual",
    priority: 140,
    enabled: true,
    instructions: "Run tests.",
    created_at: "invalid",
    updated_at: "invalid",
    metrics: { usage_count: -1, success_rate: 4, favorite: true },
    metadata: { source: "marketplace", version: 2, invalid: [] },
  },
]);
assertEqual(
  {
    name: normalized[0].name,
    priority: normalized[0].priority,
    usage: normalized[0].metrics.usage_count,
    success: normalized[0].metrics.success_rate,
    favorite: normalized[0].metrics.favorite,
    metadata: normalized[0].metadata,
  },
  {
    name: "Normalized",
    priority: 100,
    usage: 0,
    success: 1,
    favorite: true,
    metadata: { source: "marketplace", schema_version: 1, version: 2 },
  },
  "structured skills should normalize bounds while preserving future-safe metadata",
);

function skill(values: Partial<AgentSkill> & Pick<AgentSkill, "id" | "name">): AgentSkill {
  return {
    ...createAgentSkill("2026-08-01T00:00:00.000Z"),
    description: `${values.name} description`,
    instructions: `${values.name} instructions`,
    ...values,
  };
}

const catalog = [
  skill({
    id: "baseline",
    name: "Baseline",
    trigger: "automatic",
    category: "coding",
    priority: 10,
  }),
  skill({
    id: "deployment",
    name: "Deployment",
    trigger: "contextual",
    category: "deployment",
    priority: 80,
    notes: "Private maintainer note",
  }),
  skill({ id: "git", name: "Git", trigger: "contextual", category: "git", priority: 60 }),
  skill({
    id: "documentation",
    name: "Documentation",
    trigger: "contextual",
    category: "documentation",
    priority: 90,
  }),
  skill({
    id: "manual",
    name: "Manual",
    trigger: "manual",
    category: "custom",
    custom_category: "Audit",
  }),
  skill({ id: "disabled", name: "Disabled", trigger: "automatic", enabled: false }),
];

assertEqual(
  skillMatchesSearch(catalog[1], "deployment enabled contextual"),
  true,
  "search should cover metadata",
);
assertEqual(
  skillMatchesSearch(catalog[5], "disabled"),
  true,
  "search should cover effective status",
);
assertEqual(
  validateAgentSkillCatalog(catalog),
  null,
  "valid structured catalog should pass validation",
);
assertEqual(
  validateAgentSkillCatalog([...catalog, { ...catalog[0], id: "duplicate-name" }])?.includes(
    "already named",
  ),
  true,
  "duplicate names should fail catalog validation",
);
const oversizedCatalog = Array.from({ length: 65 }, (_, index) =>
  skill({ id: `large-${index}`, name: `Large ${index}` }),
);
assertEqual(
  normalizeAgentSkills(oversizedCatalog).length,
  65,
  "normalization must never truncate an over-limit persisted catalog",
);
assertEqual(
  validateAgentSkillCatalog(oversizedCatalog)?.includes("maximum"),
  true,
  "over-limit catalogs should be reported without losing entities",
);
assertEqual(
  skillMatchesSearch(skill({ id: "unicode", name: "Déploiement Étendu" }), "déploiement"),
  true,
  "search should preserve non-ASCII letters",
);

const duplicate = duplicateAgentSkill(catalog[0], "2026-08-02T00:00:00.000Z");
assertEqual(
  duplicate.id === catalog[0].id,
  false,
  "duplicating a skill should create a new stable entity",
);
assertEqual(duplicate.metrics.usage_count, 0, "duplicating should reset usage metrics");

console.log("agent skill tests passed");
