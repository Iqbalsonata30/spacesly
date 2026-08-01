import type { AgentRunLog } from "$lib/agentRun";

export type TimelineStatus =
  "running" | "completed" | "failed" | "waiting" | "cancelled" | "neutral";
export type TimelineImportance = "major" | "minor";

export type TimelineSection = {
  title: string;
  lines: string[];
};

export type TimelineActivity = {
  id: string;
  log: AgentRunLog;
  key: string;
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

const HIDDEN_PRESENTATION_LABELS = new Set(["board", "local", "model", "operator"]);

export function timelineActivities(logs: AgentRunLog[], limit = 10): TimelineActivity[] {
  const activities: TimelineActivity[] = [];
  const seenLogIds = new Set<string>();
  let terminalStatus: Extract<TimelineStatus, "completed" | "failed" | "cancelled"> | null = null;
  for (const log of logs) {
    if (seenLogIds.has(log.id)) continue;
    seenLogIds.add(log.id);
    const activity = timelineActivity(log);
    if (!activity.title) continue;

    const lifecycleState = lifecycleStateFromLog(log);
    const incomingTerminalStatus = terminalActivityStatus(lifecycleState);
    if (terminalStatus && !incomingTerminalStatus) continue;
    const existingIndex = activities.findIndex((candidate) => candidate.key === activity.key);
    const existing = existingIndex >= 0 ? activities[existingIndex] : null;

    if (incomingTerminalStatus) {
      terminalStatus = incomingTerminalStatus;
      finalizeRunningActivities(activities, incomingTerminalStatus);
    } else if (activity.status === "running") {
      finalizeRunningActivities(
        activities,
        "completed",
        existing?.status === "running" ? existing : null,
      );
    } else if (activity.status === "waiting") {
      finalizeRunningActivities(activities, "completed");
    } else if (activity.status === "failed") {
      finalizeRunningActivities(activities, "failed");
    } else if (activity.status === "cancelled") {
      finalizeRunningActivities(activities, "cancelled");
    }

    if (existing) {
      existing.repeatCount += 1;
      existing.log = log;
      existing.title = activity.title;
      existing.summary = activity.summary;
      existing.status = activity.status;
      existing.importance = activity.importance;
      existing.sections = mergeSections(existing.sections, activity.sections);
      activities.splice(existingIndex, 1);
      activities.push(existing);
      continue;
    }
    activities.push(activity);
  }
  return activities.slice(-limit).reverse();
}

function finalizeRunningActivities(
  activities: TimelineActivity[],
  status: Extract<TimelineStatus, "completed" | "failed" | "cancelled">,
  except: TimelineActivity | null = null,
): void {
  for (const activity of activities) {
    if (activity !== except && activity.status === "running") activity.status = status;
  }
}

function terminalActivityStatus(
  lifecycleState: string | null,
): Extract<TimelineStatus, "completed" | "failed" | "cancelled"> | null {
  if (lifecycleState === "succeeded") return "completed";
  if (lifecycleState === "failed" || lifecycleState === "blocked") return "failed";
  if (lifecycleState === "cancelled") return "cancelled";
  return null;
}

export function timelineActivity(log: AgentRunLog): TimelineActivity {
  const parsed = parseStructuredLog(log.message);
  const rawSummary =
    parsed.summary ?? firstUsefulLine(parsed.unstructured) ?? "Execution activity recorded.";
  const presentation = presentationForLog(log, parsed, rawSummary);
  const sections = technicalSections(parsed, log);
  return {
    id: log.id,
    log,
    ...presentation,
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
  let section: keyof Pick<ParsedLog, "evidence" | "details" | "next"> | "unstructured" =
    "unstructured";
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

function presentationForLog(
  log: AgentRunLog,
  parsed: ParsedLog,
  rawSummary: string,
): Pick<TimelineActivity, "key" | "title" | "summary" | "status" | "importance"> {
  const lifecycleState = lifecycleStateFrom(rawSummary);
  if (log.label === "lifecycle" && lifecycleState) return lifecyclePresentation(lifecycleState);

  if (log.label === "progress") {
    const phase = parsed.details.find((line) => /^Progress phase:/i.test(line)) ?? rawSummary;
    if (/commit|persist|saving/i.test(phase)) {
      return activity(
        "saving-results",
        "Saving Results",
        "Persisting execution results.",
        "running",
      );
    }
    if (/resolv|prepar|context|queue/i.test(phase)) {
      return activity(
        "preparing-task",
        "Preparing Task",
        "Gathering the information needed to begin.",
        "running",
      );
    }
    return activity(
      "executing-task",
      "Executing Task",
      "The agent is currently working.",
      "running",
    );
  }

  if (HIDDEN_PRESENTATION_LABELS.has(log.label)) return hiddenActivity(log.id);
  if (log.label === "runtime" && /started|initialized|completed|result/i.test(rawSummary)) {
    return hiddenActivity(log.id);
  }
  if (log.label === "start" || log.label === "context") {
    return activity(
      "preparing-task",
      "Preparing Task",
      "Gathering the information needed to begin.",
      "running",
    );
  }
  if (log.label === "continue") {
    return activity(
      "preparing-task",
      "Preparing Task",
      "Restoring the previous task context.",
      "running",
    );
  }
  if (log.label === "agent" && log.tone === "success") {
    return activity("completed", "Execution Completed", "Task finished successfully.", "completed");
  }
  if (log.label === "manual-done") {
    return activity("completed", "Execution Completed", "Task finished successfully.", "completed");
  }
  if (log.label === "approval") {
    return activity(
      "approval",
      "Approval Required",
      "The agent is waiting for your approval.",
      "waiting",
    );
  }
  if (log.label === "timeout") {
    return activity(
      "attention",
      "Execution Timed Out",
      "The agent stopped while waiting for a response.",
      "failed",
    );
  }

  const operation = toolOperation(rawSummary);
  if (log.label === "tool" || isBusinessToolLabel(log.label)) {
    const title = businessActionTitle(log.label, operation);
    const status = toolStatus(log, parsed.status, rawSummary);
    return activity(
      `tool:${normalizeKey(title)}`,
      title,
      status === "failed"
        ? "The action needs attention before the agent can continue."
        : status === "completed"
          ? "The action finished successfully."
          : businessActionSummary(title),
      status,
    );
  }

  if (log.tone === "error") {
    return activity(
      "attention",
      "Execution Needs Attention",
      "Review the details before continuing.",
      "failed",
    );
  }
  return activity("executing-task", "Executing Task", "The agent is currently working.", "running");
}

function lifecyclePresentation(state: string): ReturnType<typeof activity> {
  if (state === "queued")
    return activity("queued", "Queued", "Waiting for an available worker.", "running");
  if (state === "running")
    return activity("agent-started", "Agent Started", "Execution has started.", "running");
  if (state === "committing")
    return activity("saving-results", "Saving Results", "Persisting execution results.", "running");
  if (state === "succeeded")
    return activity("completed", "Execution Completed", "Task finished successfully.", "completed");
  if (state === "blocked")
    return activity(
      "attention",
      "Input Required",
      "The agent needs your input to continue.",
      "waiting",
    );
  if (state === "failed")
    return activity(
      "attention",
      "Execution Needs Attention",
      "The task did not finish successfully.",
      "failed",
    );
  if (state === "cancelled")
    return activity("cancelled", "Execution Cancelled", "The task was cancelled.", "cancelled");
  if (state === "cancelling")
    return activity("cancelling", "Cancelling Execution", "Stopping the active task.", "running");
  return activity("executing-task", "Executing Task", "The agent is currently working.", "running");
}

function activity(
  key: string,
  title: string,
  summary: string,
  status: TimelineStatus,
): Pick<TimelineActivity, "key" | "title" | "summary" | "status" | "importance"> {
  return { key, title, summary, status, importance: "major" };
}

function hiddenActivity(key: string): ReturnType<typeof activity> {
  return activity(`hidden:${key}`, "", "", "neutral");
}

function lifecycleStateFrom(summary: string): string | null {
  return summary.match(/entered\s+([a-z_]+)/i)?.[1]?.toLowerCase() ?? null;
}

function lifecycleStateFromLog(log: AgentRunLog): string | null {
  if (log.label !== "lifecycle") return null;
  return lifecycleStateFrom(log.message);
}

function toolOperation(summary: string): string {
  return cleanTimelineLine(summary)
    .replace(/^(Tool started|Tool failed|Tool completed(?:; task still running)?):\s*/i, "")
    .replace(/\.$/, "")
    .trim();
}

function isBusinessToolLabel(label: string): boolean {
  return ["files", "commands", "git", "jira", "kubernetes", "bamboo", "external"].includes(label);
}

function businessActionTitle(label: string, operation: string): string {
  const context = `${label} ${operation}`.toLowerCase();
  const reading = /read|get|list|fetch|inspect|check|status|search|collect|cat/.test(context);
  const verifying = /verify|test|lint|validate|health|diagnos/.test(context);
  const configuration =
    /config|\.ya?ml|\.json|\.toml|environment|variable|manifest|helm|template/.test(context);

  if (/jira|ticket|issue/.test(context))
    return reading ? "Reading Jira Ticket" : "Updating Jira Ticket";
  if (/kubernetes|cluster|pod|openshift|ocp/.test(context)) {
    if (verifying || reading) return "Checking Kubernetes Resources";
    if (/deploy|apply|rollout|restart|scale/.test(context)) return "Deploying Application";
    return "Updating Kubernetes Resources";
  }
  if (/bamboo|deployment|build/.test(context)) {
    if (verifying || reading) return "Reading Deployment Status";
    return "Deploying Application";
  }
  if (verifying) return "Verifying Changes";
  if (configuration) return reading ? "Reading Configuration" : "Updating Configuration";
  if (label === "files") return reading ? "Collecting Workspace Information" : "Updating Workspace";
  if (label === "git") return reading ? "Checking Repository" : "Saving Changes";
  if (label === "commands") return "Preparing Workspace";
  return reading ? "Collecting Information" : "Executing Task Action";
}

function businessActionSummary(title: string): string {
  if (title.startsWith("Reading")) return "The agent is gathering the latest information.";
  if (title.startsWith("Checking") || title.startsWith("Verifying"))
    return "The agent is checking the current state.";
  if (title.startsWith("Updating")) return "The agent is applying the requested changes.";
  if (title === "Deploying Application") return "The agent is applying the deployment changes.";
  if (title === "Saving Changes") return "The agent is recording the completed changes.";
  return "The agent is working on this action.";
}

function toolStatus(
  log: AgentRunLog,
  structuredStatus: string | null,
  summary: string,
): TimelineStatus {
  if (log.tone === "error" || /tool failed/i.test(summary)) return "failed";
  if (
    log.tone === "success" ||
    /tool completed|^completed:/i.test(summary) ||
    /complete/i.test(structuredStatus ?? "")
  ) {
    return "completed";
  }
  return "running";
}

function normalizeKey(value: string): string {
  return value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
}

function technicalSections(parsed: ParsedLog, log: AgentRunLog): TimelineSection[] {
  const sections: TimelineSection[] = [];
  if (parsed.evidence.length > 0) sections.push({ title: "Evidence", lines: parsed.evidence });
  if (parsed.details.length > 0) sections.push({ title: "Runtime details", lines: parsed.details });
  if (parsed.next.length > 0) sections.push({ title: "Follow-up", lines: parsed.next });
  sections.push({
    title: "Raw event",
    lines: [`${log.label} · ${log.tone}`, ...log.message.split("\n").filter(Boolean)],
  });
  return sections;
}

function firstUsefulLine(lines: string[]): string | null {
  return lines.map(cleanTimelineLine).find(Boolean) ?? null;
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
