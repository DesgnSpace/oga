import React from "react";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import type { Task, TaskEventView } from "@/bridge/types";
import { ActivityStory, ActivityStoryProjection } from "@/domain/activity";
import { collectRunChanges, RunChangeProjection } from "@/domain/changes";
import { collectRunChangesByTurn, RunChangeByTurnProjection } from "@/domain/changes/grouped";
import type { TraceRow } from "@/domain/trace";
import { TraceRows } from "./Trace";
import { buildTranscript, WorkSegmentCache } from "./transcriptModel";

const BASE_TIME = Date.parse("2026-07-30T15:00:00Z");
const APPENDS = 8;
const REPEATS = 5;
const TRANSCRIPT_SIZES = [150, 1_000, 10_000];
const CHANGE_SIZES = [150, 1_000];

function stamp(index: number): string {
  return new Date(BASE_TIME + index * 1_000).toISOString();
}

const task: Task = {
  id: "task",
  profileId: "claude",
  model: "sonnet",
  prompt: "Do the thing",
  cwd: "/repo",
  state: "running",
  createdAt: stamp(0),
  updatedAt: stamp(0),
  output: "",
  scope: { read: [], write: [] },
  allowQuestions: true,
  canDelegate: false,
};

function action(id: number): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.tool_use",
    kind: "command",
    phase: "completed",
    title: "Run command",
    detail: `step ${id}`,
    createdAt: stamp(id),
    turnId: 1,
  };
}

function claudeRich(id: number): TaskEventView {
  const path = `/repo/src/file-${id % 40}.ts`;
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.tool_use",
    kind: "file",
    phase: "completed",
    title: "Read file",
    detail: path,
    target: path,
    rawText: JSON.stringify({ tool_name: "Read", tool_input: { file_path: path } }),
    presentation: { type: "file", path },
    actionId: `read-${id}`,
    createdAt: stamp(id),
    turnId: 1,
  };
}

function codexRich(id: number): TaskEventView {
  const item = Math.floor((id - 1) / 2) + 1;
  const started = id % 2 === 1;
  return {
    id,
    taskId: "task",
    source: "codex",
    type: started ? "agent.item.started" : "agent.item.completed",
    kind: "command",
    phase: started ? "started" : "completed",
    title: "Run command",
    detail: "bun test",
    target: "bun test",
    rawText: JSON.stringify({ type: started ? "item.started" : "item.completed", item: { id: `item-${item}` } }),
    presentation: { type: "command", command: "bun test" },
    actionId: `item-${item}`,
    createdAt: stamp(id),
    turnId: 1,
  };
}

function edit(id: number, turnId?: number): TaskEventView {
  const oldText = Array.from({ length: 80 }, (_, line) => `const before_${id}_${line} = ${line};`).join("\n");
  const newText = Array.from({ length: 80 }, (_, line) => `const after_${id}_${line} = ${line};`).join("\n");
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.file",
    kind: "file",
    phase: "completed",
    title: "Edit file",
    detail: `file ${id}`,
    createdAt: stamp(id),
    rawText: JSON.stringify({
      tool_input: {
        file_path: `/repo/src/file-${id % 40}.ts`,
        old_string: oldText,
        new_string: newText,
      },
    }),
    turnId,
  };
}

interface TimingSummary {
  medianMs: number;
  p95Ms: number;
}

function percentile(values: number[], fraction: number): number {
  const ordered = values.slice().sort((left, right) => left - right);
  const index = Math.min(ordered.length - 1, Math.max(0, Math.ceil(ordered.length * fraction) - 1));
  return ordered[index] ?? 0;
}

function summarize(values: number[]): TimingSummary {
  return {
    medianMs: Number(percentile(values, 0.5).toFixed(2)),
    p95Ms: Number(percentile(values, 0.95).toFixed(2)),
  };
}

interface TranscriptBenchmarkResult {
  size: number;
  initial: TimingSummary;
  optimizedAppend: TimingSummary;
  baselineAppend: TimingSummary;
  outputEquivalent: boolean;
  transcriptItems: number;
}

