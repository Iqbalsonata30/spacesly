import { applyTaskSessionEventPage, createTaskSessionReplay } from "../src/lib/taskSessionReplay";
import type { TaskSessionEvent, TaskSessionEventPage } from "../src/lib/ipc/taskSessions";

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `${message}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function event(
  sequence: number,
  kind: TaskSessionEvent["kind"],
  payload: unknown,
  progress: TaskSessionEvent["progress"] = null,
  sessionId = 7,
): TaskSessionEvent {
  return {
    id: sequence,
    session_id: sessionId,
    attempt_id: sequence > 1 ? 1 : null,
    fencing_token: sequence > 1 ? 1 : 0,
    sequence,
    kind,
    payload,
    progress,
    created_at: sequence,
  };
}

function page(events: TaskSessionEvent[], hasMore = false): TaskSessionEventPage {
  return {
    events,
    next_cursor: events.at(-1)?.sequence ?? 0,
    has_more: hasMore,
  };
}

const initial = createTaskSessionReplay(7);
const first = applyTaskSessionEventPage(
  initial,
  page([
    event(1, "lifecycle", { state: "queued" }, { phase: "queued", completed: 0, total: null }),
    event(
      2,
      "lifecycle",
      { state: "running" },
      {
        phase: "executing",
        completed: 0,
        total: null,
      },
    ),
  ]),
);
assertEqual(first.gapDetected, false, "ordered replay should not report a gap");
assertEqual(
  { cursor: first.replay.cursor, state: first.replay.state, phase: first.replay.progress?.phase },
  { cursor: 2, state: "running", phase: "executing" },
  "ordered replay should project lifecycle and progress",
);

const duplicate = applyTaskSessionEventPage(
  first.replay,
  page([
    event(2, "lifecycle", { state: "running" }),
    event(
      3,
      "progress",
      { message: "verified" },
      {
        phase: "verifying",
        completed: 1,
        total: 1,
      },
    ),
  ]),
);
assertEqual(
  { cursor: duplicate.replay.cursor, eventCount: duplicate.replay.events.length },
  { cursor: 3, eventCount: 3 },
  "duplicate events should be ignored without duplicating materialized history",
);

const gap = applyTaskSessionEventPage(
  duplicate.replay,
  page([event(5, "lifecycle", { state: "succeeded" })]),
);
assertEqual(
  { gap: gap.gapDetected, cursor: gap.replay.cursor },
  { gap: true, cursor: 3 },
  "a gap should leave the cursor at the last applied event",
);

let rejectedForeignSession = false;
try {
  applyTaskSessionEventPage(initial, page([event(1, "activity", {}, null, 8)]));
} catch {
  rejectedForeignSession = true;
}
assertEqual(rejectedForeignSession, true, "replay should reject cross-session event contamination");
