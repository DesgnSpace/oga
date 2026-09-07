import { describe, expect, it } from "bun:test";
import type { Task, TaskAttempt, TaskEventView } from "@/bridge/types";
import { buildTranscript, REQUEST_ID } from "./transcriptModel";

/**
 * Turn ids below mirror what the broker's database actually stores: real
 * rows (task with profile "claude", ids 700536-700543) carry a `turn_id`
 * column that increments once per turn — 1182 for one turn's worth of
 * assistant/tool/result rows, 1183 for the next. `TaskEventView.turnId` is
 * that column, not something inferred client-side.
 */
function event(id: number, turnId: number | undefined, title = "Ran command"): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.tool_use",
    kind: "command",
    phase: "completed",
    title,
    detail: `step ${id}`,
    createdAt: `2026-07-30T15:00:${String(id % 60).padStart(2, "0")}Z`,
    turnId,
  };
}

function messageEvent(id: number, text: string): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.assistant",
    kind: "message",
    phase: "info",
    title: "Agent message",
    detail: text,
    presentation: { type: "message", text },
    createdAt: `2026-07-30T15:00:${String(id % 60).padStart(2, "0")}Z`,
  };
}

function usageEvent(id: number): TaskEventView {
  return {
    ...event(id, undefined, "Run summary"),
    type: "agent.result",
    kind: "usage",
    phase: "completed",
  };
}

function task(overrides: Partial<Task> = {}): Task {
  return {
    id: "task",
    profileId: "claude",
    model: "sonnet",
    prompt: "Do the thing",
    cwd: "/repo",
    state: "completed",
    createdAt: "2026-07-30T15:00:00Z",
    updatedAt: "2026-07-30T15:05:00Z",
    output: "",
    scope: { read: [], write: [] },
    allowQuestions: true,
  canDelegate: false,
    ...overrides,
  };
}

describe("buildTranscript work segments", () => {
  it("breaks a settled run's activity into one segment per turn", () => {
    const events: TaskEventView[] = [
      event(1, 100),
      event(2, 100),
      event(3, 200),
      event(4, 200),
      event(5, 200),
    ];

    const items = buildTranscript(task(), events);
    const workItems = items.filter((item) => item.type === "work");
    expect(workItems.length).toBe(2);
    if (workItems[0]?.type !== "work" || workItems[1]?.type !== "work") throw new Error("expected work items");
    // First turn's window is id 1's timestamp through id 2's.
    expect(workItems[0].segment.id).toBe(1);
    expect(workItems[0].segment.durationMs).toBe(1_000);
    expect(workItems[0].segment.startsExpanded).toBe(false);
    // Second turn starts fresh at id 3 rather than carrying the first
    // turn's span forward.
    expect(workItems[1].segment.id).toBe(3);
    expect(workItems[1].segment.durationMs).toBe(2_000);
    expect(workItems[1].segment.startsExpanded).toBe(true);
  });

  it("keeps one segment when no event carries a turn id", () => {
    const events: TaskEventView[] = [
      event(1, undefined),
      event(2, undefined),
      event(3, undefined),
    ];

    const items = buildTranscript(task(), events);
    const workItems = items.filter((item) => item.type === "work");
    expect(workItems.length).toBe(1);
    if (workItems[0]?.type !== "work") throw new Error("expected a work item");
    expect(workItems[0].segment.durationMs).toBe(2_000);
  });

  it("keeps one segment when every event shares the same turn id", () => {
    const events: TaskEventView[] = [event(1, 100), event(2, 100), event(3, 100)];

    const items = buildTranscript(task(), events);
    const workItems = items.filter((item) => item.type === "work");
    expect(workItems.length).toBe(1);
  });

  it("keeps only the run's last turn segment live while earlier turns read as settled", () => {
    const events: TaskEventView[] = [event(1, 100), event(2, 200)];
    const items = buildTranscript(task({ state: "running" }), events);
    const workItems = items.filter((item) => item.type === "work");
    expect(workItems.length).toBe(2);
    if (workItems[0]?.type !== "work" || workItems[1]?.type !== "work") throw new Error("expected work items");
    expect(workItems[0].segment.live).toBe(false);
    expect(workItems[0].segment.startsExpanded).toBe(false);
    expect(workItems[1].segment.live).toBe(true);
    expect(workItems[1].segment.startsExpanded).toBe(true);
  });
});

