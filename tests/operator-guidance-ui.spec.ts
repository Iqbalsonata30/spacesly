import { expect, test, type Page } from "@playwright/test";

type GuidanceKind = "approve" | "continue" | "retry_fresh" | "supersede_mutation";

async function installGuidance(page: Page, kind: GuidanceKind) {
  await page.addInitScript((actionKind) => {
    const causes = {
      approve: ["approval_required", "scheduler_error", "Review and approve"],
      continue: ["execution_blocked", "scheduler_state", "Continue after resolving the block"],
      retry_fresh: ["retry_fresh_required", "scheduler_state", "Retry as a new task"],
      supersede_mutation: [
        "mutation_outcome_uncertain",
        "resource_mutation_ledger",
        "Review mutation fence",
      ],
    } as const;
    const [causeCode, source, label] = causes[actionKind];
    const key = "a".repeat(64);
    const testWindow = window as typeof window & {
      __TAURI_INTERNALS__: {
        invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
        transformCallback: () => number;
        convertFileSrc: (value: string) => string;
      };
    };
    testWindow.__TAURI_INTERNALS__ = {
      invoke: async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "get_task_session_operator_guidance") {
          return {
            schema_version: 1,
            session_id: 41,
            cause_code: causeCode,
            summary:
              actionKind === "supersede_mutation"
                ? "An external mutation may have reached its provider, so automatic replay is fenced."
                : "Backend-authoritative operator guidance.",
            source,
            action: {
              kind: actionKind,
              label,
              requires_confirmation: true,
              ...(actionKind === "supersede_mutation"
                ? { mutation: { mutation_id: 9, operation_key: key, revision: 2 } }
                : {}),
            },
          };
        }
        if (cmd === "list_task_session_resource_mutations") {
          return [
            {
              mutation_id: 9,
              operation_key: key,
              identity: {
                schema_version: 1,
                connector: "corporate-jira",
                operation: "jira_add_comment",
                resource: {
                  api_version: "jira/v1",
                  kind: "comment",
                  namespace: null,
                  name: "OPS-42/10001",
                },
                environment_fingerprint: "b".repeat(64),
                mutation_fingerprint: "c".repeat(64),
                key,
              },
              connector_id: "corporate-jira",
              tool_name: "jira_add_comment",
              state: "uncertain",
              session_id: 41,
              attempt_id: 2,
              attempt: 1,
              fencing_token: 3,
              evidence: null,
              failure_kind: "transport",
              failure_code: "lost_response",
              revision: 2,
              reserved_at: 1,
              resolved_at: 2,
              superseded_at: null,
              supersede_reason: null,
              checkpoint_objective_id: null,
              checkpoint_tool_call_id: null,
              checkpoint_recorded_at: null,
            },
          ];
        }
        if (cmd === "supersede_task_session_resource_mutation") {
          if (
            args?.sessionId !== 41 ||
            args?.mutationId !== 9 ||
            args?.expectedKey !== key ||
            args?.expectedRevision !== 2 ||
            args?.reason !== "Verified Jira did not create the comment"
          ) {
            throw new Error("mock: supersede arguments did not match the selected fence");
          }
          return {
            mutation_id: 9,
            operation_key: key,
            identity: {
              schema_version: 1,
              connector: "corporate-jira",
              operation: "jira_add_comment",
              resource: {
                api_version: "jira/v1",
                kind: "comment",
                namespace: null,
                name: "OPS-42/10001",
              },
              environment_fingerprint: "b".repeat(64),
              mutation_fingerprint: "c".repeat(64),
              key,
            },
            connector_id: "corporate-jira",
            tool_name: "jira_add_comment",
            state: "superseded",
            session_id: 41,
            attempt_id: 2,
            attempt: 1,
            fencing_token: 3,
            evidence: null,
            failure_kind: "transport",
            failure_code: "lost_response",
            revision: 3,
            reserved_at: 1,
            resolved_at: 2,
            superseded_at: 3,
            supersede_reason: args.reason,
            checkpoint_objective_id: null,
            checkpoint_tool_call_id: null,
            checkpoint_recorded_at: null,
          };
        }
        throw new Error(`mock: unhandled command ${cmd}`);
      },
      transformCallback: () => 1,
      convertFileSrc: (value: string) => value,
    };
  }, kind);
}

test("approval guidance performs the structured approval action", async ({ page }) => {
  await installGuidance(page, "approve");
  await page.goto("/__operator-guidance?approval=1");

  const approvalRegion = page.getByRole("region", { name: "Action approval required" });
  await expect(approvalRegion.locator(".guidance-source")).toContainText(
    "Cause: approval required",
  );
  await expect(page.getByRole("button", { name: "Review and approve" })).toHaveCount(0);
  await approvalRegion.getByRole("button", { name: "Approve & Continue" }).click();
  await expect(page.locator("#approval-clicks")).toHaveText("1");
  await expect(page.locator("#task-status")).toHaveText("running");
});

for (const scenario of [
  ["continue", "Continue after resolving the block"],
  ["retry_fresh", "Retry as a new task"],
] as const) {
  test(`${scenario[0]} guidance performs the guarded task action`, async ({ page }) => {
    await installGuidance(page, scenario[0]);
    await page.goto("/__operator-guidance");

    await page.getByRole("button", { name: scenario[1] }).click();
    const counter = scenario[0] === "continue" ? "#continue-clicks" : "#retry-fresh-clicks";
    await expect(page.locator(counter)).toHaveText("1");
    await expect(page.locator("#open-clicks")).toHaveText("0");
    await expect(page.getByRole("button", { name: "Mark done manually" })).toHaveCount(0);
  });
}

test("uncertain mutation guidance opens the exact retained fence", async ({ page }) => {
  await installGuidance(page, "supersede_mutation");
  await page.goto("/__operator-guidance");

  await expect(page.getByText(/Recovery stopped before reassignment/)).toBeVisible();
  await expect(page.getByText(/No replacement Worker was allowed/)).toBeVisible();
  await expect(page.getByRole("button", { name: /Continue/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Retry as a new task" })).toHaveCount(0);
  await page.getByRole("button", { name: "Review mutation fence" }).click();
  const recommended = page.locator(".mutation-record.recommended");
  await expect(recommended.getByText("#9 · jira add comment")).toBeVisible();
  await expect(page.getByText("corporate-jira", { exact: true })).toBeVisible();
  await expect(page.locator(".mutation-disclosure .context-notice")).toContainText(
    "records supported external mutations with deterministic operation identity",
  );
  await recommended
    .getByRole("textbox", { name: "Reason for releasing this fence" })
    .fill("Verified Jira did not create the comment");
  await recommended.getByRole("button", { name: "Release fence" }).click();
  await expect(recommended.getByText("superseded", { exact: true })).toBeVisible();
  await expect(
    page.getByText("Mutation fence #9 was released. No task was retried."),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Mark done manually" })).toHaveCount(0);
});
