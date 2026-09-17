// Rows a worker streams itself, as the activity story reads them.

import { describe, expect, it } from "bun:test";
import type { TaskEventView } from "@/bridge/types";
import { ActivityStory, compositionCalls, normalizeAntigravityEvents, withoutDuplicateHookCalls } from "./index";
import { TraceVisibility, expansionFromEvent, type TraceRow } from "@/domain/trace";

/** The settled row of every call a composition holds, in the order they opened. */
function callRows(events: TaskEventView[]): TaskEventView[] {
  return compositionCalls(ActivityStory.compose(events).blocks).map((call) => call.event);
}

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

  it("drops a hook posted without a turn when the worker reported that same call", () => {
    const events = [
      agentCall(1, "toolu_1", false, 4),
      { ...hookCall(2, "toolu_1"), turnId: undefined },
      { ...hookCall(3, "toolu_9"), turnId: undefined },
      agentCall(4, "toolu_1", true, 4),
    ];

    expect(withoutDuplicateHookCalls(events).map((event) => event.id)).toEqual([1, 3, 4]);
  });

  it("keeps a hook posted without a turn when only another turn has its call id", () => {
    const events = [agentCall(1, "toolu_1", true, 4), { ...hookCall(2, "toolu_2"), turnId: undefined }, agentCall(3, "toolu_2", true, 5)];

    expect(withoutDuplicateHookCalls(events).map((event) => event.id)).toEqual([1, 2, 3]);
  });

  it("only drops hooks from the turns the worker narrated", () => {
    const events = [hookCall(1, "toolu_1", 1), agentCall(2, "call_1", true, 2), hookCall(3, "toolu_2", 2)];

    expect(withoutDuplicateHookCalls(events).map((event) => event.id)).toEqual([1, 2]);
  });

  it("settles an open call into one row when its update arrives", () => {
    const rows = callRows([agentCall(1, "call_1", false), agentCall(2, "call_1", true)]);

    expect(rows.map((event) => event.phase)).toEqual(["completed"]);
  });

  it("folds a replayed call back into the row it already has", () => {
    const replayed = { ...agentCall(3, "call_1", true), id: 3 };
    expect(callRows([agentCall(1, "call_1", false), agentCall(2, "call_1", true), replayed])).toHaveLength(1);
  });
  it("patches a settled call from an update that names nothing of its own", () => {
    // A page boundary separates an update from its opening row, so it arrives
    // carrying only its own content and no status to settle on.
    const partial: TaskEventView = {
      ...agentCall(3, "call_1", false),
      type: "agent.tool_call_update",
      title: "Activity",
      verb: "Using",
      kind: "tool",
      detail: undefined,
      target: undefined,
      presentation: { type: "tool" },
      rawText: JSON.stringify({ sessionUpdate: "tool_call_update", toolCallId: "call_1", rawOutput: "done" }),
    };
    const events = callRows([agentCall(1, "call_1", false), agentCall(2, "call_1", true), partial]);

    expect(events).toHaveLength(1);
    expect(events[0]?.title).toBe("Edit file");
    expect(events[0]?.presentation?.path).toBe("src/main.rs");
    expect(events[0]?.phase).toBe("completed");
    expect(events[0]?.complete).toBe(true);
  });

  it("settles a call that reported only how it ended, keeping its subject", () => {
    const events = callRows([runningCall(1, "call_2"), callEnded(2, "call_2", "2 tests passed")]);

    expect(events).toHaveLength(1);
    expect(events[0]?.phase).toBe("completed");
    expect(events[0]?.complete).toBe(true);
    expect(events[0]?.verb).toBe("Ran");
    expect(events[0]?.presentation?.command).toBe("cargo test");
    expect(events[0]?.presentation?.outcome).toBe("2 tests passed");
  });
});

describe("a run that tidies up after it speaks", () => {
  function row(id: number, overrides: Partial<TaskEventView>): TaskEventView {
    return {
      id,
      taskId: "task",
      source: "claude",
      type: "agent.agent_message_chunk",
      kind: "message",
      phase: "info",
      title: "Agent message",
      createdAt: `2026-09-17T15:28:${String(id).padStart(2, "0")}Z`,
      turnId: 1,
      ...overrides,
    };
  }

  const answer = row(10, { detail: "## TL;DR\n- The probe file is written." });
  const context = row(11, {
    type: "agent.usage_update",
    kind: "usage",
    title: "Context",
    presentation: { type: "usage", tokensIn: 39_344, total: 1_000_000 },
    minor: true,
  });
  const done = row(14, { type: "completed", source: "broker", kind: "lifecycle", phase: "completed", title: "Task completed" });

  function answerStaysInTheTrace(events: TaskEventView[]): boolean {
    return ActivityStory.composeWithState(events, true, undefined, true).blocks.some(
      (block) =>
        block.type === "turn" &&
        block.turn.segments.some((segment) =>
          segment.nodes.some((node) => node.type === "message" && node.event.id === answer.id),
        ),
    );
  }

  it("shows the closing answer once, whatever bookkeeping follows it", () => {
    const stderr = row(13, {
      type: "worker_stderr",
      source: "broker",
      kind: "lifecycle",
      phase: "started",
      title: "Worker stderr",
      detail: "[session/create] phase=validate-cwd",
    });

    expect(answerStaysInTheTrace([answer, context, done])).toBe(false);
    expect(answerStaysInTheTrace([answer, context, stderr, done])).toBe(false);
  });

  it("keeps the answer in place when the run failed after saying it", () => {
    const failure = row(13, { type: "agent.error", kind: "error", phase: "failed", title: "Error", detail: "the agent stopped" });

    expect(answerStaysInTheTrace([answer, context, failure, done])).toBe(true);
  });
});

