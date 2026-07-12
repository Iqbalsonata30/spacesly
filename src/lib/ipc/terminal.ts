import { Channel, invoke } from "@tauri-apps/api/core";
import { IPC_POLICIES, invokeWithPolicy } from "$lib/ipc/policy";

export interface ShellCommandRequest {
  command: string;
  workdir: string | null;
  timeout_seconds: number | null;
}

export interface ShellCommandResult {
  exit_code: number | null;
  stdout: string;
  stderr: string;
  timed_out: boolean;
  cwd: string | null;
}

export interface ShellCompletionRequest {
  input: string;
  workdir: string | null;
}

export interface ShellCompletionResult {
  replacement: string | null;
  suggestions: string[];
}

export async function runShellCommand(request: ShellCommandRequest): Promise<ShellCommandResult> {
  return invoke<ShellCommandResult>("run_shell_command", { request });
}

export async function completeShellInput(request: ShellCompletionRequest): Promise<ShellCompletionResult> {
  return invokeWithPolicy<ShellCompletionResult>("complete_shell_input", { request }, IPC_POLICIES.shellCompletion);
}

export async function openPtyTerminal(
  terminalId: string,
  workdir: string | null,
  onData: (data: number[]) => void,
): Promise<void> {
  const channel = new Channel<number[]>();
  channel.onmessage = onData;
  return invokeWithPolicy<void>("open_pty_terminal", { terminalId, workdir, onData: channel }, IPC_POLICIES.pty);
}

export async function closePtyTerminal(terminalId: string): Promise<void> {
  return invokeWithPolicy<void>("close_pty_terminal", { terminalId }, IPC_POLICIES.pty);
}

export async function writePtyTerminal(terminalId: string, data: number[]): Promise<void> {
  return invokeWithPolicy<void>("write_pty_terminal", { terminalId, data }, IPC_POLICIES.pty);
}

export async function resizePtyTerminal(terminalId: string, rows: number, cols: number): Promise<void> {
  return invokeWithPolicy<void>("resize_pty_terminal", { terminalId, rows, cols }, IPC_POLICIES.pty);
}

export async function ptyCurrentDirectory(terminalId: string): Promise<string | null> {
  return invokeWithPolicy<string | null>("pty_current_directory", { terminalId }, IPC_POLICIES.pty);
}