describe("buildTranscript across resumed attempts", () => {
  function attempt(overrides: Partial<TaskAttempt> = {}): TaskAttempt {
    return { output: "First reply.", endedAt: "2026-07-30T15:00:03Z", ...overrides };
  }

  it("orders a resumed task's activity and reply per attempt, oldest first", () => {
    // Attempt 1 ran events 1-2 and ended between :02 and :03; attempt 2 (the
    // current run) picks up with events 3-4 after that.
    const events: TaskEventView[] = [event(1, undefined), event(2, undefined), event(3, undefined), event(4, undefined)];
    const built = task({
      attempts: [attempt({ output: "First reply.", endedAt: "2026-07-30T15:00:02.500Z" })],
      output: "Second reply, settled.",
    });

    const items = buildTranscript(built, events);

    expect(items.map((item) => item.type)).toEqual(["bubble", "work", "response", "work", "response"]);
    const [, work1, response1, work2, response2] = items;
    if (work1?.type !== "work" || work2?.type !== "work") throw new Error("expected work items");
    if (response1?.type !== "response" || response2?.type !== "response") throw new Error("expected response items");
    // First attempt's own events (1-2) sit under its own reply, keyed off event 1.
    expect(work1.segment.id).toBe(1);
    expect(response1.block.text).toBe("First reply.");
    // The second attempt's events (3-4) and its reply come after, not before.
    expect(work2.segment.id).toBe(3);
    expect(response2.block.text).toBe("Second reply, settled.");
  });

  it("shows a resumed attempt's reply once when the resume shares its end time", () => {
    // The resume closes attempt 1, so the "Resumed" event carries the same
    // timestamp as the attempt's end; it opens attempt 2 rather than
    // closing attempt 1 a second time.
    const events: TaskEventView[] = [
      messageEvent(1, "First reply."),
      { ...event(2, undefined, "Resumed"), createdAt: "2026-07-30T15:00:03Z" },
      event(4, undefined),
    ];
    const built = task({
      attempts: [attempt({ output: "First reply.", endedAt: "2026-07-30T15:00:03Z" })],
      output: "Second reply, settled.",
    });

    const items = buildTranscript(built, events);

    expect(items.map((item) => item.type)).toEqual(["bubble", "work", "response", "bubble", "work", "response"]);
    expect(items.filter((item) => item.type === "response" && item.block.text === "First reply.").length).toBe(1);
  });

  it("leaves a single-run task unchanged when there are no past attempts", () => {
    const events: TaskEventView[] = [event(1, undefined), event(2, undefined)];
    const items = buildTranscript(task({ output: "Only reply." }), events);

    expect(items.map((item) => item.type)).toEqual(["bubble", "work", "response"]);
    const [bubble, work, response] = items;
    if (bubble?.type !== "bubble" || work?.type !== "work" || response?.type !== "response") {
      throw new Error("expected bubble, work, response");
    }
    expect(bubble.bubble.id).toBe(REQUEST_ID);
    expect(work.segment.id).toBe(1);
    expect(response.block.text).toBe("Only reply.");
  });
});

describe("buildTranscript while running", () => {
  it("keeps interim messages in work and hides the run receipt", () => {
    const items = buildTranscript(task({ state: "running" }), [messageEvent(1, "Checking the project..."), usageEvent(2)]);

    expect(items.map((item) => item.type)).toEqual(["bubble", "work"]);
    const work = items[1];
    if (work?.type !== "work") throw new Error("expected work item");
    expect(work.segment.composition.blocks.some((block) => block.type === "receipt")).toBe(false);
  });

  it("starts a new user turn for resumed instructions", () => {
    const resumed: TaskEventView = {
      ...event(2, undefined, "Resumed"),
      detail: "make the pr",
      rawText: JSON.stringify({ instruction: "make the pr" }),
    };
    const items = buildTranscript(task({ state: "running" }), [event(1, 10), resumed, event(3, 11)]);

    expect(items.map((item) => item.type)).toEqual(["bubble", "work", "bubble", "work"]);
    const instruction = items[2];
    if (instruction?.type !== "bubble") throw new Error("expected instruction bubble");
    expect(instruction.bubble.text).toBe("make the pr");
    expect(instruction.bubble.kind).toBe("resume");
    expect(instruction.bubble.rawText).toBe(JSON.stringify({ instruction: "make the pr" }));
  });
});
