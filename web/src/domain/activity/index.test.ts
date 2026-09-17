// Ported from rust/crates/oga-ui/src/activity/mod.rs `#[cfg(test)] mod tests`.

import { describe, expect, it } from "bun:test";
import type { EventKind, TaskEventView } from "@/bridge/types";
import {
  type ActivityCall,
  type ActivityComposition,
  type ActivityNode,
  ActivityStory,
  ActivityStoryProjection,
  type ActivitySegment,
  type ActivityTurn,
  compositionCalls,
  nodesCallCount,
  nodesDurationMs,
  normalizeAntigravityEvents,
  type ReasoningPulse,
  turnDurationMs,
} from "./index";

/**
 * Field shapes below are taken from real `agent.hook` rows recorded for a
 * Claude Code subagent run in `~/.oga/oga.db` (task e2cc68b5, agent_id
 * a93973f285e94df1c): `SubagentStart`/`SubagentStop` carry a shared
 * `agent_id` and `agent_type`; `PreToolUse`/`PostToolUse` in between carry
 * only `agent_id`, no `parent_tool_use_id` back to the parent's `Agent` call.
 */
function hookEvent(
  id: number,
  title: string,
  detail: string | undefined,
  payload: Record<string, unknown>,
): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.hook",
    kind: "lifecycle",
    phase: "info",
    title,
    detail,
    createdAt: `2026-07-30T15:00:${String(id % 60).padStart(2, "0")}Z`,
    rawText: JSON.stringify(payload),
  };
}

function subagentStart(id: number, agentId: string, agentType = "general-purpose"): TaskEventView {
  return hookEvent(id, "SubagentStart", agentType, { agent_id: agentId, agent_type: agentType });
}

function subagentStop(id: number, agentId: string): TaskEventView {
  return hookEvent(id, "SubagentStop", undefined, { agent_id: agentId, hook_event_name: "SubagentStop" });
}

function subagentToolUse(id: number, agentId: string, command: string): TaskEventView {
  return {
    ...event(id, "tool", "Bash"),
    phase: "completed",
    detail: command,
    rawText: JSON.stringify({ agent_id: agentId }),
  };
}

/**
 * The second bracketing shape, taken from `agent.system` rows of task
 * 749deccd: `task_started`/`task_notification` share a `task_id`, and the
 * sub-agent's own calls point back at the `tool_use_id` that launched it.
 */
function taskStarted(id: number, taskId: string, toolUseId: string, description: string): TaskEventView {
  return {
    ...event(id, "lifecycle", "Subagent started"),
    phase: "started",
    detail: description,
    rawText: JSON.stringify({
      type: "system",
      subtype: "task_started",
      task_id: taskId,
      tool_use_id: toolUseId,
      description,
    }),
  };
}

function taskNotification(id: number, taskId: string, status: string): TaskEventView {
  return {
    ...event(id, "lifecycle", "Subagent finished"),
    phase: status === "completed" ? "completed" : "failed",
    detail: status === "completed" ? undefined : "Failed",
    rawText: JSON.stringify({ type: "system", subtype: "task_notification", task_id: taskId, status }),
  };
}

function subagentChildTool(id: number, parentToolUseId: string, command: string): TaskEventView {
  return {
    ...event(id, "command", "Bash"),
    detail: command,
    parentActionId: parentToolUseId,
  };
}

function skippedNotice(id: number, detail: string): TaskEventView {
  return {
    ...event(id, "lifecycle", "Some activity was not recorded"),
    type: "line_dropped",
    source: "broker",
    phase: "info",
    detail,
    presentation: { type: "signal", text: `Some activity was not recorded — ${detail}`, level: "warning" },
  };
}

function usageWindowEvent(id: number, windows: Record<string, number>): TaskEventView {
  const unifiedWindows = Object.fromEntries(
    Object.entries(windows).map(([name, utilization]) => [name, { utilization, resetsAt: 1_788_322_800 }]),
  );
  return {
    ...event(id, "lifecycle", "Usage window"),
    phase: "info",
    minor: true,
    detail: "13%",
    rawText: JSON.stringify({ type: "rate_limit_event", rate_limit_info: { unifiedWindows } }),
  };
}

function event(id: number, kind: EventKind, title: string): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: `agent.${kind}`,
    kind,
    phase: "completed",
    title,
    createdAt: `2026-07-30T15:00:${String(id % 60).padStart(2, "0")}Z`,
  };
}

/** One block of reasoning; text is absent when the provider redacted it. */
function thinking(id: number, text?: string): TaskEventView {
  return { ...event(id, "reasoning", "Thinking"), phase: "info", detail: text };
}

function thinkingTokens(id: number, tokens: number): TaskEventView {
  return {
    ...event(id, "usage", "Thinking tokens"),
    phase: "info",
    minor: true,
    presentation: { type: "usage", tokensThinking: tokens },
  };
}

