import type { AppSecrets } from "$lib/settings";
import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

export async function loadAppSecrets(): Promise<AppSecrets> {
  return invokeWithPolicy<AppSecrets>("load_app_secrets", undefined, IPC_POLICIES.secret);
}

export async function saveAppSecrets(secrets: AppSecrets): Promise<void> {
  return invokeWithPolicy<void>("save_app_secrets", { secrets }, IPC_POLICIES.secret);
}
