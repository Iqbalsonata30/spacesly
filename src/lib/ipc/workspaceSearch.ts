import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

export interface WorkspaceSearchRequest {
  workspace_id: string;
  query: string;
  case_sensitive: boolean;
  max_results: number;
}

export interface WorkspaceSearchResult {
  file_path: string;
  line: number;
  character: number;
  preview: string;
}

export interface WorkspaceSearchResponse {
  results: WorkspaceSearchResult[];
  files_searched: number;
  truncated: boolean;
}

export interface WorkspaceReplacePreviewRequest {
  workspace_id: string;
  query: string;
  replacement: string;
  case_sensitive: boolean;
}

export interface WorkspaceReplacePlanFile {
  file_path: string;
  version: string;
  replacement_count: number;
}

export interface WorkspaceReplacePreviewFile extends WorkspaceReplacePlanFile {
  before_preview: string;
  after_preview: string;
}

export interface WorkspaceReplacePreviewResponse {
  files: WorkspaceReplacePreviewFile[];
  total_replacements: number;
  truncated: boolean;
}

export interface WorkspaceReplaceApplyRequest extends WorkspaceReplacePreviewRequest {
  files: WorkspaceReplacePlanFile[];
  truncated: boolean;
}

export interface WorkspaceReplaceAppliedFile {
  file_path: string;
  replacement_count: number;
}

export interface WorkspaceReplaceApplyResponse {
  files: WorkspaceReplaceAppliedFile[];
  total_replacements: number;
}

export async function searchWorkspace(
  request: WorkspaceSearchRequest,
): Promise<WorkspaceSearchResponse> {
  return invokeWithPolicy<WorkspaceSearchResponse>(
    "search_workspace",
    { request },
    IPC_POLICIES.fileRead,
  );
}

export async function previewWorkspaceReplace(
  request: WorkspaceReplacePreviewRequest,
): Promise<WorkspaceReplacePreviewResponse> {
  return invokeWithPolicy<WorkspaceReplacePreviewResponse>(
    "preview_workspace_replace",
    { request },
    IPC_POLICIES.fileRead,
  );
}

export async function applyWorkspaceReplace(
  request: WorkspaceReplaceApplyRequest,
): Promise<WorkspaceReplaceApplyResponse> {
  return invokeWithPolicy<WorkspaceReplaceApplyResponse>(
    "apply_workspace_replace",
    { request },
    IPC_POLICIES.fileWrite,
  );
}