function transcriptBenchmark(size: number): TranscriptBenchmarkResult {
  const initialTimes: number[] = [];
  const optimizedTimes: number[] = [];
  const baselineTimes: number[] = [];
  let outputEquivalent = true;
  let optimizedFinal = buildTranscript(task, [], new WorkSegmentCache());

  for (let repeat = 0; repeat < REPEATS; repeat += 1) {
    const optimizedEvents = Array.from({ length: size }, (_, index) => action(index + 1));
    const optimizedCache = new WorkSegmentCache();
    const initialStart = performance.now();
    buildTranscript(task, optimizedEvents, optimizedCache);
    initialTimes.push(performance.now() - initialStart);

    for (let append = 0; append < APPENDS; append += 1) {
      optimizedEvents.push(action(size + append + 1));
      const start = performance.now();
      optimizedFinal = buildTranscript(task, optimizedEvents, optimizedCache);
      optimizedTimes.push(performance.now() - start);
    }

    const baselineEvents = Array.from({ length: size }, (_, index) => action(index + 1));
    for (let append = 0; append < APPENDS; append += 1) {
      baselineEvents.push(action(size + append + 1));
      const start = performance.now();
      buildTranscript(task, baselineEvents, new WorkSegmentCache());
      baselineTimes.push(performance.now() - start);
    }
    const baselineFinal = buildTranscript(task, optimizedEvents, new WorkSegmentCache());
    outputEquivalent &&= JSON.stringify(optimizedFinal) === JSON.stringify(baselineFinal);
  }

  return {
    size,
    initial: summarize(initialTimes),
    optimizedAppend: summarize(optimizedTimes),
    baselineAppend: summarize(baselineTimes),
    outputEquivalent,
    transcriptItems: optimizedFinal.length,
  };
}

interface ProjectionBenchmarkResult {
  fixture: string;
  size: number;
  initial: TimingSummary;
  optimizedAppend: TimingSummary;
  baselineAppend: TimingSummary;
  incrementalRate: number;
  fallbackRate: number;
  outputEquivalent: boolean;
}

function projectionBenchmark(
  fixture: string,
  size: number,
  makeEvent: (id: number) => TaskEventView,
): ProjectionBenchmarkResult {
  const initialTimes: number[] = [];
  const optimizedTimes: number[] = [];
  const baselineTimes: number[] = [];
  let incrementalUpdates = 0;
  let fallbackUpdates = 0;
  let appendUpdates = 0;
  let outputEquivalent = true;

  for (let repeat = 0; repeat < REPEATS; repeat += 1) {
    const events = Array.from({ length: size }, (_, index) => makeEvent(index + 1));
    const projection = new ActivityStoryProjection();
    const initialStart = performance.now();
    const optimizedInitial = projection.update(events, false, undefined);
    initialTimes.push(performance.now() - initialStart);
    const baselineInitial = ActivityStory.composeWithState(events, false, undefined);
    outputEquivalent &&= JSON.stringify(optimizedInitial) === JSON.stringify(baselineInitial);

    for (let append = 0; append < APPENDS; append += 1) {
      events.push(makeEvent(size + append + 1));
      let start = performance.now();
      const optimized = projection.update(events, false, undefined);
      optimizedTimes.push(performance.now() - start);
      start = performance.now();
      const baseline = ActivityStory.composeWithState(events, false, undefined);
      baselineTimes.push(performance.now() - start);
      outputEquivalent &&= JSON.stringify(optimized) === JSON.stringify(baseline);
      appendUpdates += 1;
    }
    incrementalUpdates += projection.incrementalCount;
    fallbackUpdates += projection.fallbackCount;
  }

  return {
    fixture,
    size,
    initial: summarize(initialTimes),
    optimizedAppend: summarize(optimizedTimes),
    baselineAppend: summarize(baselineTimes),
    incrementalRate: Number((incrementalUpdates / appendUpdates).toFixed(3)),
    fallbackRate: Number((fallbackUpdates / appendUpdates).toFixed(3)),
    outputEquivalent,
  };
}

interface ChangeBenchmarkResult {
  size: number;
  optimizedAppend: TimingSummary;
  baselineAppend: TimingSummary;
  outputEquivalent: boolean;
  files: number;
}

function changesBenchmark(size: number): ChangeBenchmarkResult {
  const optimizedTimes: number[] = [];
  const baselineTimes: number[] = [];
  let outputEquivalent = true;
  let optimizedFinal = new RunChangeProjection().update([], "/repo");

  for (let repeat = 0; repeat < REPEATS; repeat += 1) {
    const events = Array.from({ length: size }, (_, index) => edit(index + 1));
    const projection = new RunChangeProjection();
    projection.update(events, "/repo");
    for (let append = 0; append < APPENDS; append += 1) {
      events.push(edit(size + append + 1));
      let start = performance.now();
      optimizedFinal = projection.update(events, "/repo");
      optimizedTimes.push(performance.now() - start);
      start = performance.now();
      const baseline = collectRunChanges(events, "/repo");
      baselineTimes.push(performance.now() - start);
      outputEquivalent &&= JSON.stringify(optimizedFinal) === JSON.stringify(baseline);
    }
  }

  return {
    size,
    optimizedAppend: summarize(optimizedTimes),
    baselineAppend: summarize(baselineTimes),
    outputEquivalent,
    files: optimizedFinal.files.length,
  };
}

