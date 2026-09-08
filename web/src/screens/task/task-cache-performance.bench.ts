import { setTransport, type Transport } from "@/bridge/transport";
import { resetFeedsForTests } from "@/bridge/events";
import type { Task, TaskEventView, TaskSnapshot } from "@/bridge/types";
import * as taskDetail from "@/state/taskDetail";

const REPEATS = 20;
const SMALL_EVENTS = 64;
const LARGE_EVENTS = 2_048;

interface CacheStats {
  entries: number;
  bytes: number;
  maxEntries: number;
  maxBytes: number;
}

interface CacheApi {
  clearTaskDetailCacheForTests?: () => void;
  taskDetailCacheStats?: () => CacheStats;
}

const cacheApi = taskDetail as unknown as CacheApi;

interface Fixture {
  name: "small" | "large";
  events: TaskEventView[];
  cursor: number;
}

interface FixturePair {
  a: Fixture;
  b: Fixture;
}

interface TimingSummary {
  medianMs: number;
  p95Ms: number;
}

interface SwitchResult {
  fixture: Fixture["name"];
  mode: "no-cache-control" | "cache";
  coldVisible: TimingSummary;
  coldCurrent: TimingSummary;
  warmVisible: TimingSummary;
  warmCurrent: TimingSummary;
  watchRequests: number;
  unwatchRequests: number;
  activeWatchersAfterRelease: number;
  retained: CacheStats;
}

function stamp(index: number): string {
  return new Date(Date.parse("2026-07-30T15:00:00Z") + index * 1_000).toISOString();
}

function task(id: string, state: Task["state"] = "running"): Task {
  return {
    id,
    profileId: "worker",
    model: "sonnet",
    prompt: "Run the fixed benchmark task",
    cwd: "/repo",
    state,
    createdAt: stamp(0),
    updatedAt: stamp(1),
    output: "",
    scope: { read: [], write: [] },
    allowQuestions: true,
  };
}

function event(taskId: string, id: number, provider: "claude" | "codex"): TaskEventView {
  const isClaude = provider === "claude";
  const path = `/repo/src/file-${id % 40}.ts`;
  const started = id % 2 === 1;
  return {
    id,
    taskId,
    source: provider,
    type: isClaude ? "agent.tool_use" : started ? "agent.item.started" : "agent.item.completed",
    kind: isClaude ? "file" : "command",
    phase: isClaude ? "completed" : started ? "started" : "completed",
    title: isClaude ? "Read file" : "Run command",
    detail: isClaude ? path : "bun test --filter task-detail",
    target: isClaude ? path : "bun test --filter task-detail",
    rawText: JSON.stringify({
      provider,
      action: isClaude ? "Read" : started ? "item.started" : "item.completed",
      path,
      output: "sanitized provider-shaped fixture",
    }),
    presentation: isClaude ? { type: "file", path } : { type: "command", command: "bun test --filter task-detail" },
    actionId: `${provider}-${Math.ceil(id / 2)}`,
    createdAt: stamp(id),
    turnId: 1,
  };
}

function fixture(name: Fixture["name"], taskId: string, size: number, provider: "claude" | "codex"): Fixture {
  return {
    name,
    events: Array.from({ length: size }, (_, index) => event(taskId, index + 1, provider)),
    cursor: size,
  };
}

function snapshot(taskId: string, fixture: Fixture): TaskSnapshot {
  return {
    task: task(taskId),
    events: structuredClone(fixture.events),
    cursor: fixture.cursor,
    oldestId: 1,
    hasEarlier: false,
  };
}

function percentile(values: number[], fraction: number): number {
  const ordered = values.slice().sort((left, right) => left - right);
  const index = Math.min(ordered.length - 1, Math.max(0, Math.ceil(ordered.length * fraction) - 1));
  return ordered[index] ?? 0;
}

function summarize(values: number[]): TimingSummary {
  return {
    medianMs: Number(percentile(values, 0.5).toFixed(3)),
    p95Ms: Number(percentile(values, 0.95).toFixed(3)),
  };
}

