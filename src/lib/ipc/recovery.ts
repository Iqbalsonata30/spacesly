import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";
import type { LineEnding, TextEncoding } from "$lib/ipc/files";

export interface RecoverySnapshotInput {
  path: string;
  name: string;
  content: string;
  persisted_version: string;
  root_revision: number;
  encoding: TextEncoding;
  line_ending: LineEnding;
  revision: number;
  scroll_top: number;
}

export interface RecoverySnapshot extends RecoverySnapshotInput {
  workspace_id: string;
  persisted_content: string;
  current_version: string | null;
  disk_status: "unchanged" | "changed" | "missing";
  updated_at: number;
}

export async function syncRecoverySnapshots(
  workspaceId: string,
  snapshots: RecoverySnapshotInput[],
): Promise<void> {
  return invokeWithPolicy<void>(
    "sync_recovery_snapshots",
    { workspaceId, snapshots },
    IPC_POLICIES.fileWrite,
  );
}

export async function listRecoverySnapshots(workspaceId: string): Promise<RecoverySnapshot[]> {
  return invokeWithPolicy<RecoverySnapshot[]>(
    "list_recovery_snapshots",
    { workspaceId },
    IPC_POLICIES.fileRead,
  );
}

export async function deleteRecoverySnapshot(workspaceId: string, path: string): Promise<void> {
  return invokeWithPolicy<void>(
    "delete_recovery_snapshot",
    { workspaceId, path },
    IPC_POLICIES.fileWrite,
  );
}
