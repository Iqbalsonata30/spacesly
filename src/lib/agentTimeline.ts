import type { AgentRunLog } from "$lib/agentRun";

export type TimelineStatus = "running" | "completed" | "failed" | "waiting" | "neutral";
export type TimelineImportance = "major" | "minor";

export type TimelineSection = {
  title: string;
  lines: string[];
};

export type TimelineActivity = {
  log: AgentRunLog;
  title: string;
  summary: string;
  status: TimelineStatus;
  importance: TimelineImportance;
  sections: TimelineSection[];
  repeatCount: number;
};

type ParsedLog = {
  status: string | null;
  summary: string | null;
  evidence: string[];
  details: string[];
  next: string[];
  unstructured: string[];
};

const MAJOR_LABELS = new Set(["start", "continue", "context", "agent", "blocked", "timeout", "approval"]);
const EXTERNAL_LABELS = new Set(["jira", "kubernetes", "bamboo"]);

export function timelineActivities(logs: AgentRunLog[], limit = 10): TimelineActivity[] {
  const activities: TimelineActivity[] = [];
  for (const log of logs) {
    const activity = timelineActivity(log);
    const previous = activities[activities.length - 1];
    if (
      previous &&
      previous.title === activity.title &&
      previous.summary === activity.summary &&
      previous.status === activity.status
    ) {
      previous.repeatCount += 1;
      previous.log = log;
      previous.sections = mergeSections(previous.sections, activity.sections);
      continue;
    }
    activities.push(activity);
  }
  return activities.slice(-limit);
}

export function timelineActivity(log: AgentRunLog): TimelineActivity {
  const parsed = parseStructuredLog(log.message);
  const rawSummary = parsed.summary ?? firstUsefulLine(parsed.unstructured) ?? "Execution activity recorded.";
  const title = titleForLog(log, rawSummary);
  const summary = summaryForLog(log, rawSummary, title);
  const sections = technicalSections(parsed, log);
  return {
    log,
    title,
    summary,
    status: statusForLog(log, parsed.status, rawSummary),
    importance: importanceForLog(log),
    sections,
    repeatCount: 1,
  };
}

export function cleanTimelineLine(value: string): string {
  return value
    .replace(/^[-*]\s*/, "")
    .replace(/^(EVIDENCE|DETAILS|NEXT|SUMMARY|STATUS):\s*/i, "")
    .trim();
}

function parseStructuredLog(message: string): ParsedLog {
  const parsed: ParsedLog = {
    status: null,
    summary: null,
    evidence: [],
    details: [],
    next: [],
    unstructured: [],
  };
  let section: keyof Pick<ParsedLog, "evidence" | "details" | "next"> | "unstructured" = "unstructured";
  for (const line of message.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const heading = trimmed.match(/^(STATUS|SUMMARY|EVIDENCE|DETAILS|NEXT):\s*(.*)$/i);
    if (heading) {
      const name = heading[1].toUpperCase();
      const value = cleanTimelineLine(heading[2] ?? "");
      if (name === "STATUS") parsed.status = value || null;
      else if (name === "SUMMARY") parsed.summary = value || null;
      else section = name.toLowerCase() as typeof section;
      continue;
    }
    const cleaned = cleanTimelineLine(trimmed);
    if (!cleaned) continue;
    parsed[section].push(cleaned);
  }
  return parsed;
}

function titleForLog(log: AgentRunLog, summary: string): string {
  if (log.tone === "error") {
    if (log.label === "timeout") return "Execution timed out";
    if (log.label === "approval") return "Approval required";
    return "Execution needs attention";
  }

  if (log.label === "agent") return log.tone === "success" ? "Task completed" : "Task result prepared";
  if (log.label === "start") return "Preparing task";
  if (log.label === "continue") return "Continuing task";
  if (log.label === "board") return log.tone === "success" ? "Board updated" : "Updating board";
  if (log.label === "model") return "Runtime configured";
  if (log.label === "local") return "Local execution selected";
  if (log.label === "context") return "Preparing execution context";
  if (log.label === "runtime") return log.tone === "success" ? "Runtime completed" : "Runtime initialized";
  if (log.label === "operator") return "Guidance added";
  if (log.label === "approval") return "Approval recorded";
  if (log.label === "manual-done") return "Task marked complete";
  if (log.label === "files") return actionTitle(summary, "Workspace updated");
  if (log.label === "commands") return actionTitle(summary, "Command executed");
  if (log.label === "git") return actionTitle(summary, "Repository updated");
  if (log.label === "jira") return jiraTitle(summary, log.tone);
  if (log.label === "kubernetes") return actionTitle(summary, "Cluster updated");
  if (log.label === "bamboo") return actionTitle(summary, "Deployment updated");
  if (log.label === "external") return actionTitle(summary, "External tool completed");
  return actionTitle(summary, "Execution activity");
}