function receipt(id: number): TaskEventView {
  return { ...event(id, "usage", "Run summary"), presentation: { type: "usage", turns: 1 } };
}

function turnsOf(composition: ActivityComposition): ActivityTurn[] {
  return composition.blocks.flatMap((block) => (block.type === "turn" ? [block.turn] : []));
}

function segmentsOf(composition: ActivityComposition): ActivitySegment[] {
  return turnsOf(composition).flatMap((turn) => turn.segments);
}

function nodesOf(composition: ActivityComposition): ActivityNode[] {
  const flatten = (nodes: ActivityNode[]): ActivityNode[] =>
    nodes.flatMap((node) => (node.type === "subagent" ? [node, ...flatten(node.subagent.nodes)] : [node]));
  return flatten(segmentsOf(composition).flatMap((segment) => segment.nodes));
}

function callsOf(composition: ActivityComposition): ActivityCall[] {
  return compositionCalls(composition.blocks);
}

function subagentsOf(composition: ActivityComposition) {
  return nodesOf(composition).flatMap((node) => (node.type === "subagent" ? [node.subagent] : []));
}

/** What a reader sees where a call has no id of its own to fold on. */
function looseEvents(composition: ActivityComposition): TaskEventView[] {
  return nodesOf(composition).flatMap((node) => (node.type === "notice" ? [node.event] : []));
}

function reasoningPulses(composition: ActivityComposition): ReasoningPulse[] {
  return nodesOf(composition).flatMap((node) => (node.type === "thinking" ? [node.pulse] : []));
}

type LookupTitle = "Search code" | "Find files" | "Inspect changes";

function lookupEvent(id: number, title: LookupTitle, target: string): TaskEventView {
  const tool = title === "Find files" ? "glob" : "grep";
  return {
    ...event(id, "tool", title),
    source: "opencode",
    type: "agent.tool_use",
    actionId: `call_${id}`,
    verb: title === "Find files" ? "Found" : title === "Inspect changes" ? "Inspected" : "Searched",
    detail: target,
    presentation: { type: "tool", text: target },
    rawText: JSON.stringify({
      type: "tool_use",
      part: {
        type: "tool",
        tool,
        callID: `call_${id}`,
        state: { status: "completed", input: { pattern: target } },
      },
    }),
  };
}

