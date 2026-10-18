// A worker streams a reply or a thought a piece at a time; the story reads each as one row.

import { describe, expect, it } from "bun:test";
import type { TaskEventView } from "@/bridge/types";
import { ActivityStory, type ActivityNode } from "./index";

function slice(
  id: number,
  type: string,
  kind: "message" | "reasoning",
  text: string,
  extra: Partial<TaskEventView> = {},
): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "opencode",
    type,
    kind,
    phase: "info",
    title: kind === "message" ? "Agent message" : "Thinking",
    detail: text,
    presentation: { type: "message", text },
    createdAt: `2026-10-18T10:00:${String(id % 60).padStart(2, "0")}Z`,
    turnId: 1,
    ...extra,
  };
}

const said = (id: number, text: string) => slice(id, "agent.agent_message_chunk", "message", text);
const thought = (id: number, text: string) => slice(id, "agent.agent_thought_chunk", "reasoning", text);

function nodes(events: TaskEventView[]): ActivityNode[] {
  const composition = ActivityStory.compose(events);
  return composition.blocks.flatMap((block) =>
    block.type === "turn" ? block.turn.segments.flatMap((segment) => segment.nodes) : [],
  );
}

function messages(events: TaskEventView[]): string[] {
  return nodes(events).flatMap((node) => (node.type === "message" ? [node.event.detail ?? ""] : []));
}

function thoughts(events: TaskEventView[]): (string | undefined)[] {
  return nodes(events).flatMap((node) => (node.type === "thinking" ? [node.pulse.text] : []));
}

describe("a streamed reply", () => {
  it("reads as one row, joined exactly as sent", () => {
    const events = [
      said(519717, "I'll start by checking the current state of"),
      said(519721, " the worktree."),
      said(519722, " Then I'll read the"),
      said(519723, " tests.\n\nAfter that, I'll fix them."),
    ];
    expect(messages(events)).toEqual([
      "I'll start by checking the current state of the worktree. Then I'll read the tests.\n\nAfter that, I'll fix them.",
    ]);
  });

  it("is still one row, with its text so far, while the message grows", () => {
    const events = [said(1, "I'll start by checking the"), said(2, " worktree")];
    expect(messages(events)).toEqual(["I'll start by checking the worktree"]);
    const nodeId = (list: TaskEventView[]) =>
      nodes(list).flatMap((node) => (node.type === "message" ? [node.id] : []));
    expect(nodeId([...events, said(3, " and the tests.")])).toEqual(nodeId(events));
    expect(messages([...events, said(3, " and the tests.")])).toEqual([
      "I'll start by checking the worktree and the tests.",
    ]);
  });

  it("splits into two rows when work happens between the pieces", () => {
    const events = [
      said(1, "Reading the file"),
      thought(2, "Check the exports first."),
      said(3, " now."),
    ];
    expect(messages(events)).toEqual(["Reading the file", " now."]);
  });
});

describe("a streamed thought", () => {
  it("is one block when the worker sends a stray line break between its pieces", () => {
    const events = [
      thought(1, "I should decline"),
      said(2, "\n\n"),
      thought(3, " the trailer and explain"),
      said(4, "\n\n"),
      thought(5, ", but do the rest."),
    ];
    expect(thoughts(events)).toEqual(["I should decline the trailer and explain, but do the rest."]);
    expect(messages(events)).toEqual([]);
  });
});

describe("a Pi reply", () => {
  const delta = (id: number, kind: "message" | "reasoning", index: number, text: string) =>
    slice(id, "agent.message_update", kind, text, { source: "pi", contentIndex: index });
  const bracket = (id: number, type: string): TaskEventView => ({
    ...slice(id, type, "message", ""),
    source: "pi",
    detail: undefined,
    presentation: undefined,
    minor: true,
  });
  const streamed = [
    bracket(1, "agent.message_start"),
    bracket(2, "agent.message_update"),
    delta(3, "reasoning", 0, "Let me start"),
    delta(4, "reasoning", 0, " by understanding"),
    bracket(5, "agent.message_update"),
    delta(6, "message", 1, "I"),
    delta(7, "message", 1, "'ll look at the docs."),
  ];

  it("shows the thinking and text so far while it streams", () => {
    expect(thoughts(streamed)).toEqual(["Let me start by understanding"]);
    expect(messages(streamed)).toEqual(["I'll look at the docs."]);
  });

  it("swaps the streamed text for the final text when the message ends", () => {
    const end = slice(8, "agent.message_end", "message", "I'll look at the docs folder.", {
      source: "pi",
      phase: "completed",
    });
    const events = [...streamed, end];
    expect(messages(events)).toEqual(["I'll look at the docs folder."]);
    expect(thoughts(events)).toEqual(["Let me start by understanding"]);
  });

  it("swaps streamed thinking for the final thinking when a tool-only message ends", () => {
    const end = slice(8, "agent.message_end", "reasoning", "Let me start by understanding the task.", {
      source: "pi",
      phase: "completed",
    });
    expect(thoughts([...streamed.slice(0, 5), end])).toEqual(["Let me start by understanding the task."]);
  });

  it("keeps separate blocks apart when one text block follows another", () => {
    const events = [delta(1, "message", 1, "First."), delta(2, "message", 3, "Second.")];
    expect(messages(events)).toEqual(["First.", "Second."]);
  });
});
