import type { AppSecrets } from "$lib/settings";
import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

export async function loadAppSecrets(): Promise<AppSecrets> {
  return invokeWithPolicy<AppSecrets>("load_app_secrets", undefined, IPC_POLICIES.secret);
}

export async function saveAppSecrets(secrets: AppSecrets): Promise<void> {
  return invokeWithPolicy<void>("save_app_secrets", { secrets }, IPC_POLICIES.secret);
}

export async function aiProviderSecretStatuses(): Promise<Record<string, boolean>> {
  return invokeWithPolicy<Record<string, boolean>>(
    "ai_provider_secret_statuses",
    undefined,
    IPC_POLICIES.secret,
  );
}

export async function saveAiProviderSecret(
  providerId: string,
  apiKey: string | null,
): Promise<void> {
  return invokeWithPolicy<void>(
    "save_ai_provider_secret",
    { providerId, apiKey },
    IPC_POLICIES.secret,
  );
}

export async function mcpEnvironmentSecretStatuses(): Promise<Record<string, string[]>> {
  return invokeWithPolicy<Record<string, string[]>>(
    "mcp_environment_secret_statuses",
    undefined,
    IPC_POLICIES.secret,
  );
}

export async function saveMcpEnvironmentSecret(
  serverId: string,
  environment: Record<string, string>,
): Promise<void> {
  return invokeWithPolicy<void>(
    "save_mcp_environment_secret",
    { serverId, environment },
    IPC_POLICIES.secret,
  );
}

export async function jiraSecretStatuses(): Promise<Record<string, boolean>> {
  return invokeWithPolicy<Record<string, boolean>>(
    "jira_secret_statuses",
    undefined,
    IPC_POLICIES.secret,
  );
}

export async function saveJiraSecret(
  secretType: "api_token" | "personal_access_token" | "password",
  value: string | null,
): Promise<void> {
  return invokeWithPolicy<void>(
    "save_jira_secret",
    { secretType, value },
    IPC_POLICIES.secret,
  );
}
