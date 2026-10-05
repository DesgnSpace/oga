// Finding the row a composer-tray reveal asks for, wherever it sits.

import { describe, expect, it } from "bun:test";
import type { TaskEventView } from "@/bridge/types";
import { ActivityStory, compositionContainsNode } from "@/domain/activity";
import { findRevealTarget, type TraceRow } from "./index";

function row(id: number, nodeId?: string, children: TraceRow[] = []): TraceRow {
  return {
    id,
    nodeId,
    style: "work",
    state: "done",
    children,
    startsExpanded: false,
    isStepStart: false,
    isTechnical: false,
  };
}

function launch(id: number, subagentId: string, label: string): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "codex",
    type: "agent.item.started",
    kind: "tool",
    phase: "started",
    title: "Subagent",
    detail: label,
    createdAt: `2026-07-30T15:00:${String(id % 60).padStart(2, "0")}Z`,
    subagents: [{ id: subagentId, role: "launch", label }],
    turnId: 1,
  };
}

describe("findRevealTarget", () => {
  it("lands on a top-level row with nothing to open", () => {
    expect(findRevealTarget([row(1, "call:0:first"), row(2, "call:0:second")], "call:0:second")).toEqual({
      index: 1,
      ancestors: [],
    });
  });

  it("opens each row down to a nested child, and picks the right sibling", () => {
    const rows = [
      row(1, "subagents:batch", [row(2, "subagent:one"), row(3, "subagent:two")]),
      row(4, "call:0:other"),
    ];

    expect(findRevealTarget(rows, "subagent:two")).toEqual({ index: 0, ancestors: ["subagents:batch"] });
    expect(findRevealTarget(rows, "subagent:one")).toEqual({ index: 0, ancestors: ["subagents:batch"] });
    expect(findRevealTarget(rows, "call:0:other")).toEqual({ index: 1, ancestors: [] });
  });

  it("misses a row that is not there", () => {
    expect(findRevealTarget([row(1, "call:0:first")], "subagent:one")).toBeUndefined();
  });
});

describe("compositionContainsNode", () => {
  function composition() {
    return ActivityStory.composeWithState(
      [launch(1, "task-a", "Alpha"), launch(2, "task-b", "Beta")],
      false,
      undefined,
      false,
    );
  }

  it("finds each running subagent and the batch holding them", () => {
    expect(compositionContainsNode(composition(), "subagent:task-a")).toBe(true);
    expect(compositionContainsNode(composition(), "subagent:task-b")).toBe(true);
  });

  it("rejects a node that never ran", () => {
    expect(compositionContainsNode(composition(), "subagent:task-c")).toBe(false);
    expect(compositionContainsNode(composition(), "call:1:toolu_9")).toBe(false);
  });
});
