// Rows a worker streams itself, as the activity story reads them.

import { describe, expect, it } from "bun:test";
import type { TaskEventView } from "@/bridge/types";
import { ActivityStory, chapterRowEvents, withoutDuplicateHookCalls } from "./index";

function agentCall(id: number, callId: string, complete: boolean, turnId = 1): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: complete ? "agent.tool_call_update" : "agent.tool_call",
    kind: "file",
    phase: complete ? "completed" : "started",
    title: "Edit file",
    detail: "src/main.rs",
    verb: "Edited",
    target: "src/main.rs",
    presentation: { type: "file", path: "src/main.rs" },
    createdAt: `2026-09-17T10:00:0${id}Z`,
    turnId,
    actionId: callId,
    sourceId: callId,
    complete,
  };
}

function callEnded(id: number, callId: string, outcome: string, turnId = 1): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.tool_call_update",
    kind: "tool",
    phase: "completed",
    title: "Run command",
    verb: "Ran",
    result: outcome,
    presentation: { type: "tool", outcome },
    createdAt: `2026-09-17T10:00:0${id}Z`,
    turnId,
    actionId: callId,
    sourceId: callId,
    complete: true,
  };
}

function runningCall(id: number, callId: string, turnId = 1): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.tool_call",
    kind: "command",
    phase: "started",
    title: "Run command",
    detail: "cargo test",
    verb: "Running",
    target: "cargo test",
    presentation: { type: "command", command: "cargo test", text: "cargo test" },
    createdAt: `2026-09-17T10:00:0${id}Z`,
    turnId,
    actionId: callId,
    sourceId: callId,
    complete: false,
  };
}

function hookCall(id: number, callId: string, turnId = 1): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.hook",
    kind: "file",
    phase: "completed",
    title: "Edit file",
    detail: "src/main.rs",
    verb: "Edited",
    target: "src/main.rs",
    presentation: { type: "file", path: "src/main.rs" },
    createdAt: `2026-09-17T10:00:0${id}Z`,
    turnId,
    actionId: callId,
    sourceId: callId,
    complete: true,
  };
}

function sessionStartHook(id: number, turnId = 1): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.hook",
    kind: "lifecycle",
    phase: "info",
    title: "SessionStart",
    detail: "startup",
    createdAt: `2026-09-17T10:00:0${id}Z`,
    turnId,
  };
}

describe("a turn the worker narrates itself", () => {
  it("keeps one row per call instead of the hook copy beside it", () => {
    const events = [agentCall(1, "call_1", false), hookCall(2, "toolu_1"), agentCall(3, "call_1", true)];

    const kept = withoutDuplicateHookCalls(events);

    expect(kept.map((event) => event.id)).toEqual([1, 3]);
  });

  it("keeps the hooks that are not about a call", () => {
    const events = [sessionStartHook(1), agentCall(2, "call_1", true)];

    expect(withoutDuplicateHookCalls(events).map((event) => event.id)).toEqual([1, 2]);
  });

  it("leaves a command-line turn's hooks alone", () => {
    const events = [sessionStartHook(1), hookCall(2, "toolu_1")];

    expect(withoutDuplicateHookCalls(events)).toEqual(events);
  });

  it("only drops hooks from the turns the worker narrated", () => {
    const events = [hookCall(1, "toolu_1", 1), agentCall(2, "call_1", true, 2), hookCall(3, "toolu_2", 2)];

    expect(withoutDuplicateHookCalls(events).map((event) => event.id)).toEqual([1, 2]);
  });

  it("settles an open call into one row when its update arrives", () => {
    const composition = ActivityStory.compose([agentCall(1, "call_1", false), agentCall(2, "call_1", true)]);
    const rows = composition.blocks.flatMap((block) => (block.type === "chapter" ? block.rows : []));

    expect(rows).toHaveLength(1);
    expect(rows.flatMap(chapterRowEvents).map((event) => event.phase)).toEqual(["completed"]);
  });

  it("folds a replayed call back into the row it already has", () => {
    const replayed = { ...agentCall(3, "call_1", true), id: 3 };
    const composition = ActivityStory.compose([
      agentCall(1, "call_1", false),
      agentCall(2, "call_1", true),
      replayed,
    ]);
    const rows = composition.blocks.flatMap((block) => (block.type === "chapter" ? block.rows : []));

    expect(rows).toHaveLength(1);
  });
  it("settles a call that reported only how it ended, keeping its subject", () => {
    const composition = ActivityStory.compose([
      runningCall(1, "call_2"),
      callEnded(2, "call_2", "2 tests passed"),
    ]);
    const rows = composition.blocks.flatMap((block) => (block.type === "chapter" ? block.rows : []));
    const events = rows.flatMap(chapterRowEvents);

    expect(events).toHaveLength(1);
    expect(events[0]?.phase).toBe("completed");
    expect(events[0]?.complete).toBe(true);
    expect(events[0]?.verb).toBe("Ran");
    expect(events[0]?.presentation?.command).toBe("cargo test");
    expect(events[0]?.presentation?.outcome).toBe("2 tests passed");
  });
});
