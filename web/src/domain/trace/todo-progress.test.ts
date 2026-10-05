// The list the tray above the composer shows: the newest one the run wrote,
// how far it has got, and when there is nothing left worth showing.

import { describe, expect, it } from "bun:test";
import type { TaskEventView } from "@/bridge/types";
import { latestTodoProgress } from "./index";

function todoEvent(id: number, items: { content: string; status: string }[]): TaskEventView {
  const done = items.filter((item) => item.status === "completed").length;
  return {
    id,
    taskId: "task",
    source: "opencode",
    type: "agent.tool_call",
    kind: "tool",
    phase: "started",
    title: "Todo list",
    presentation: { type: "todo", completed: done, total: items.length, text: `${items.length} steps` },
    rawText: JSON.stringify({ tool: "todowrite", input: { todos: items } }),
    createdAt: `2026-07-30T15:00:${String(id % 60).padStart(2, "0")}Z`,
    turnId: 1,
  };
}

function call(id: number): TaskEventView {
  return {
    ...todoEvent(id, []),
    presentation: { type: "tool", text: "Bash" },
    rawText: undefined,
    title: "Bash",
  };
}

describe("latestTodoProgress", () => {
  it("counts the steps done and names the one in hand", () => {
    const progress = latestTodoProgress(
      [
        todoEvent(1, [
          { content: "Read the files", status: "completed" },
          { content: "Fix the palette", status: "in_progress" },
          { content: "Run the tests", status: "pending" },
        ]),
      ],
      true,
    );

    expect(progress).toEqual({
      items: [
        { text: "Read the files", status: "completed" },
        { text: "Fix the palette", status: "in_progress" },
        { text: "Run the tests", status: "pending" },
      ],
      done: 1,
      total: 3,
      current: "Fix the palette",
    });
  });

  it("takes the newer list once the run rewrites it", () => {
    const progress = latestTodoProgress(
      [
        todoEvent(1, [{ content: "Survey the crates", status: "in_progress" }]),
        call(2),
        todoEvent(3, [
          { content: "Survey the crates", status: "completed" },
          { content: "Write the report", status: "in_progress" },
        ]),
      ],
      true,
    );

    expect(progress?.total).toBe(2);
    expect(progress?.done).toBe(1);
    expect(progress?.current).toBe("Write the report");
  });

  it("has nothing to show until the run writes a list", () => {
    expect(latestTodoProgress([call(1), call(2)], true)).toBeUndefined();
  });

  it("leaves once every step is done and the run has settled", () => {
    const events = [todoEvent(1, [{ content: "Read the files", status: "completed" }])];

    expect(latestTodoProgress(events, true)).toBeDefined();
    expect(latestTodoProgress(events, false)).toBeUndefined();
  });

  it("keeps a half-done list on screen after the run settles", () => {
    const progress = latestTodoProgress(
      [
        todoEvent(1, [
          { content: "Read the files", status: "completed" },
          { content: "Run the tests", status: "pending" },
        ]),
      ],
      false,
    );

    expect(progress?.done).toBe(1);
    expect(progress?.current).toBeUndefined();
  });

  it("ignores a list whose payload carries no steps", () => {
    const broken: TaskEventView = { ...todoEvent(2, []), rawText: JSON.stringify({ input: { todos: [] } }) };

    expect(latestTodoProgress([todoEvent(1, [{ content: "Survey", status: "in_progress" }]), broken], true)?.total).toBe(1);
  });
});