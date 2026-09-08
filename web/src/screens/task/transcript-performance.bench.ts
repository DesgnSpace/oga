import React from "react";
import { cleanup, render } from "@testing-library/react";
import type { Task, TaskEventView } from "@/bridge/types";
import { collectRunChanges, RunChangeProjection } from "@/domain/changes";
import { collectRunChangesByTurn, RunChangeByTurnProjection } from "@/domain/changes/grouped";
import type { TraceRow } from "@/domain/trace";
import { TraceRows } from "./Trace";
import { buildTranscript, WorkSegmentCache } from "./transcriptModel";

const BASE_TIME = Date.parse("2026-07-30T15:00:00Z");

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

function median(values: number[]): number {
  const ordered = values.slice().sort((left, right) => left - right);
  return Number((ordered[Math.floor(ordered.length / 2)] ?? 0).toFixed(2));
}

function transcriptBenchmark(size: number) {
  const optimizedEvents = Array.from({ length: size }, (_, index) => action(index + 1));
  const optimizedCache = new WorkSegmentCache();
  const optimizedInitialStart = performance.now();
  buildTranscript(task, optimizedEvents, optimizedCache);
  const optimizedInitialMs = performance.now() - optimizedInitialStart;
  const optimizedAppend: number[] = [];
  for (let index = 0; index < 10; index += 1) {
    optimizedEvents.push(action(size + index + 1));
    const start = performance.now();
    buildTranscript(task, optimizedEvents, optimizedCache);
    optimizedAppend.push(performance.now() - start);
  }
  const optimizedFinal = buildTranscript(task, optimizedEvents, optimizedCache);

  const baselineEvents = Array.from({ length: size }, (_, index) => action(index + 1));
  const baselineAppend: number[] = [];
  for (let index = 0; index < 10; index += 1) {
    baselineEvents.push(action(size + index + 1));
    const start = performance.now();
    buildTranscript(task, baselineEvents, new WorkSegmentCache());
    baselineAppend.push(performance.now() - start);
  }
  const baselineFinal = buildTranscript(task, optimizedEvents, new WorkSegmentCache());
  return {
    size,
    initialMs: Number(optimizedInitialMs.toFixed(2)),
    appendOptimizedMedianMs: median(optimizedAppend),
    appendBaselineMedianMs: median(baselineAppend),
    outputEquivalent: JSON.stringify(optimizedFinal) === JSON.stringify(baselineFinal),
    transcriptItems: optimizedFinal.length,
  };
}

function changesBenchmark(size: number) {
  const events = Array.from({ length: size }, (_, index) => edit(index + 1));
  const projection = new RunChangeProjection();
  projection.update(events, "/repo");
  const optimized: number[] = [];
  const baseline: number[] = [];
  for (let index = 0; index < 10; index += 1) {
    events.push(edit(size + index + 1));
    let start = performance.now();
    projection.update(events, "/repo");
    optimized.push(performance.now() - start);
    start = performance.now();
    collectRunChanges(events, "/repo");
    baseline.push(performance.now() - start);
  }
  const optimizedFinal = projection.update(events, "/repo");
  const baselineFinal = collectRunChanges(events, "/repo");
  return {
    size,
    appendOptimizedMedianMs: median(optimized),
    appendBaselineMedianMs: median(baseline),
    outputEquivalent: JSON.stringify(optimizedFinal) === JSON.stringify(baselineFinal),
    files: optimizedFinal.files.length,
  };
}

function groupedBenchmark(size: number) {
  const events = Array.from({ length: size }, (_, index) => edit(index + 1, Math.floor(index / 100) + 1));
  const projection = new RunChangeByTurnProjection();
  projection.update(events, "/repo");
  const optimized: number[] = [];
  const baseline: number[] = [];
  for (let index = 0; index < 10; index += 1) {
    events.push(edit(size + index + 1, Math.floor(size / 100) + 1));
    let start = performance.now();
    projection.update(events, "/repo");
    optimized.push(performance.now() - start);
    start = performance.now();
    collectRunChangesByTurn(events, "/repo");
    baseline.push(performance.now() - start);
  }
  const optimizedFinal = projection.update(events, "/repo");
  const baselineFinal = collectRunChangesByTurn(events, "/repo");
  return {
    size,
    appendOptimizedMedianMs: median(optimized),
    appendBaselineMedianMs: median(baseline),
    outputEquivalent: JSON.stringify(optimizedFinal) === JSON.stringify(baselineFinal),
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

function mountedBenchmark(size: number) {
  const rows = Array.from({ length: size }, (_, index) => traceRow(index + 1));
  const start = performance.now();
  const view = render(React.createElement(TraceRows, { rows, cwd: "/repo" }));
  const list = view.container.querySelector(".trace-list-static");
  const result = {
    size,
    renderMs: Number((performance.now() - start).toFixed(2)),
    baselineRows: size,
    mountedRows: list?.querySelectorAll(":scope > .trace-row").length ?? 0,
    mountedElements: view.container.querySelectorAll("*").length,
  };
  cleanup();
  return result;
}

class BenchmarkIntersectionObserver implements IntersectionObserver {
  readonly root = null;
  readonly rootMargin = "";
  readonly thresholds: readonly number[] = [];

  constructor(_callback: IntersectionObserverCallback, _options?: IntersectionObserverInit) {}

  observe(_target: Element) {}
  disconnect() {}
  unobserve(_target: Element) {}
  takeRecords(): IntersectionObserverEntry[] {
    return [];
  }
}

globalThis.IntersectionObserver = BenchmarkIntersectionObserver;

console.log(JSON.stringify({
  transcript: [transcriptBenchmark(1_000), transcriptBenchmark(10_000)],
  changes: [changesBenchmark(100), changesBenchmark(1_000)],
  grouped: groupedBenchmark(1_000),
  mounted: [mountedBenchmark(1_000), mountedBenchmark(10_000)],
}, null, 2));