describe("ActivityStory.compose", () => {
  it("renders the captured Antigravity run as shared tool and error rows", async () => {
    const lines = (await Bun.file(new URL("./fixtures/antigravity-run.jsonl", import.meta.url)).text()).trim().split("\n");
    const events: TaskEventView[] = lines.map((rawText, id) => ({
      id,
      taskId: "task",
      source: "antigravity",
      type: "agent.event",
      kind: "tool",
      phase: "info",
      title: "Event",
      createdAt: `2026-09-03T00:00:${String(id).padStart(2, "0")}Z`,
      rawText,
    }));
    const normalized = normalizeAntigravityEvents(events);
    const tools = normalized.filter((event) => event.actionId !== undefined);
    const errors = normalized.filter((event) => event.phase === "failed");

    expect(tools.map((event) => [event.title, event.phase])).toEqual([
      ["Find code", "started"],
      ["Find code", "failed"],
      ["Find code", "started"],
      ["Find code", "failed"],
      ["view_file", "completed"],
      ["list_dir", "completed"],
      ["grep_search", "completed"],
      ["find_by_name", "completed"],
      ["replace_file_content", "completed"],
      ["run_command", "completed"],
    ]);
    expect(tools[0]?.presentation).toEqual({ type: "tool", text: "tray icon loading code in oga-desktop" });
    expect(tools[1]?.result).toBe("invalid params: cwd is required");
    expect(tools[4]?.kind).toBe("file");
    expect(tools[4]?.presentation).toEqual({ type: "file", path: "/abs/path/file.ts" });
    expect(tools[4]?.result).toBe("184 lines, 8597 bytes");
    expect(tools[6]?.presentation).toEqual({ type: "tool", text: "deploy-landing" });
    expect(tools[8]?.kind).toBe("file");
    expect(tools[8]?.presentation?.change).toBeUndefined();
    expect(tools[9]?.presentation).toEqual({ type: "command", command: "git status" });
    expect(tools[9]?.result).toContain("working tree clean");
    expect(errors.at(-1)?.detail).toContain("Individual quota reached");
    expect(normalized.some((event) => event.detail === "step 0")).toBe(false);
  });

  it("shows the workers and carried context at a handoff boundary", () => {
    const composition = ActivityStory.compose([
      { ...event(1, "message", "Before handoff"), type: "agent.text" },
      {
        ...event(2, "lifecycle", "Handed off"),
        type: "handed_off",
        detail: "night-shift → day-shift · rebuilt brief",
      },
      {
        ...event(3, "lifecycle", "Handoff brief"),
        type: "handoff_brief",
        detail: "verbatim carry-over, 7579 chars",
      },
      { ...event(4, "message", "After handoff"), type: "agent.text" },
    ]);

    const boundary = composition.blocks[0];
    expect(boundary?.type).toBe("handoff");
    if (boundary?.type !== "handoff") throw new Error("expected handoff boundary");
    expect(boundary.boundary.chain).toBe("night-shift → day-shift · rebuilt brief");
    expect(boundary.boundary.briefTier).toBe("verbatim");
    expect(composition.technical.some((event) => event.type === "handoff_brief")).toBe(true);
  });

  it("folds every update a call sent into that one call", () => {
    const start: TaskEventView = {
      ...event(1, "command", "Bash"),
      phase: "started",
      actionId: "call",
      rawText: JSON.stringify({ type: "tool_execution_start" }),
    };
    const update: TaskEventView = { ...start, id: 2, rawText: JSON.stringify({ type: "tool_execution_update" }) };
    const end: TaskEventView = { ...start, id: 3, phase: "completed", rawText: JSON.stringify({ type: "tool_execution_end" }) };

    const calls = callsOf(ActivityStory.compose([start, update, end]));

    expect(calls).toHaveLength(1);
    expect(calls[0].events.map((value) => value.id)).toEqual([1, 2, 3]);
    expect(calls[0].status).toBe("done");
  });

  it("keeps three reads that share a title as three calls", () => {
    const events = Array.from({ length: 3 }, (_, index) => ({
      ...event(index + 1, "file", "Read file"),
      detail: `src/${index}.rs`,
      verb: "Read",
      actionId: `call_${index}`,
      createdAt: `2026-07-30T15:00:0${index}Z`,
    }));

    const composition = ActivityStory.compose(events);

    expect(callsOf(composition).map((call) => call.event.detail)).toEqual(["src/0.rs", "src/1.rs", "src/2.rs"]);
    expect(turnDurationMs(turnsOf(composition)[0])).toBe(2_000);
  });

  it("keeps a stretch of lookups as one call each", () => {
    const events = [
      lookupEvent(1, "Search code", "TaskEventView in rust"),
      lookupEvent(2, "Search code", "ActivityStory in web"),
      lookupEvent(3, "Find files", "web/src/**/*.css"),
      lookupEvent(4, "Inspect changes", "git diff --stat"),
    ];

    const calls = callsOf(ActivityStory.compose(events));

    expect(calls.map((call) => call.actionId)).toEqual(["call_1", "call_2", "call_3", "call_4"]);
  });

  it("keeps an edit between two lookups where the worker made it", () => {
    const events = [
      lookupEvent(1, "Search code", "TaskEventView in rust"),
      { ...event(2, "file", "Edit file"), detail: "src/main.rs", target: "src/main.rs", verb: "Edited", actionId: "call_edit" },
      lookupEvent(3, "Search code", "ActivityStory in web"),
    ];

    const calls = callsOf(ActivityStory.compose(events));

    expect(calls.map((call) => call.event.title)).toEqual(["Search code", "Edit file", "Search code"]);
  });

  /** The bug this model exists to fix: six edits read as "Edited oga.css ×2". */
  it("keeps six passes over one file as six calls", () => {
    const events = Array.from({ length: 6 }, (_, index) => ({
      ...event(index + 1, "file", "Edit file"),
      detail: `web/src/oga.css · rule ${index + 1}`,
      target: "web/src/oga.css",
      verb: "Edited",
      actionId: `call_${index}`,
    }));

    const calls = callsOf(ActivityStory.compose(events));

    expect(calls).toHaveLength(6);
    expect(new Set(calls.map((call) => call.id)).size).toBe(6);
  });

  it("keeps a loop that comes round four times as every pass it made", () => {
    const cycle = (round: number): TaskEventView[] => [
      { ...event(round * 3 + 1, "file", "Edit file"), detail: "src/app.ts", target: "src/app.ts", verb: "Edited", actionId: `edit_${round}` },
      { ...event(round * 3 + 2, "command", "Check lint"), detail: "bun run lint", target: "bun run lint", verb: "Checked", actionId: `lint_${round}` },
      { ...event(round * 3 + 3, "command", "Check tests"), detail: "bun test", target: "bun test", verb: "Checked", actionId: `test_${round}` },
    ];

    const calls = callsOf(ActivityStory.compose([0, 1, 2, 3].flatMap(cycle)));

    expect(calls).toHaveLength(12);
  });

  it("counts a replayed call once when a resumed session repeats it", () => {
    const call = (id: number, phase: "started" | "completed"): TaskEventView => ({
      ...event(id, "command", "Run command"),
      phase,
      verb: phase === "completed" ? "Ran" : "Running",
      target: "bun run dev",
      detail: "bun run dev",
      actionId: "toolu_1",
    });

    const calls = callsOf(ActivityStory.compose([call(1, "started"), call(2, "completed"), call(3, "started")]));

    expect(calls).toHaveLength(1);
    expect(calls[0].event.phase).toBe("completed");
  });

  it("keeps one call as one row, whatever its title said last", () => {
    const opened: TaskEventView = {
      ...event(1, "command", "Run command"),
      phase: "started",
      title: "Terminal",
      target: "Terminal",
      actionId: "toolu_1",
    };
    const settled: TaskEventView = { ...opened, id: 2, phase: "completed", title: "Run command", target: "bun test" };

    const calls = callsOf(ActivityStory.compose([opened, settled]));

    expect(calls).toHaveLength(1);
    expect(calls[0].event.title).toBe("Run command");
    expect(calls[0].event.target).toBe("bun test");
  });

  it("leaves a call the turn never closed as interrupted, not running", () => {
    const opened: TaskEventView = {
      ...event(1, "command", "Run command"),
      phase: "started",
      target: "bun test",
      actionId: "toolu_1",
      turnId: 1,
    };
    const next = { ...event(2, "message", "Agent message"), detail: "Picking up again.", turnId: 2 };

    const calls = callsOf(ActivityStory.compose([opened, next]));

    expect(calls.map((call) => call.status)).toEqual(["interrupted"]);
  });

  it("nests a subagent's work under a collapsible group keyed by agent_id", () => {
    const events: TaskEventView[] = [
      event(1, "message", "Response"),
      subagentStart(2, "a93973f285e94df1c"),
      subagentToolUse(3, "a93973f285e94df1c", "wc -l README.md"),
      subagentToolUse(4, "a93973f285e94df1c", "grep -rn Worked web/src"),
      subagentStop(5, "a93973f285e94df1c"),
      event(6, "message", "Response"),
    ];

    const composition = ActivityStory.compose(events);
    const [subagent] = subagentsOf(composition);

    expect(subagent?.start.id).toBe(2);
    expect(subagent?.label).toBe("Subagent · general-purpose");
    // Every event between the pair, and the stop itself, stays reachable
    // behind the group instead of being dropped.
    expect(subagent?.nodes.flatMap((node) => (node.type === "notice" ? [node.event.id] : []))).toEqual([3, 4, 5]);
    // Nothing outside the pair is swallowed into it.
    expect(segmentsOf(composition).flatMap((segment) => segment.lead ? [segment.lead.id] : [])).toEqual([1, 6]);
  });

  it("nests a hook subagent run under normalized started/finished titles keyed by agent_id", () => {
    // Rust normalizes hook SubagentStart/SubagentStop to started/finished titles,
    // keeping agent_id in the raw payload; trace groups on agent_id.
    const agentId = "a26c5ad830544e025";
    const started = hookEvent(2, "Subagent started", "general-purpose", {
      hook_event_name: "SubagentStart",
      agent_id: agentId,
      agent_type: "general-purpose",
    });
    const finished = hookEvent(5, "Subagent finished", "general-purpose", {
      hook_event_name: "SubagentStop",
      agent_id: agentId,
      agent_type: "general-purpose",
    });
    const events: TaskEventView[] = [
      event(1, "message", "Response"),
      started,
      subagentToolUse(3, agentId, "Read pwa/src/lib/api.ts"),
      subagentToolUse(4, agentId, "Read api/internal/httpx/ratelimit.go"),
      finished,
      event(6, "message", "Response"),
    ];

    const [subagent] = subagentsOf(ActivityStory.compose(events));

    expect(subagent?.start.id).toBe(2);
    expect(subagent?.label).toBe("Subagent · general-purpose");
    expect(subagent?.nodes.flatMap((node) => (node.type === "notice" ? [node.event.id] : []))).toEqual([3, 4, 5]);
  });

  it("nests a streamed sub-agent's work under its description", () => {
    const events: TaskEventView[] = [
      event(1, "message", "Response"),
      taskStarted(2, "b5r75b4nr", "toolu_parent", "Audit the allocation path"),
      subagentChildTool(3, "toolu_parent", "rg -n allocate rust/"),
      subagentChildTool(4, "toolu_parent", "wc -l rust/src/alloc.rs"),
      taskNotification(5, "b5r75b4nr", "completed"),
      event(6, "message", "Response"),
    ];

    const composition = ActivityStory.compose(events);
    const [subagent] = subagentsOf(composition);

    expect(subagent?.start.id).toBe(2);
    expect(subagent?.label).toBe("Subagent · Audit the allocation path");
    expect(subagent?.nodes.flatMap((node) => (node.type === "notice" ? [node.event.id] : []))).toEqual([3, 4, 5]);
    expect(subagent?.status).toBe("done");
    expect(segmentsOf(composition).flatMap((segment) => segment.lead ? [segment.lead.id] : [])).toEqual([1, 6]);
  });

  it("keeps two streamed sub-agents' interleaved tool calls correlated by their parent call", () => {
    const events: TaskEventView[] = [
      taskStarted(1, "task-a", "toolu_a", "Port the parser"),
      taskStarted(2, "task-b", "toolu_b", "Write the migration"),
      subagentChildTool(3, "toolu_b", "bun run migrate"),
      subagentChildTool(4, "toolu_a", "cargo test -p parser"),
      taskNotification(5, "task-a", "completed"),
      taskNotification(6, "task-b", "failed"),
    ];

    const subagents = subagentsOf(ActivityStory.compose(events));
    const owned = (index: number) =>
      subagents[index].nodes.flatMap((node) => (node.type === "notice" ? [node.event.id] : []));

    expect(subagents).toHaveLength(2);
    expect(owned(0)).toEqual([4, 5]);
    expect(owned(1)).toEqual([3, 6]);
  });

  it("leaves a sub-agent start with no finish flat", () => {
    const composition = ActivityStory.compose([
      taskStarted(1, "task-a", "toolu_a", "Port the parser"),
      subagentChildTool(2, "toolu_a", "cargo test -p parser"),
    ]);

    expect(subagentsOf(composition)).toHaveLength(0);
  });

  it("folds the thinking between two tool calls into one readable stretch", () => {
    const composition = ActivityStory.compose([
      thinking(1, "First the shape."),
      thinking(2, "Then the cost."),
      { ...event(3, "tool", "Bash"), detail: "bun test" },
      thinking(4, "Now the write."),
    ]);
    expect(reasoningPulses(composition).map((pulse) => pulse.text)).toEqual([
      "First the shape.\n\nThen the cost.",
      "Now the write.",
    ]);
  });

  it("keeps a stretch the provider redacted, with nothing to read", () => {
    const composition = ActivityStory.compose([thinking(1), thinking(2), event(3, "message", "Response")]);
    const [pulse] = reasoningPulses(composition);
    expect(pulse?.text).toBeUndefined();
  });

  it("replaces a streamed block with the fuller copy that grew from it", () => {
    const composition = ActivityStory.compose([
      thinking(1, "First"),
      thinking(2, "First the"),
      thinking(3, "First the shape."),
    ]);
    expect(reasoningPulses(composition).map((pulse) => pulse.text)).toEqual(["First the shape."]);
  });

  it("counts reasoning tokens onto the stretch they arrived during and the receipt", () => {
    const composition = ActivityStory.compose([
      thinkingTokens(1, 50),
      thinking(2, "Weighing two options"),
      thinkingTokens(3, 29),
      { ...event(4, "tool", "Bash"), detail: "bun test" },
      receipt(5),
    ]);
    expect(reasoningPulses(composition).map((pulse) => pulse.tokens)).toEqual([79]);
    const block = composition.blocks.find((value) => value.type === "receipt");
    expect(block?.type === "receipt" ? block.thinkingTokens : undefined).toBe(79);
  });

  it("gives a token counter no stretch of its own", () => {
    const composition = ActivityStory.compose([{ ...event(1, "tool", "Bash"), detail: "bun test" }, thinkingTokens(2, 50)]);
    expect(reasoningPulses(composition)).toEqual([]);
  });

  it("reports every skipped line as one notice with the total", () => {
    const composition = ActivityStory.compose([
      skippedNotice(1, "3 lines skipped"),
      event(2, "message", "Response"),
      skippedNotice(3, "4 lines skipped"),
      skippedNotice(4, "1 event skipped"),
    ]);
    const notices = looseEvents(composition).filter((value) => value.type === "line_dropped");

    expect(notices).toHaveLength(1);
    expect(notices[0].presentation?.text).toBe(
      "Some activity was not recorded — 7 lines skipped · 1 event skipped",
    );
  });

  it("puts the busiest usage window on the receipt instead of the trace", () => {
    const composition = ActivityStory.compose([
      usageWindowEvent(1, { five_hour: 0.02, seven_day: 0.13 }),
      { ...event(2, "usage", "Run summary"), presentation: { type: "usage", costUsd: 0.42 } },
    ]);
    const receipt = composition.blocks.find((block) => block.type === "receipt");
    if (receipt?.type !== "receipt") throw new Error("expected a receipt");
    expect(receipt.usageWindow).toStartWith("Usage window 13% · resets ");
    expect(looseEvents(composition)).toHaveLength(0);
  });

  it("leaves an unpaired SubagentStart flat instead of guessing where it ends", () => {
    const events: TaskEventView[] = [
      subagentStart(1, "orphan-start"),
      subagentToolUse(2, "orphan-start", "wc -l README.md"),
    ];

    expect(subagentsOf(ActivityStory.compose(events))).toHaveLength(0);
  });

  it("leaves an unpaired SubagentStop flat instead of guessing where it started", () => {
    const events: TaskEventView[] = [
      subagentToolUse(1, "orphan-stop", "wc -l README.md"),
      subagentStop(2, "orphan-stop"),
    ];

    expect(subagentsOf(ActivityStory.compose(events))).toHaveLength(0);
  });

  it("keeps two concurrent subagents' interleaved events correlated by their own agent_id", () => {
    // Real runs interleave two subagents' hook rows event-by-event (task
    // e2cc68b5 ran five at once); a naive nearest-stop match would cross-wire
    // them.
    const events: TaskEventView[] = [
      subagentStart(1, "agent-a"),
      subagentStart(2, "agent-b"),
      subagentToolUse(3, "agent-a", "wc -l README.md"),
      subagentToolUse(4, "agent-b", "grep -rn Worked web/src"),
      subagentStop(5, "agent-a"),
      subagentStop(6, "agent-b"),
    ];

    const subagents = subagentsOf(ActivityStory.compose(events));
    const owned = (index: number) =>
      subagents[index].nodes.flatMap((node) => (node.type === "notice" ? [node.event.id] : []));

    expect(subagents).toHaveLength(2);
    expect(owned(0)).toEqual([3, 5]);
    expect(owned(1)).toEqual([4, 6]);
  });

  it("keeps every one of ten thousand events now that nothing is windowed away", () => {
    const events = Array.from({ length: 10_000 }, (_, index) => {
      const id = index + 1;
      const kind: EventKind = id % 2 === 0 ? "tool" : "file";
      return { ...event(id, kind, "Read"), detail: `file-${id}` };
    });
    expect(nodesOf(ActivityStory.compose(events))).toHaveLength(events.length);
  });
});