describe("recorded runs", () => {
  async function recorded(name: string): Promise<TaskEventView[]> {
    // SAFETY: the fixture is rows the broker's own presenter wrote for these runs, keyed by run.
    const runs = (await Bun.file(new URL("./fixtures/probe-rows.json", import.meta.url)).json()) as Record<string, TaskEventView[]>;
    return runs[name];
  }

  const workRows = callRows;

  it("keeps an OpenCode read naming its file after an update that leaves the kind out", async () => {
    const calls = workRows(await recorded("opencodeAcpRead")).filter((event) => event.actionId !== undefined);

    expect(calls).toHaveLength(1);
    expect(calls[0]?.title).toBe("Read file");
    expect(calls[0]?.presentation?.path).toBe("/repo/rust/Cargo.toml");
    expect(calls[0]?.phase).toBe("completed");
    expect(calls[0]?.presentation?.outcome).toStartWith("[workspace]");
  });

  it("keeps an Antigravity read naming its file when it ends with only a status", async () => {
    const calls = workRows(await recorded("antigravityAcpRead")).filter((event) => event.actionId === "call_860659");

    expect(calls).toHaveLength(1);
    expect(calls[0]?.title).toBe("Read file");
    expect(calls[0]?.verb).toBe("Read");
    expect(calls[0]?.presentation?.path).toBe("/repo/rust/Cargo.toml");
    expect(calls[0]?.phase).toBe("completed");
  });

  it("reads an Antigravity response streamed in pieces as one answer", async () => {
    const events = await recorded("antigravityCliResponse");
    const messages = normalizeAntigravityEvents(events).filter((event) => event.kind === "message");

    expect(messages).toHaveLength(1);
    expect(messages[0]?.complete).toBe(true);
    expect(ActivityStory.responseEvent(events)?.presentation?.text).toBe(
      "- PROBE_OK\n- First nonempty line: [workspace]\n- Workspace members: 2\n- Tool used: view_file\n",
    );
  });

  it("shows a Claude call once when its transcript repeats a start its hooks already ended", async () => {
    const calls = workRows(await recorded("claudeCliHooksThenTranscript")).filter(
      (event) => event.actionId === "toolu_01LcGnWSJx4ZXjmkM4QGSVmn",
    );

    expect(calls.map((event) => event.phase)).toEqual(["completed"]);
  });
});

/**
 * Whole runs as the daemon served them, captured with
 * `bun scripts/dump-task-events.ts <task id> <fixture name>`.
 */
describe("whole runs, as the daemon served them", () => {
  async function run(name: string): Promise<TaskEventView[]> {
    // SAFETY: the fixture is this task's own `TaskEventView[]`, written by the script above.
    return (await Bun.file(new URL(`./fixtures/${name}.json`, import.meta.url)).json()) as TaskEventView[];
  }

  function storyRows(events: TaskEventView[]): TraceRow[] {
    const composition = ActivityStory.composeWithState(events, true, undefined, true);
    return TraceVisibility.interleavedRows(composition, "/Users/malico/desgn/oga", false).filter(
      (row) => !row.isTechnical,
    );
  }

  it("reads an Antigravity probe as the three calls it made and nothing else", async () => {
    const events = await run("antigravity-acp-probe");
    const composition = ActivityStory.composeWithState(events, true, undefined, true);
    const calls = compositionCalls(composition.blocks);

    // Both edits carry the same file and the same title; only their call ids
    // tell them apart, and a reader has to see both.
    expect(calls.map((call) => call.event.title)).toEqual(["Edit file", "Edit file", "Read file"]);
    expect(new Set(calls.map((call) => call.actionId)).size).toBe(3);

    // The worker's log and the broker's own bookkeeping are not the story.
    const bookkeeping = ["worker_stderr", "created", "started", "worker_spawned", "session_captured", "completed"];
    expect(composition.technical.map((event) => event.type).sort()).toEqual([...bookkeeping].sort());
    expect(storyRows(events).flatMap((row) => (row.event ? [row.event.type] : []))).not.toContain("worker_stderr");
  });

  it("reads every command in a resumed Claude run as one call under its final name", async () => {
    const events = await run("claude-acp-resumed");
    const calls = compositionCalls(ActivityStory.composeWithState(events, true, undefined, true).blocks);
    const commands = calls.filter((call) => call.event.kind === "command");
    const streamed = new Set(
      events.filter((event) => event.kind === "command" && event.actionId !== undefined).map((event) => event.actionId),
    );

    expect(commands.length).toBe(streamed.size);
    // Every one of them opened as "Terminal" and was renamed to the command it
    // ran; a call that reached an end keeps the name it ended with.
    expect(commands.every((call) => call.event.title === "Run command")).toBe(true);
    expect(commands.filter((call) => call.event.target === "Terminal").map((call) => call.status)).toEqual([
      "interrupted",
    ]);
  });

  it("keeps a finished command's output on the call it settles", async () => {
    const events = await run("claude-acp-resumed");
    const calls = compositionCalls(ActivityStory.composeWithState(events, true, undefined, true).blocks);
    const finished = calls.filter((call) => call.event.kind === "command" && call.status === "done");

    expect(finished.length).toBeGreaterThan(0);
    for (const call of finished) {
      const expansion = expansionFromEvent(call.event);
      expect(expansion?.type).toBe("command");
      expect(expansion?.type === "command" && expansion.output).toBeTruthy();
    }
  });
});
