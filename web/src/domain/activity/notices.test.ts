// The notices Oga adds to a run's story, read the way a person sees them.

import { describe, expect, it } from "bun:test";
import type { TaskEventView } from "@/bridge/types";
import { TraceRowBuilder } from "@/domain/trace";
import { ActivityStory } from "./index";

function notice(id: number, type: string, title: string, extra: Partial<TaskEventView> = {}): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "broker",
    type,
    kind: "lifecycle",
    phase: "info",
    title,
    createdAt: `2026-10-18T10:00:${String(id % 60).padStart(2, "0")}Z`,
    turnId: 1,
    ...extra,
  };
}

const removed = (id: number, count: number) =>
  notice(id, "history_dropped", "Older activity removed", {
    detail: `Older activity removed: ${count} update${count === 1 ? "" : "s"}`,
  });

function shown(events: TaskEventView[]): string[] {
  const composition = ActivityStory.compose(events);
  return TraceRowBuilder.rows(composition.blocks, "/repo", false).flatMap((row) =>
    row.target === undefined ? [] : [row.target],
  );
}

describe("removed history", () => {
  it("shows once per run, with every removed update counted", () => {
    const events = [2, 3, 1, 573].map((count, index) => removed(10 + index, count));

    expect(shown(events)).toEqual(["Older activity removed: 579 updates"]);
  });
});

describe("provider retries", () => {
  const waiting = (id: number, attempt: number) =>
    notice(id, "agent.session_info_update", "Rate limited", {
      source: "opencode-2",
      detail: `Rate limited · retrying, attempt ${attempt} · next try in 2s · Rate limit exceeded: free-models-per-day-stealth`,
      actionId: "model-response-recovery",
      sourceId: "model-response-recovery",
    });
  const recovered = (id: number) =>
    notice(id, "agent.session_info_update", "Retry succeeded", {
      source: "opencode-2",
      phase: "completed",
      detail: "Retry succeeded",
      actionId: "model-response-recovery",
      sourceId: "model-response-recovery",
    });

  it("reads as one row that ends recovered, not a row per attempt", () => {
    const rows = shown([waiting(1, 1), waiting(2, 2), recovered(3)]);

    expect(rows).toHaveLength(1);
    expect(rows[0]).toBe("Retry succeeded");
  });

  it("stays on the latest attempt while the provider is still failing", () => {
    const rows = shown([waiting(1, 1), waiting(2, 2)]);

    expect(rows).toHaveLength(1);
    expect(rows[0]).toContain("attempt 2");
  });
});