describe("ActivityStoryProjection", () => {
  it("matches the canonical composition while appending a simple run", () => {
    const events = Array.from({ length: 3 }, (_, index) => ({
      ...event(index + 1, "command", "Run command"),
      detail: `step ${index + 1}`,
      turnId: 1,
    }));
    const projection = new ActivityStoryProjection();
    projection.update(events, false, undefined);

    events.push({ ...event(4, "command", "Run command"), detail: "step 4", turnId: 1 });
    expect(projection.update(events, false, undefined)).toEqual(ActivityStory.composeWithState(events, false, undefined));
  });

  it("falls back when an append changes the grouping shape", () => {
    const events = [{ ...event(1, "command", "Run command"), detail: "step 1", turnId: 1 }];
    const projection = new ActivityStoryProjection();
    projection.update(events, false, undefined);
    events.push({ ...event(2, "command", "Run command"), detail: "step 1", turnId: 1 });

    expect(projection.update(events, false, undefined)).toEqual(ActivityStory.composeWithState(events, false, undefined));
  });

  it("falls back for retry events that the canonical composer rewrites", () => {
    const events = [{ ...event(1, "command", "API retry"), detail: "retrying" }];
    const projection = new ActivityStoryProjection();
    projection.update(events, false, undefined);
    events.push({ ...event(2, "command", "Run command"), detail: "step 2" });

    expect(projection.update(events, false, undefined)).toEqual(ActivityStory.composeWithState(events, false, undefined));
  });

  it("falls back for canonical signal rows", () => {
    const events = [{ ...event(1, "command", "Worker needs input"), detail: "answer this" }];
    const projection = new ActivityStoryProjection();
    projection.update(events, false, undefined);
    events.push({ ...event(2, "command", "Run command"), detail: "step 2" });

    expect(projection.update(events, false, undefined)).toEqual(ActivityStory.composeWithState(events, false, undefined));
  });

  it("falls back when canonical composition removes a redundant failure row", () => {
    const events = [
      { ...event(1, "command", "Agent error"), detail: "failed" },
      { ...event(2, "command", "Task failed"), detail: "failed" },
    ];
    const projection = new ActivityStoryProjection();
    projection.update(events, false, undefined);
    events.push({ ...event(3, "command", "Turn Failed"), detail: "failed" });

    expect(projection.update(events, false, undefined)).toEqual(ActivityStory.composeWithState(events, false, undefined));
  });

  it("appends rich provider rows with raw payloads, presentations, and action ids", () => {
    const rich = (id: number, actionId: string, path: string): TaskEventView => ({
      ...event(id, "file", "Read file"),
      detail: path,
      target: path,
      rawText: JSON.stringify({ tool_name: "Read", tool_input: { file_path: path } }),
      presentation: { type: "file", path },
      actionId,
      turnId: 1,
    });
    const events = [rich(1, "read-1", "/repo/one.ts")];
    const projection = new ActivityStoryProjection();
    projection.update(events, false, undefined);
    events.push(rich(2, "read-2", "/repo/two.ts"));

    expect(projection.update(events, false, undefined)).toEqual(ActivityStory.composeWithState(events, false, undefined));
    expect(projection.update(events, false, undefined)).toBe(projection.update(events, false, undefined));
  });

  it("settles a rich action incrementally when its provider update is adjacent", () => {
    const rich = (id: number, phase: "started" | "completed"): TaskEventView => ({
      ...event(id, "command", "Run command"),
      phase,
      detail: "bun test",
      target: "bun test",
      rawText: JSON.stringify({ tool_input: { command: "bun test" } }),
      presentation: { type: "command", command: "bun test" },
      actionId: "command-1",
      turnId: 1,
    });
    const events = [rich(1, "started")];
    const projection = new ActivityStoryProjection();
    projection.update(events, false, undefined);
    events.push(rich(2, "completed"));

    expect(projection.update(events, false, undefined)).toEqual(ActivityStory.composeWithState(events, false, undefined));
    expect(projection.update(events, false, undefined)).toBe(projection.update(events, false, undefined));
  });

  it("keeps repeated rich Codex actions equivalent while appending", () => {
    const codex = (id: number): TaskEventView => {
      const item = Math.floor((id - 1) / 2) + 1;
      const started = id % 2 === 1;
      return {
        ...event(id, "command", "Run command"),
        source: "codex",
        type: started ? "agent.item.started" : "agent.item.completed",
        phase: started ? "started" : "completed",
        detail: "bun test",
        target: "bun test",
        rawText: JSON.stringify({ type: started ? "item.started" : "item.completed", item: { id: `item-${item}` } }),
        presentation: { type: "command", command: "bun test" },
        actionId: `item-${item}`,
        turnId: 1,
      };
    };
    const events = [codex(1), codex(2)];
    const projection = new ActivityStoryProjection();
    projection.update(events, false, undefined);
    events.push(codex(3), codex(4));

    expect(projection.update(events, false, undefined)).toEqual(ActivityStory.composeWithState(events, false, undefined));
    expect(projection.update(events, false, undefined)).toBe(projection.update(events, false, undefined));
  });

  it("falls back when a rich action reopens or changes turns", () => {
    const rich = (id: number, phase: "started" | "completed", turnId: number): TaskEventView => ({
      ...event(id, "command", "Run command"),
      phase,
      detail: "bun test",
      target: "bun test",
      rawText: JSON.stringify({ tool_input: { command: "bun test" } }),
      presentation: { type: "command", command: "bun test" },
      actionId: "command-1",
      turnId,
    });
    const events = [rich(1, "started", 1), rich(2, "completed", 1)];
    const projection = new ActivityStoryProjection();
    projection.update(events, false, undefined);
    events.push(rich(3, "started", 1));
    expect(projection.update(events, false, undefined)).toEqual(ActivityStory.composeWithState(events, false, undefined));

    events.push({ ...rich(4, "completed", 2), actionId: "command-2" });
    expect(projection.update(events, false, undefined)).toEqual(ActivityStory.composeWithState(events, false, undefined));
    expect(projection.fallbackCount).toBe(3);
  });
});