function summaryForLog(log: AgentRunLog, summary: string, title: string): string {
  if (log.tone === "error") return sentence(summary);
  if (log.label === "start") return "Spacesly opened the execution session and prepared the work item.";
  if (log.label === "continue") return "Spacesly resumed the existing execution with prior context.";
  if (log.label === "board" && log.tone === "success") return "The local task state was updated with the latest execution result.";
  if (log.label === "board") return "The task was moved into the active execution flow.";
  if (log.label === "local") return "This task will run locally without Jira synchronization.";
  if (log.label === "operator") return "Your note was saved and will be included in the execution context.";
  if (log.label === "approval") return "The run has the approval signal it needs to continue.";
  if (EXTERNAL_LABELS.has(log.label) && log.tone === "success") return sentence(summary);
  if (log.label === "runtime" && /started/i.test(summary)) {
    return "Spacesly is tracking execution events from the AI runtime.";
  }
  if (log.label === "model") return "The selected AI runtime and model are ready for this task.";
  if (log.label === "context") return "Workspace, task, evidence, and operator context were prepared for execution.";
  if (log.label === "agent") return sentence(summary);
  if (title === summary) return "Spacesly is progressing through this step.";
  return sentence(summary);
}

function statusForLog(
  log: AgentRunLog,
  structuredStatus: string | null,
  summary: string,
): TimelineStatus {
  if (log.tone === "error") return log.label === "approval" ? "waiting" : "failed";
  if (
    log.tone === "success" ||
    /complete|completed/i.test(structuredStatus ?? "") ||
    /^completed:/i.test(summary)
  ) {
    return "completed";
  }
  if (log.label === "approval") return "waiting";
  return "running";
}

function importanceForLog(log: AgentRunLog): TimelineImportance {
  if (log.tone !== "info") return "major";
  if (MAJOR_LABELS.has(log.label) || EXTERNAL_LABELS.has(log.label)) return "major";
  return "minor";
}

function technicalSections(parsed: ParsedLog, log: AgentRunLog): TimelineSection[] {
  const sections: TimelineSection[] = [];
  if (parsed.evidence.length > 0) sections.push({ title: "Evidence", lines: parsed.evidence });
  if (parsed.details.length > 0) sections.push({ title: "Runtime details", lines: parsed.details });
  if (parsed.next.length > 0) sections.push({ title: "Follow-up", lines: parsed.next });
  sections.push({ title: "Raw event", lines: [`${log.label} · ${log.tone}`, ...log.message.split("\n").filter(Boolean)] });
  return sections;
}

function actionTitle(summary: string, fallback: string): string {
  return stripOutcomePrefix(sentence(summary)) || fallback;
}

function jiraTitle(summary: string, tone: AgentRunLog["tone"]): string {
  if (/done in jira|completion comment posted|in progress in jira/i.test(summary)) return "Jira updated";
  if (/assigning|moving|posting/i.test(summary)) return stripOutcomePrefix(sentence(summary));
  return tone === "success" ? "Jira updated" : "Updating Jira";
}

function stripOutcomePrefix(value: string): string {
  return value.replace(/^(Completed|Failed):\s*/i, "").replace(/\.$/, "");
}

function firstUsefulLine(lines: string[]): string | null {
  return lines.map(cleanTimelineLine).find(Boolean) ?? null;
}

function sentence(value: string): string {
  const cleaned = cleanTimelineLine(value);
  if (!cleaned) return cleaned;
  return /[.!?]$/.test(cleaned) ? cleaned : `${cleaned}.`;
}

function mergeSections(left: TimelineSection[], right: TimelineSection[]): TimelineSection[] {
  const merged = left.map((section) => ({ ...section, lines: [...section.lines] }));
  for (const section of right) {
    const existing = merged.find((candidate) => candidate.title === section.title);
    if (!existing) {
      merged.push({ ...section, lines: [...section.lines] });
      continue;
    }
    for (const line of section.lines) {
      if (!existing.lines.includes(line)) existing.lines.push(line);
    }
  }
  return merged;
}
