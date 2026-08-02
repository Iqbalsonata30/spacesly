import { exampleAgentRules, summarizeAgentRules } from "../src/lib/agentRules";

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}

const clean = summarizeAgentRules(exampleAgentRules);
assert(clean.rules.length === 3, "example rules should contain three rules");
assert(clean.notices.length === 0, "example rules should not produce validation notices");

const duplicate = summarizeAgentRules("Verify first.\nAsk before deleting.\nVerify first.");
assert(
  duplicate.notices.some((notice) => notice.message === "Rule 3 is identical to rule 1."),
  "duplicate rules should identify both line numbers",
);

const longRule = summarizeAgentRules(`Verify ${"state ".repeat(50)}`);
assert(
  longRule.notices.some((notice) => notice.message.includes("unusually long")),
  "long rules should produce a non-blocking suggestion",
);

const empty = summarizeAgentRules("");
assert(empty.rules.length === 0, "empty text should produce no rules");
assert(
  empty.notices.some((notice) => notice.message === "No rules have been added yet."),
  "empty rules should provide guidance",
);

console.log("agent rules tests passed");