/**
 * Codex reports one piece of work as `item.started` → [`item.updated`*] →
 * `item.completed`, paired by `$.item.id` and carried on `TaskEventView.actionId`
 * by the Rust mapper (see `item_event_view` in `oga-events`). The activity
 * layer must fold that triple into one row the same way it folds any other
 * provider's action id, and must still render a row for an item that started
 * but never completed.
 */
describe("Codex item pairing", () => {
  function codexItem(
    id: number,
    itemId: string,
    overrides: Partial<TaskEventView> = {},
  ): TaskEventView {
    return {
      id,
      taskId: "task",
      source: "codex",
      type: "agent.item.started",
      kind: "command",
      phase: "started",
      title: "Run command",
      verb: "Running",
      target: "npm test",
      actionId: itemId,
      createdAt: `2026-08-31T00:00:${String(id % 60).padStart(2, "0")}Z`,
      ...overrides,
    };
  }

  it("collapses a started/completed pair sharing an item id into one row", () => {
    const events = [
      codexItem(1, "item_2"),
      codexItem(2, "item_2", {
        type: "agent.item.completed",
        phase: "completed",
        verb: "Ran",
        detail: "exit 0",
      }),
    ];

    const calls = callsOf(ActivityStory.compose(events));

    expect(calls).toHaveLength(1);
    expect(calls[0].event.verb).toBe("Ran");
  });

  it("still renders a row for an item that started but never completed", () => {
    const events = [codexItem(1, "item_9")];

    const calls = callsOf(ActivityStory.compose(events));

    expect(calls).toHaveLength(1);
    expect(calls[0].event.phase).toBe("started");
  });

  it("keeps two different items separate even when they arrive interleaved", () => {
    const events = [
      codexItem(1, "item_2"),
      codexItem(2, "item_3", { target: "npm build" }),
      codexItem(3, "item_2", { type: "agent.item.completed", phase: "completed", verb: "Ran" }),
      codexItem(4, "item_3", {
        type: "agent.item.completed",
        phase: "completed",
        verb: "Ran",
        target: "npm build",
      }),
    ];

    expect(callsOf(ActivityStory.compose(events)).map((call) => call.actionId)).toEqual(["item_2", "item_3"]);
  });
});

