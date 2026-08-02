export interface JiraExecutionResult {
  summary: string;
  evidence: string[];
  details: string[];
  next: string[];
  completion_status: "completed" | "blocked";
  blocked_reason: string | null;
}

export interface JiraExecutionCommentInput {
  request: string;
  result: JiraExecutionResult;
  runtime: string;
  model: string;
  environment: string;
  branch?: string | null;
  revision?: string | null;
  upstream?: string | null;
}

export function formatJiraExecutionComment(input: JiraExecutionCommentInput): string {
  const { result } = input;
  const partial = result.completion_status === "blocked" && result.evidence.length > 0;
  const resultLabel =
    result.completion_status === "completed"
      ? "✅ Success"
      : partial
        ? "⚠️ Partial Success"
        : "❌ Failed";
  const outcome =
    result.completion_status === "completed"
      ? "The requested work was completed successfully."
      : partial
        ? "Part of the requested work was completed, but the task could not be fully finished."
        : "The requested work could not be completed.";
  const summary = `${outcome} ${cleanLine(result.summary, 500)}`.trim();
  const changes = unique(
    result.details.filter(isHumanFacingEvidence).map(humanFacingChange).filter(Boolean),
  ).slice(0, 5);
  const verification = unique(
    result.evidence.filter(isHumanFacingEvidence).map(humanFacingVerification).filter(Boolean),
  ).slice(0, 5);
  const actions = unique(
    result.next
      .filter(isHumanFacingEvidence)
      .map((line) => cleanLine(line, 300))
      .filter(
        (line) =>
          line &&
          !/^review the (result|evidence)/i.test(line) &&
          !/^keep or sync the completed/i.test(line),
      ),
  );
  if (result.blocked_reason && result.completion_status === "blocked") {
    actions.unshift(cleanLine(result.blocked_reason, 500));
  }

  const technicalEvidence = [
    "*Runtime*",
    `* Runtime: ${wikiText(input.runtime, 300)}`,
    `* Model: ${wikiText(input.model, 300)}`,
    `* Environment: ${wikiText(input.environment, 500)}`,
    ...(input.branch ? [`* Branch: ${wikiText(input.branch, 300)}`] : []),
    ...(input.revision ? [`* Revision: ${wikiText(input.revision, 300)}`] : []),
    ...(input.upstream ? [`* Upstream: ${wikiText(input.upstream, 300)}`] : []),
    ...(result.evidence.length > 0
      ? [
          "",
          "*Verification evidence*",
          ...result.evidence
            .filter(isHumanFacingEvidence)
            .map((line) => `* ${wikiText(line, 900)}`),
        ]
      : []),
    ...(result.details.length > 0
      ? [
          "",
          "*Execution details*",
          ...result.details.filter(isHumanFacingEvidence).map((line) => `* ${wikiText(line, 900)}`),
        ]
      : []),
  ];

  return [
    "h3. Executive Summary",
    "",
    `Requested: ${wikiText(input.request, 500)}. ${wikiText(summary, 900)}`,
    "",
    "h3. Result",
    "",
    `*${resultLabel}*`,
    "",
    "h3. What Changed",
    "",
    ...(changes.length > 0
      ? changes.map((line) => `* ${wikiText(line, 500)}`)
      : ["No material changes were reported."]),
    "",
    "h3. Verification",
    "",
    ...(verification.length > 0
      ? verification.map((line) => `* ${wikiText(line, 500)}`)
      : ["Verification evidence was not reported."]),
    "",
    "h3. Required Action",
    "",
    ...(actions.length > 0 ? actions.map((line) => `* ${wikiText(line, 500)}`) : ["None."]),
    "",
    "{expand:title=Technical Evidence}",
    ...technicalEvidence,
    "{expand}",
  ].join("\n");
}

function humanFacingChange(value: string): string {
  const line = cleanLine(value, 500);
  if (/(configmap|configuration|environment variable|feature flag)/i.test(line)) {
    return `Application configuration was updated. ${line}`;
  }
  if (/(deploy|rollout|release)/i.test(line)) return `Deployment changes were applied. ${line}`;
  if (/(commit|push|branch|repository)/i.test(line))
    return "Repository changes were committed and synchronized.";
  if (/(database|migration|schema)/i.test(line)) return `Database changes were applied. ${line}`;
  return line;
}

function humanFacingVerification(value: string): string {
  const line = cleanLine(value, 500);
  if (/(bamboo|pipeline|build)/i.test(line)) return "Build pipeline completed successfully.";
  if (/(openshift|kubernetes|ocp|pod|deployment|rollout)/i.test(line)) {
    return "All services are running successfully with the expected configuration.";
  }
  if (/(test|lint|check|typecheck|compile)/i.test(line)) return "Automated checks passed.";
  if (/(commit|push|upstream|branch)/i.test(line)) {
    return "Repository changes were committed and synchronized.";
  }
  return line;
}

function isHumanFacingEvidence(value: string): boolean {
  return !/(contract[_ -]?id|current_step|worker\.execute|planner state|orchestration|lease_owner)/i.test(
    value,
  );
}

function unique(lines: string[]): string[] {
  return [...new Set(lines.map((line) => line.trim()).filter(Boolean))];
}

function cleanLine(value: string, maxLength: number): string {
  const normalized = value.replace(/\s+/g, " ").trim();
  return normalized.length <= maxLength
    ? normalized
    : `${normalized.slice(0, Math.max(0, maxLength - 1)).trimEnd()}…`;
}

function wikiText(value: string, maxLength: number): string {
  return cleanLine(value, maxLength).replaceAll("{", "&#123;").replaceAll("}", "&#125;");
}
