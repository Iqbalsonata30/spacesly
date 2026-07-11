import type { AppSecrets } from "$lib/settings";
import { invokeWithPolicy } from "$lib/ipc/policy";

const SECRET_POLICY = { timeoutMs: 10_000, retries: 0 };

export async function loadAppSecrets(): Promise<AppSecrets> {
  return invokeWithPolicy<AppSecrets>("load_app_secrets", undefined, SECRET_POLICY);
}

export async function saveAppSecrets(secrets: AppSecrets): Promise<void> {
  return invokeWithPolicy<void>("save_app_secrets", { secrets }, SECRET_POLICY);
}