/**
 * Pi reports one message as `message_start` → [`message_update`*] → `message_end`,
 * but unlike Codex's items or its own `toolCallId`, none of those three carry an id
 * in the payload. The Rust mapper (`pi_message_view` in `oga-events`) papers over
 * that with a fixed action id shared by the whole stream; `foldCalls` merges each
 * later event's own title/detail over the open row (role never changes mid-stream,
 * so the title stays stable) and an update with nothing new falls back to whatever
 * the row already had. This is the fold that keeps 35,000 streamed deltas from
 * becoming 35,000 rows.
 */
describe("Pi message-stream pairing", () => {
  function piMessage(id: number, overrides: Partial<TaskEventView> = {}): TaskEventView {
    return {
      id,
      taskId: "task",
      source: "pi",
      type: "agent.message_start",
      kind: "message",
      phase: "started",
      title: "Agent message",
      actionId: "pi:message-stream",
      createdAt: `2026-08-31T00:00:${String(id % 60).padStart(2, "0")}Z`,
      ...overrides,
    };
  }

  it("collapses message_start/update/end deltas sharing the stream key into one row", () => {
    const events = [
      piMessage(1),
      piMessage(2, { type: "agent.message_update", detail: "## Res" }),
      piMessage(3, { type: "agent.message_update", detail: "## Resume flag" }),
      piMessage(4, {
        type: "agent.message_end",
        phase: "completed",
        detail: "## Resume flag\n\n`--session-id`.",
        complete: true,
      }),
    ];

    const calls = callsOf(ActivityStory.compose(events));

    expect(calls).toHaveLength(1);
    expect(calls[0].event.detail).toBe("## Resume flag\n\n`--session-id`.");
    expect(calls[0].event.phase).toBe("completed");
  });

  it("opens a fresh call for the next message instead of reopening the one message_end closed", () => {
    const events = [
      piMessage(1),
      piMessage(2, { type: "agent.message_end", phase: "completed", detail: "first", complete: true }),
      piMessage(3, { title: "User message", detail: "second" }),
    ];

    expect(callsOf(ActivityStory.compose(events)).map((call) => call.event.detail)).toEqual(["first", "second"]);
  });
});

