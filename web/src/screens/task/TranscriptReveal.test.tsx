// Revealing a running subagent from the composer tray: its collapsed groups
// open and its own row highlights, while its neighbor stays quiet.

import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import type { TaskEventView } from "@/bridge/types";
import { ActivityStory } from "@/domain/activity";
import { Transcript } from "./Transcript";
import type { TranscriptItem } from "./transcriptModel";

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

function items(): TranscriptItem[] {
  const composition = ActivityStory.composeWithState(
    [launch(1, "task-a", "Alpha"), launch(2, "task-b", "Beta")],
    false,
    undefined,
    false,
  );
  return [{
    type: "work",
    segment: { id: 1, composition, cwd: "/", live: true, startsExpanded: false },
  }];
}

function flashed(text: string): boolean {
  return screen.getByText(text).closest("article")?.className.includes("trace-row-flash") ?? false;
}

describe("revealing a running subagent", () => {
  afterEach(cleanup);

  it("opens its collapsed groups and highlights its own row, not its neighbor", async () => {
    render(<Transcript items={items()} reveal={{ nodeId: "subagent:task-b", nonce: 1 }} />);

    await waitFor(() => expect(flashed("Subagent · Beta")).toBe(true));
    expect(flashed("Subagent · Alpha")).toBe(false);
  });
});