function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

async function waitForVisible(controller: { snapshot: { task?: Task } }): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (controller.snapshot.task !== undefined) return;
    await flush();
  }
  throw new Error("controller did not show task content");
}

async function waitForCurrent(controller: { snapshot: { task?: Task; cursor: number; loading: boolean } }, cursor: number): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (controller.snapshot.task !== undefined && controller.snapshot.cursor >= cursor && !controller.snapshot.loading) return;
    await flush();
  }
  throw new Error(`controller did not reach cursor ${cursor}`);
}

function makeTransport(fixtures: Map<string, Fixture>): {
  transport: Transport;
  counts: { watch: number; unwatch: number; active: Set<string> };
} {
  const listeners = new Map<string, Set<(payload: unknown) => void>>();
  const counts = { watch: 0, unwatch: 0, active: new Set<string>() };
  const transport: Transport = {
    async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
      await Promise.resolve();
      if (command === "broker_watch_task") {
        const taskId = String(args?.taskId);
        const current = fixtures.get(taskId);
        if (!current) throw { message: `missing fixture ${taskId}` };
        counts.watch += 1;
        counts.active.add(taskId);
        return snapshot(taskId, current) as T;
      }
      if (command === "broker_unwatch_task") {
        counts.unwatch += 1;
        counts.active.delete(String(args?.taskId));
        return undefined as T;
      }
      if (command === "broker_stream_status") {
        return { connected: true, cursor: 0, streamFloor: 0, stale: false } as T;
      }
      if (command === "broker_call") {
        const call = (args?.call as { call?: string } | undefined)?.call;
        if (call === "task") {
          const taskId = String((args?.call as { taskId?: string } | undefined)?.taskId);
          const current = fixtures.get(taskId);
          if (!current) throw { message: `missing fixture ${taskId}` };
          return task(taskId) as T;
        }
        if (call === "taskEvents") {
          const taskId = String((args?.call as { taskId?: string } | undefined)?.taskId);
          const current = fixtures.get(taskId);
          if (!current) throw { message: `missing fixture ${taskId}` };
          return { events: structuredClone(current.events), cursor: current.cursor, oldestId: 1, hasEarlier: false } as T;
        }
      }
      throw { message: `unexpected command ${command}` };
    },
    listen<T>(event: string, handle: (payload: T) => void): void {
      let handlers = listeners.get(event);
      if (!handlers) {
        handlers = new Set();
        listeners.set(event, handlers);
      }
      handlers.add(handle as (payload: unknown) => void);
    },
  };
  return { transport, counts };
}

function clearCache(): void {
  cacheApi.clearTaskDetailCacheForTests?.();
}

function cacheStats(): CacheStats {
  return cacheApi.taskDetailCacheStats?.() ?? { entries: 0, bytes: 0, maxEntries: 0, maxBytes: 0 };
}