interface GroupedBenchmarkResult {
  size: number;
  optimizedAppend: TimingSummary;
  baselineAppend: TimingSummary;
  outputEquivalent: boolean;
  turns: number;
}

function groupedBenchmark(size: number): GroupedBenchmarkResult {
  const optimizedTimes: number[] = [];
  const baselineTimes: number[] = [];
  let outputEquivalent = true;
  let optimizedFinal = new RunChangeByTurnProjection().update([], "/repo");

  for (let repeat = 0; repeat < REPEATS; repeat += 1) {
    const events = Array.from({ length: size }, (_, index) => edit(index + 1, Math.floor(index / 100) + 1));
    const projection = new RunChangeByTurnProjection();
    projection.update(events, "/repo");
    for (let append = 0; append < APPENDS; append += 1) {
      events.push(edit(size + append + 1, Math.floor(size / 100) + 1));
      let start = performance.now();
      optimizedFinal = projection.update(events, "/repo");
      optimizedTimes.push(performance.now() - start);
      start = performance.now();
      const baseline = collectRunChangesByTurn(events, "/repo");
      baselineTimes.push(performance.now() - start);
      outputEquivalent &&= JSON.stringify(optimizedFinal) === JSON.stringify(baseline);
    }
  }

  return {
    size,
    optimizedAppend: summarize(optimizedTimes),
    baselineAppend: summarize(baselineTimes),
    outputEquivalent,
    turns: optimizedFinal.turns.length,
  };
}

function traceRow(id: number): TraceRow {
  return {
    id,
    style: "work",
    state: "done",
    children: [],
    startsExpanded: false,
    isStepStart: false,
    isTechnical: false,
    target: `file-${id}.ts`,
  };
}

function waitForViewport(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 25));
}

interface MountedBenchmarkResult {
  size: number;
  environment: "jsdom";
  initialMountedRows: number;
  afterFullDownRows: number;
  afterFullUpRows: number;
  afterFullUpElements: number;
}

async function mountedBenchmark(size: number): Promise<MountedBenchmarkResult> {
  const rows = Array.from({ length: size }, (_, index) => traceRow(index + 1));
  const root = document.createElement("div");
  Object.defineProperty(root, "clientHeight", { configurable: true, value: 560 });
  Object.defineProperty(root, "scrollTop", { configurable: true, writable: true, value: 0 });
  root.getBoundingClientRect = () => new DOMRect();
  const scrollRoot = { current: root };
  const view = render(React.createElement(TraceRows, { rows, cwd: "/repo", scrollRoot }));
  const panel = view.container.querySelector(".trace-panel") as HTMLElement;
  panel.getBoundingClientRect = () => ({ top: -root.scrollTop } as DOMRect);
  const mountedRows = () => view.container.querySelectorAll(".trace-list-static > .trace-row").length;

  root.scrollTop = 0;
  fireEvent.scroll(root);
  await act(waitForViewport);
  const initialMountedRows = mountedRows();

  root.scrollTop = size * 28;
  fireEvent.scroll(root);
  await act(waitForViewport);
  const afterFullDownRows = mountedRows();

  root.scrollTop = 0;
  fireEvent.scroll(root);
  await act(waitForViewport);
  const afterFullUpRows = mountedRows();
  const afterFullUpElements = view.container.querySelectorAll("*").length;
  cleanup();
  return { size, environment: "jsdom", initialMountedRows, afterFullDownRows, afterFullUpRows, afterFullUpElements };
}

const mounted: MountedBenchmarkResult[] = [];
for (const size of TRANSCRIPT_SIZES) {
  mounted.push(await mountedBenchmark(size));
}

console.log(JSON.stringify({
  model: {
    note: "Pure model timings use sanitized provider-shaped fixtures; no browser or desktop runtime.",
    transcript: TRANSCRIPT_SIZES.map(transcriptBenchmark),
    activity: [
      ...TRANSCRIPT_SIZES.map((size) => projectionBenchmark("plain-command", size, action)),
      ...TRANSCRIPT_SIZES.flatMap((size) => [
        projectionBenchmark("claude-rich-read", size, claudeRich),
        projectionBenchmark("codex-rich-item", size, codexRich),
      ]),
    ],
    changes: CHANGE_SIZES.map(changesBenchmark),
    grouped: CHANGE_SIZES.map(groupedBenchmark),
  },
  rendering: {
    note: "jsdom DOM counts only; browser frames, desktop frames, and memory are unavailable.",
    mounted,
  },
}, null, 2));
