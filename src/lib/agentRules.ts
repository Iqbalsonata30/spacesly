export type AgentRuleNotice = {
  level: "warning" | "suggestion";
  message: string;
};

export type AgentRulesSummary = {
  characterCount: number;
  lineCount: number;
  rules: string[];
  notices: AgentRuleNotice[];
};

export const exampleAgentRules = [
  "Verify the current state before making changes.",
  "Ask before performing destructive actions.",
  "Avoid modifying files unrelated to the task.",
].join("\n");

const MAX_AGENT_RULES_BYTES = 32 * 1024;

/** Builds the deterministic Rules snapshot used by new Agent executions. */
export function resolveAgentRulesSnapshot(value: string): string {
  const seen = new Set<string>();
  const rules: string[] = [];
  for (const line of value.split("\n")) {
    const rule = line.trim();
    if (!rule || seen.has(rule)) continue;
    seen.add(rule);
    rules.push(rule);
  }
  const snapshot = rules.join("\n");
  const bytes = new TextEncoder().encode(snapshot).length;
  if (bytes > MAX_AGENT_RULES_BYTES) {
    throw new Error(
      `Agent Rules exceed the ${MAX_AGENT_RULES_BYTES / 1024} KiB execution limit (${bytes} bytes).`,
    );
  }
  return snapshot;
}

export function summarizeAgentRules(value: string): AgentRulesSummary {
  const lines = value.split("\n");
  const rules = lines.map((line) => line.trim()).filter(Boolean);
  const notices: AgentRuleNotice[] = [];
  const firstLineByRule = new Map<string, number>();

  for (const [index, line] of lines.entries()) {
    const rule = line.trim();
    if (!rule) continue;
    const firstLine = firstLineByRule.get(rule);
    if (firstLine !== undefined) {
      notices.push({
        level: "warning",
        message: `Rule ${index + 1} is identical to rule ${firstLine}.`,
      });
    } else {
      firstLineByRule.set(rule, index + 1);
    }
    if (rule.length > 240) {
      notices.push({
        level: "suggestion",
        message: `Rule ${index + 1} is unusually long. Consider splitting it into separate rules.`,
      });
    }
  }

  if (!value.trim()) {
    notices.push({ level: "suggestion", message: "No rules have been added yet." });
  }
  if (/\n\s*\n\s*\n/.test(value)) {
    notices.push({
      level: "suggestion",
      message: "Several consecutive blank lines make the rule list harder to scan.",
    });
  }

  return {
    characterCount: value.length,
    lineCount: lines.length,
    rules,
    notices,
  };
}
