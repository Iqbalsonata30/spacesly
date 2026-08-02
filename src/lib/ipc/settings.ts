import type { AppSecrets } from "$lib/settings";
import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

export interface GlobalEnvironmentVariable {
  id: string;
  key: string;
  value: string;
  secret: boolean;
  enabled: boolean;
  value_set: boolean;
}

export interface GlobalEnvironmentVariableInput {
  id?: string | null;
  key: string;
  value?: string | null;
  secret: boolean;
  enabled: boolean;
}

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
  command: string,
  args: string[],
  environment: Record<string, string> | null,
): Promise<void> {
  return invokeWithPolicy<void>(
    "save_mcp_environment_secret",
    { serverId, command, args, environment },
    IPC_POLICIES.secret,
  );
}

export async function removeMcpConnector(serverId: string): Promise<void> {
  return invokeWithPolicy<void>("remove_mcp_connector", { serverId }, IPC_POLICIES.secret);
}

export async function saveJiraConnectionProfile(profile: {
  base_url: string;
  auth_mode: "api_token" | "pat" | "password";
  username: string;
  command: string;
  args: string[];
}): Promise<void> {
  return invokeWithPolicy<void>("save_jira_connection_profile", { profile }, IPC_POLICIES.secret);
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
  return invokeWithPolicy<void>("save_jira_secret", { secretType, value }, IPC_POLICIES.secret);
}

export async function listGlobalEnvironmentVariables(): Promise<GlobalEnvironmentVariable[]> {
  return invokeWithPolicy<GlobalEnvironmentVariable[]>(
    "list_global_environment_variables",
    undefined,
    IPC_POLICIES.secret,
  );
}

export async function saveGlobalEnvironmentVariable(
  variable: GlobalEnvironmentVariableInput,
): Promise<GlobalEnvironmentVariable> {
  return invokeWithPolicy<GlobalEnvironmentVariable>(
    "save_global_environment_variable",
    { input: variable },
    IPC_POLICIES.secret,
  );
}

export async function deleteGlobalEnvironmentVariable(id: string): Promise<void> {
  return invokeWithPolicy<void>("delete_global_environment_variable", { id }, IPC_POLICIES.secret);
}

export async function revealGlobalEnvironmentVariable(id: string): Promise<string> {
  return invokeWithPolicy<string>(
    "reveal_global_environment_variable",
    { id },
    IPC_POLICIES.secret,
  );
}
