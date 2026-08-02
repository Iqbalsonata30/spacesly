import type { ExecutionRun } from "$lib/agentRun";
import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

export async function saveExecutionRun(run: ExecutionRun): Promise<ExecutionRun> {
  return invokeWithPolicy<ExecutionRun>("save_execution_run", { run }, IPC_POLICIES.workspaceCache);
}

export async function listActiveExecutionRuns(): Promise<ExecutionRun[]> {
  return invokeWithPolicy<ExecutionRun[]>(
    "list_active_execution_runs",
    undefined,
    IPC_POLICIES.workspaceCache,
  );
}