async function switchPair(
  pair: FixturePair,
  fixtureName: Fixture["name"],
  retainCache: boolean,
): Promise<SwitchResult> {
  const mode = retainCache ? "cache" : "control";
  const fixtures = new Map<string, Fixture>();
  for (let repeat = 0; repeat < REPEATS; repeat += 1) {
    fixtures.set(`a-${mode}-${repeat}`, pair.a);
    fixtures.set(`b-${mode}-${repeat}`, pair.b);
  }
  const { transport, counts } = makeTransport(fixtures);
  resetFeedsForTests();
  setTransport(transport);
  clearCache();

  const coldVisible: number[] = [];
  const coldCurrent: number[] = [];
  const warmVisible: number[] = [];
  const warmCurrent: number[] = [];

  for (let repeat = 0; repeat < REPEATS; repeat += 1) {
    const aId = `a-${mode}-${repeat}`;
    const bId = `b-${mode}-${repeat}`;
    const coldAStart = performance.now();
    const coldA = taskDetail.watchTaskDetail(aId);
    await waitForVisible(coldA.controller);
    coldVisible.push(performance.now() - coldAStart);
    await waitForCurrent(coldA.controller, pair.a.cursor);
    coldCurrent.push(performance.now() - coldAStart);
    coldA.dispose();
    await flush();
    if (!retainCache) clearCache();

    const coldBStart = performance.now();
    const coldB = taskDetail.watchTaskDetail(bId);
    await waitForVisible(coldB.controller);
    coldVisible.push(performance.now() - coldBStart);
    await waitForCurrent(coldB.controller, pair.b.cursor);
    coldCurrent.push(performance.now() - coldBStart);
    coldB.dispose();
    await flush();
    if (!retainCache) clearCache();

    const warmStart = performance.now();
    const warmA = taskDetail.watchTaskDetail(aId);
    await waitForVisible(warmA.controller);
    warmVisible.push(performance.now() - warmStart);
    await waitForCurrent(warmA.controller, pair.a.cursor);
    warmCurrent.push(performance.now() - warmStart);
    warmA.dispose();
    await flush();
    if (!retainCache) clearCache();
  }

  return {
    fixture: fixtureName,
    mode: retainCache ? "cache" : "no-cache-control",
    coldVisible: summarize(coldVisible),
    coldCurrent: summarize(coldCurrent),
    warmVisible: summarize(warmVisible),
    warmCurrent: summarize(warmCurrent),
    watchRequests: counts.watch,
    unwatchRequests: counts.unwatch,
    activeWatchersAfterRelease: counts.active.size,
    retained: cacheStats(),
  };
}

async function retentionCycle(size: number): Promise<{ retained: CacheStats; activeWatchers: number }> {
  const fixtures = new Map<string, Fixture>();
  const count = Math.max(cacheStats().maxEntries + 3, 11);
  for (let index = 0; index < count; index += 1) {
    const id = `eviction-${index}`;
    fixtures.set(id, fixture("small", id, size, index % 2 === 0 ? "claude" : "codex"));
  }
  const { transport, counts } = makeTransport(fixtures);
  resetFeedsForTests();
  setTransport(transport);
  clearCache();
  for (const taskId of fixtures.keys()) {
    const watched = taskDetail.watchTaskDetail(taskId);
    await waitForCurrent(watched.controller, fixtures.get(taskId)!.cursor);
    watched.dispose();
    await flush();
  }
  return { retained: cacheStats(), activeWatchers: counts.active.size };
}

async function main(): Promise<void> {
  const smallControl = await switchPair(
    { a: fixture("small", "a", SMALL_EVENTS, "claude"), b: fixture("small", "b", SMALL_EVENTS, "codex") },
    "small",
    false,
  );
  const small = await switchPair(
    { a: fixture("small", "a", SMALL_EVENTS, "claude"), b: fixture("small", "b", SMALL_EVENTS, "codex") },
    "small",
    true,
  );
  const largeControl = await switchPair(
    { a: fixture("large", "a", LARGE_EVENTS, "claude"), b: fixture("large", "b", LARGE_EVENTS, "codex") },
    "large",
    false,
  );
  const large = await switchPair(
    { a: fixture("large", "a", LARGE_EVENTS, "claude"), b: fixture("large", "b", LARGE_EVENTS, "codex") },
    "large",
    true,
  );
  const smallRetention = await retentionCycle(SMALL_EVENTS);
  const largeRetention = await retentionCycle(LARGE_EVENTS);
  console.log(JSON.stringify({
    workload: { repeats: REPEATS, smallEvents: SMALL_EVENTS, largeEvents: LARGE_EVENTS, capacityCycle: "maxEntries + 3" },
    environment: { runtime: "bun", bun: Bun.version, platform: process.platform, arch: process.arch },
    synthetic: true,
    switches: { control: [smallControl, largeControl], cache: [small, large] },
    retention: { small: smallRetention, large: largeRetention },
  }, null, 2));
}

await main();
