// A list should animate only for live changes: never on its first load or
// for an older page, and once per change.

import { describe, expect, it } from "bun:test";
import { renderHook, waitFor } from "@testing-library/react";
import type { TaskState, TaskSummary } from "@/bridge/types";
import { useTaskArrivals } from "./task-arrivals";

function task(id: string, state: TaskState, createdAt: string): TaskSummary {
  return {
    id,
    profileId: "worker",
    model: "sonnet",
    cwd: "/work/oga",
    state,
    promptPreview: `prompt ${id}`,
    title: `Task ${id}`,
    createdAt,
    updatedAt: createdAt,
  };
}

const first = [task("a", "running", "2026-11-24T10:00:00Z"), task("b", "completed", "2026-11-24T09:00:00Z")];

function arrivalsAcross(...lists: TaskSummary[][]) {
  const view = renderHook(({ tasks }) => useTaskArrivals(tasks), { initialProps: { tasks: lists[0] } });
  for (const tasks of lists.slice(1)) view.rerender({ tasks });
  return view;
}

describe("useTaskArrivals", () => {
  it("marks nothing on the first load, even after an empty start", () => {
    const { result } = arrivalsAcross([], first);
    expect([...result.current]).toEqual([]);
  });

  it("marks a task created after the last update", () => {
    const fresh = task("c", "queued", "2026-11-24T11:00:00Z");
    const { result } = arrivalsAcross(first, [fresh, ...first]);
    expect([...result.current]).toEqual(["c"]);
  });

  it("ignores an older page loaded later", () => {
    const older = task("z", "completed", "2026-11-20T10:00:00Z");
    const { result } = arrivalsAcross(first, [...first, older]);
    expect([...result.current]).toEqual([]);
  });

  it("marks a task that moved to another status, but not a step within one", () => {
    const needsYou = [task("a", "needs_input", "2026-11-24T10:00:00Z"), first[1]];
    const { result } = arrivalsAcross(first, needsYou);
    expect([...result.current]).toEqual(["a"]);

    const queued = [task("q", "queued", "2026-11-24T08:00:00Z")];
    const step = arrivalsAcross(queued, [task("q", "preparing_checkout", "2026-11-24T08:00:00Z")]);
    expect([...step.result.current]).toEqual([]);
  });

  it("clears the mark after the entrance, so a remounted row does not replay it", async () => {
    const fresh = task("c", "queued", "2026-11-24T11:00:00Z");
    const { result } = arrivalsAcross(first, [fresh, ...first]);
    expect(result.current.has("c")).toBe(true);
    await waitFor(() => expect(result.current.size).toBe(0));
  });
});
