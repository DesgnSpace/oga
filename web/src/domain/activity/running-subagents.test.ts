// The tray above the composer lists the subagents still running: each one
// shows while it runs and leaves once its report arrives.

import { describe, expect, it } from "bun:test";
import type { TaskEventView } from "@/bridge/types";
import { ActivityStory, compositionCalls, runningSubagents } from "./index";

async function fixture(driver: string): Promise<TaskEventView[]> {
  // SAFETY: the fixtures are `event_views` output, serialized from `TaskEventView`.
  return (await Bun.file(new URL(`./fixtures/subagents/${driver}.json`, import.meta.url)).json()) as TaskEventView[];
}

function beforeFirstReport(events: TaskEventView[]): TaskEventView[] {
  const index = events.findIndex((event) => event.subagents?.some((link) => link.role === "report"));
  return index === -1 ? events : events.slice(0, index);
}

function toolCall(id: number): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.tool_call",
    kind: "tool",
    phase: "started",
    title: "Bash",
    detail: "bun test",
    createdAt: `2026-07-30T15:00:${String(id % 60).padStart(2, "0")}Z`,
    actionId: `toolu_${id}`,
    turnId: 1,
  };
}

describe("runningSubagents", () => {
  it("lists every subagent still running, with what each one is doing", async () => {
    const events = beforeFirstReport(await fixture("codex"));
    const running = runningSubagents(ActivityStory.composeWithState(events, false, undefined, false));

    expect(running).toHaveLength(3);
    expect(running.every((subagent) => subagent.status === "running")).toBe(true);
    expect(running.every((subagent) => subagent.label !== undefined && subagent.label !== "")).toBe(true);
    expect(new Set(running.map((subagent) => subagent.label)).size).toBe(3);
  });

  it("drops a subagent once its report arrives", async () => {
    const settled = ActivityStory.composeWithState(await fixture("codex"), true, undefined, true);

    expect(runningSubagents(settled)).toEqual([]);
  });

  it("never lists a plain tool call", () => {
    const composition = ActivityStory.composeWithState([toolCall(1), toolCall(2)], false, undefined, false);

    expect(compositionCalls(composition.blocks)).toHaveLength(2);
    expect(runningSubagents(composition)).toEqual([]);
  });

  it("stays empty when the provider reports no subagents", async () => {
    const settled = ActivityStory.composeWithState(await fixture("pi"), true, undefined, true);

    expect(runningSubagents(settled)).toEqual([]);
  });
});
