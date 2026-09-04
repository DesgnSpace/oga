import { describe, expect, it } from "bun:test";
import type { TaskDiff, TaskEventView } from "@/bridge/types";
import { collectRunChangesByTurn, gitChangeSet } from "./grouped";

function edit(id: number, turnId: number | undefined, path: string, oldText: string, newText: string): TaskEventView {
  const view: TaskEventView = {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.tool",
    kind: "file",
    phase: "completed",
    title: "Edit",
    rawText: JSON.stringify({
      tool_name: "Edit",
      tool_input: { file_path: path, old_string: oldText, new_string: newText },
    }),
    createdAt: "2026-08-07T00:00:00Z",
  };
  if (turnId !== undefined) view.turnId = turnId;
  return view;
}

function broker(id: number, type: string): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "broker",
    type,
    kind: "lifecycle",
    phase: "info",
    title: type,
    createdAt: "2026-08-07T00:00:00Z",
  };
}

describe("collectRunChangesByTurn", () => {
  it("puts each turn's files under it, newest turn first", () => {
    const grouped = collectRunChangesByTurn(
      [
        edit(1, 7, "/repo/first.ts", "a", "b"),
        broker(2, "follow_up_started"),
        edit(3, 8, "/repo/second.ts", "c", "d"),
      ],
      "/repo",
    );

    expect(grouped.turns.map((turn) => turn.label)).toEqual(["Your follow-up", "First run"]);
    expect(grouped.turns.map((turn) => turn.ordinal)).toEqual([2, 1]);
    expect(grouped.turns[0].files.map((file) => file.path)).toEqual(["second.ts"]);
    expect(grouped.turns[1].files.map((file) => file.path)).toEqual(["first.ts"]);
  });

  it("names a turn after what the reader sent to start it", () => {
    const grouped = collectRunChangesByTurn(
      [
        edit(1, 7, "/repo/first.ts", "a", "b"),
        broker(2, "answered"),
        edit(3, 8, "/repo/second.ts", "c", "d"),
        broker(4, "steered"),
        edit(5, 9, "/repo/third.ts", "e", "f"),
      ],
      "/repo",
    );

    expect(grouped.turns.map((turn) => turn.label)).toEqual([
      "Your instruction",
      "Your answer",
      "First run",
    ]);
  });

  it("keeps an event with no turn under the turn still open around it", () => {
    const grouped = collectRunChangesByTurn(
      [edit(1, 7, "/repo/first.ts", "a", "b"), edit(2, undefined, "/repo/second.ts", "c", "d")],
      "/repo",
    );

    expect(grouped.turns.length).toBe(1);
    expect(grouped.turns[0].files.map((file) => file.path)).toEqual(["first.ts", "second.ts"]);
  });

  it("drops turns that changed nothing", () => {
    const grouped = collectRunChangesByTurn(
      [broker(1, "dispatched"), edit(2, 7, "/repo/first.ts", "a", "b")],
      "/repo",
    );

    expect(grouped.turns.map((turn) => turn.label)).toEqual(["First run"]);
  });
});

describe("gitChangeSet", () => {
  it("reads each file's hunks out of its patch", () => {
    const diff: TaskDiff = {
      basis: "branch",
      files: [
        {
          path: "app.ts",
          status: "modified",
          added: 1,
          removed: 1,
          patch: "@@ -1,3 +1,3 @@\n one\n-two\n+three\n four",
          tooLarge: false,
        },
        { path: "huge.bin", status: "added", added: 0, removed: 0, tooLarge: true },
      ],
      truncated: false,
    };

    const set = gitChangeSet(diff);

    expect(set.files[0].change.blocks[0].map((line) => line.kind)).toEqual([
      "context",
      "removed",
      "added",
      "context",
    ]);
    expect(set.files[0].status).toBe("modified");
    expect(set.files[1].change.blocks).toEqual([]);
    expect(set.files[1].tooLarge).toBe(true);
    expect(set.unmatched).toBe(0);
  });
});
