import type { TaskProgress, TaskSessionState } from "$lib/ipc/taskSessions";
import type { AiRun, AiRunStatus } from "$lib/ipc/agent";
import type { WorkspaceChatActionProposal } from "$lib/workspaceChat";

export type WorkspaceChatRun = {
  generation: number;
  running: boolean;
  legacyRunId: string | null;
  taskSessionId: number | null;
  state: TaskSessionState | null;
  progress: TaskProgress | null;
  error: string | null;
  streamingText: string;
  streamBuffer: string;
  streamFrame: number | null;
  lastEventSequence: number;
  actionProposal: WorkspaceChatActionProposal | null;
};

export type WorkspaceChatRuns = Record<string, WorkspaceChatRun>;

export type LegacyCancellationTerminalState = "cancelled" | "failed" | "succeeded";

type LegacyCancellationDependencies = {
  cancel: (runId: string) => Promise<boolean>;
  getRun: (runId: string) => Promise<AiRun | null>;
  sleep?: (milliseconds: number) => Promise<void>;
  now?: () => number;
  timeoutMs?: number;
  pollMs?: number;
};

/** Requests cancellation and retains identity until backend state is observably terminal. */
export async function confirmLegacyWorkspaceChatCancellation(
  runId: string,
  dependencies: LegacyCancellationDependencies,
): Promise<LegacyCancellationTerminalState | null> {
  const accepted = await dependencies.cancel(runId);
  if (!accepted) return null;
  const now = dependencies.now ?? Date.now;
  const sleep =
    dependencies.sleep ??
    ((milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds)));
  const deadline = now() + (dependencies.timeoutMs ?? 5_000);
  while (now() < deadline) {
    const run = await dependencies.getRun(runId);
    const terminal = legacyCancellationTerminalState(run?.status);
    if (terminal) return terminal;
    await sleep(dependencies.pollMs ?? 50);
  }
  return null;
}

function legacyCancellationTerminalState(
  status: AiRunStatus | undefined,
): LegacyCancellationTerminalState | null {
  if (status === "cancelled") return "cancelled";
  if (status === "completed") return "succeeded";
  if (status === "failed" || status === "blocked") return "failed";
  return null;
}

/** Creates the isolated execution state owned by one conversation. */
export function createWorkspaceChatRun(generation = 0): WorkspaceChatRun {
  return {
    generation,
    running: false,
    legacyRunId: null,
    taskSessionId: null,
    state: null,
    progress: null,
    error: null,
    streamingText: "",
    streamBuffer: "",
    streamFrame: null,
    lastEventSequence: 0,
    actionProposal: null,
  };
}

export function workspaceChatRunFor(
  runs: WorkspaceChatRuns,
  conversationId: string,
): WorkspaceChatRun {
  return runs[conversationId] ?? createWorkspaceChatRun();
}

export function updateWorkspaceChatRun(
  runs: WorkspaceChatRuns,
  conversationId: string,
  transform: (run: WorkspaceChatRun) => WorkspaceChatRun,
): WorkspaceChatRuns {
  return {
    ...runs,
    [conversationId]: transform(workspaceChatRunFor(runs, conversationId)),
  };
}

/** Invalidates one conversation while retaining its identity until terminal cancellation. */
export function cancelWorkspaceChatRun(
  runs: WorkspaceChatRuns,
  conversationId: string,
): WorkspaceChatRuns {
  return updateWorkspaceChatRun(runs, conversationId, (run) => ({
    ...run,
    generation: run.generation + 1,
    running: run.running,
    state: run.running ? "cancelling" : run.state,
    streamingText: "",
    streamBuffer: "",
    streamFrame: null,
    lastEventSequence: run.lastEventSequence,
  }));
}

/** Clears cancellation identity only after the backend confirms a terminal state. */
export function settleWorkspaceChatCancellation(
  runs: WorkspaceChatRuns,
  conversationId: string,
  generation: number,
  terminalState: LegacyCancellationTerminalState | null,
): WorkspaceChatRuns {
  return updateWorkspaceChatRun(runs, conversationId, (run) => {
    if (run.generation !== generation) return run;
    if (terminalState === null) {
      return { ...run, error: "Cancellation could not be confirmed.", state: "cancelling" };
    }
    return {
      ...run,
      running: false,
      legacyRunId: null,
      taskSessionId: null,
      state: terminalState,
      error: terminalState === "failed" ? "Cancellation failed." : null,
      lastEventSequence: 0,
    };
  });
}

export function workspaceChatRunStatus(run: WorkspaceChatRun): string | null {
  if (run.error) return "failed";
  if (run.state && !["succeeded", "cancelled"].includes(run.state)) return run.state;
  if (run.running) return "running";
  return null;
}

export function workspaceChatProgressPercent(run: WorkspaceChatRun): number | null {
  const progress = run.progress;
  if (!progress) return null;
  if (progress.total === null || progress.total <= 0) return progress.completed > 0 ? 50 : 0;
  return Math.max(0, Math.min(100, Math.round((progress.completed / progress.total) * 100)));
}
