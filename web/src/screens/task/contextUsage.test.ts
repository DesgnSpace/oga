import { describe, expect, test } from "bun:test";
import type { TaskEventView } from "@/bridge/types";
import { COMPACT_BOUNDARY_TITLE, contextUsage } from "./contextUsage";

function usageEvent(id: number, tokensIn: number, tokensCached = 0): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "broker",
    type: "agent.result",
    kind: "usage",
    phase: "completed",
    title: "Run summary",
    presentation: { type: "usage", tokensIn, tokensCached },
    createdAt: "2026-09-06T00:00:00Z",
  };
}

function boundaryEvent(id: number): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "broker",
    type: "agent.system",
    kind: "lifecycle",
    phase: "info",
    title: COMPACT_BOUNDARY_TITLE,
    createdAt: "2026-09-06T00:00:00Z",
  };
}

describe("contextUsage", () => {
  test("no window means no answer, never a zero", () => {
    expect(contextUsage([usageEvent(1, 1000)], undefined)).toBeUndefined();
  });

  test("no usage reads means no answer", () => {
    expect(contextUsage([], 200_000)).toBeUndefined();
  });

  test("fill is the freshest read against the window, with growth since the first", () => {
    const usage = contextUsage(
      [usageEvent(1, 10_000), usageEvent(2, 20_000)],
      200_000,
    );
    expect(usage).toEqual({
      used: 20_000,
      window: 200_000,
      percent: 10,
      delta: 10_000,
      deltaPercent: 5,
      compacted: false,
    });
  });

  test("cached tokens count toward the fill", () => {
    const usage = contextUsage([usageEvent(1, 10_000, 30_000)], 200_000);
    expect(usage?.used).toBe(40_000);
    expect(usage?.percent).toBe(20);
  });

  test("a compaction starts a new stretch instead of climbing forever", () => {
    const usage = contextUsage(
      [usageEvent(1, 150_000), boundaryEvent(2), usageEvent(3, 20_000)],
      200_000,
    );
    expect(usage).toEqual({
      used: 20_000,
      window: 200_000,
      percent: 10,
      delta: 0,
      deltaPercent: 0,
      compacted: true,
    });
  });
});