/**
 * Narration lines taken verbatim from real runs in `~/.oga/oga.db`: task
 * 4611be7e (opencode, `agent.text` parts) and 749deccd (claude, `agent.assistant`
 * text blocks).
 */
describe("stretches the worker opened with its own words", () => {
  function narration(id: number, text: string): TaskEventView {
    return { ...event(id, "message", "Agent message"), type: "agent.text", detail: text };
  }

  function call(id: number, kind: "file" | "command", title: string): TaskEventView {
    return { ...event(id, kind, title), detail: `subject ${id}`, actionId: `call_${id}` };
  }

  it("opens a stretch each time the worker speaks and holds the work that follows", () => {
    const composition = ActivityStory.compose([
      narration(1, "Locating the toast implementation and reference behavior"),
      call(2, "file", "Read file"),
      call(3, "command", "Run command"),
      narration(4, "Raising the global toast layer above every current overlay"),
      call(5, "file", "Edit file"),
      call(6, "command", "Run command"),
    ]);

    const segments = segmentsOf(composition);
    expect(segments.map((segment) => segment.lead?.detail)).toEqual([
      "Locating the toast implementation and reference behavior",
      "Raising the global toast layer above every current overlay",
    ]);
    expect(segments.map((segment) => nodesCallCount(segment.nodes))).toEqual([2, 2]);
  });

  it("names the turn after the first thing the worker said", () => {
    const composition = ActivityStory.composeWithState(
      [narration(1, "Root cause found. Applying fixes."), call(2, "file", "Edit file")],
      false,
      undefined,
      false,
    );

    expect(turnsOf(composition)[0].title).toBe("Root cause found. Applying fixes.");
    expect(turnsOf(composition)[0].titleFrom).toBe(1);
  });

  it("leaves a settled run's closing answer to the response instead of the trace", () => {
    const answer = "## TL;DR\n- **Choice**: Keep one global toast layer above overlays.";
    const composition = ActivityStory.composeWithState(
      [call(1, "command", "Run command"), narration(2, answer)],
      true,
      undefined,
      false,
    );

    expect(segmentsOf(composition).some((segment) => segment.lead !== undefined)).toBe(false);
    expect(ActivityStory.responseEvent([narration(2, answer)])?.id).toBe(2);
  });

  it("counts the calls and the elapsed time a stretch covers", () => {
    const composition = ActivityStory.compose([
      narration(1, "Running focused tests, type checks, and the configured linter"),
      { ...call(2, "command", "Run command"), createdAt: "2026-07-30T15:00:00Z" },
      { ...call(3, "file", "Read file"), createdAt: "2026-07-30T15:00:40Z" },
    ]);

    const worked = segmentsOf(composition)[0].nodes.slice(1);
    expect(nodesCallCount(worked)).toBe(2);
    expect(nodesDurationMs(worked)).toBe(40_000);
  });
});
