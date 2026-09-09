import { describe, expect, it, mock } from "bun:test";
import type { Transport } from "@/bridge/transport";
import type { StreamStatus, Task, TaskDelta, TaskEventPage, TaskEventView, TaskSnapshot } from "@/bridge/types";

function flush(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function task(overrides: Partial<Task> = {}): Task {
  return {
    id: "task",
    profileId: "worker",
    model: "sonnet",
    prompt: "do the thing",
    cwd: "/repo",
    state: "running",
    createdAt: "2026-07-30T15:00:00Z",
    updatedAt: "2026-07-30T15:00:00Z",
    output: "",
    scope: { read: [], write: [] },
    allowQuestions: true,
  canDelegate: false,
    ...overrides,
  };
}

function event(id: number): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.file",
    kind: "file",
    phase: "completed",
    title: `Read file ${id}`,
    createdAt: "2026-07-30T15:00:00Z",
  };
}

function snapshot(overrides: Partial<TaskSnapshot> = {}): TaskSnapshot {
  return {
    task: task(),
    events: [],
    cursor: 0,
    hasEarlier: false,
    ...overrides,
  };
}

interface FakeTransport extends Transport {
  invoke: ReturnType<typeof mock>;
  listen: ReturnType<typeof mock>;
  emit: (event: string, payload: unknown) => void;
}

function fakeTransport(handlers: Record<string, (args?: Record<string, unknown>) => unknown> = {}): FakeTransport {
  const listeners = new Map<string, Set<(payload: unknown) => void>>();
  const invoke = mock(async (command: string, args?: Record<string, unknown>) => {
    const handler = handlers[command];
    if (!handler) throw { message: `no handler for ${command}` };
    return handler(args);
  });
  const listen = mock((event: string, handler: (payload: unknown) => void) => {
    let set = listeners.get(event);
    if (!set) {
      set = new Set();
      listeners.set(event, set);
    }
    set.add(handler);
    return () => set!.delete(handler);
  });
  return {
    invoke,
    listen,
    emit: (event, payload) => {
      for (const handler of listeners.get(event) ?? []) handler(payload);
    },
  };
}

/**
 * `bridge/events`' `Feed`s are module-level singletons that attach to
 * whatever transport is active the first time they get a subscriber, so
 * each test needs its own fresh module instance (transport included)
 * rather than sharing state through the import cache. Mirrors
 * `bridge/events.test.ts`'s `freshEvents` helper.
 */
async function freshController(transport: Transport) {
  const { resetFeedsForTests } = await import("@/bridge/events");
  resetFeedsForTests();
  const { setTransport } = await import("@/bridge/transport");
  setTransport(transport);
  const controller = await import("./controller");
  controller.clearTaskDetailCacheForTests();
  return controller;
}

describe("watch lifecycle", () => {
  it("follows the task on mount and unwatches on dispose", async () => {
    const watched = fakeTransport({
      broker_watch_task: () => snapshot({ cursor: 5 }),
      broker_stream_status: () => ({ connected: true, cursor: 5, streamFloor: 0, stale: false }) as StreamStatus,
      broker_unwatch_task: () => undefined,
    });
    const { watchTaskDetail } = await freshController(watched);

    const { controller, dispose } = watchTaskDetail("task");
    // loadInitial() is fired without awaiting, mirroring the mount effect.
    await flush();
    await flush();

    expect(watched.invoke).toHaveBeenCalledWith("broker_watch_task", { taskId: "task", events: 150 });
    expect(controller.snapshot.task?.id).toBe("task");

    dispose();
    await flush();

    expect(watched.invoke).toHaveBeenCalledWith("broker_unwatch_task", { taskId: "task" });
  });

  it("takes the initial snapshot's event limit and cursor", async () => {
    const watched = fakeTransport({
      broker_watch_task: () => snapshot({ cursor: 42, events: [], hasEarlier: true, oldestId: 7 }),
      broker_stream_status: () => ({ connected: true, cursor: 42, streamFloor: 0, stale: false }) as StreamStatus,
      broker_unwatch_task: () => undefined,
    });
    const { TaskDetailController } = await freshController(watched);

    const controller = new TaskDetailController("task");
    await controller.loadInitial();

    expect(controller.snapshot.cursor).toBe(42);
    expect(controller.snapshot.hasEarlier).toBe(true);
    expect(controller.snapshot.loading).toBe(false);
    expect(controller.snapshot.connection).toBe("live");
  });

  it("overlaps fallback task and activity reads", async () => {
    const calls: string[] = [];
    let resolveTask: (value: Task) => void = () => {};
    let resolveEvents: (value: TaskEventPage) => void = () => {};
    let resolveBothStarted: () => void = () => {};
    const bothStarted = new Promise<void>((resolve) => {
      resolveBothStarted = resolve;
    });
    const transport = fakeTransport({
      broker_watch_task: () => {
        throw { message: "native follow unavailable" };
      },
      broker_call: (args) => {
        const call = (args?.call as { call?: string } | undefined)?.call;
        calls.push(String(call));
        if (calls.length === 2) resolveBothStarted();
        if (call === "task") {
          return new Promise<Task>((resolve) => {
            resolveTask = resolve;
          });
        }
        if (call === "taskEvents") {
          return new Promise<TaskEventPage>((resolve) => {
            resolveEvents = resolve;
          });
        }
        throw new Error(`unexpected broker call: ${String(call)}`);
      },
    });
    const { TaskDetailController } = await freshController(transport);
    const controller = new TaskDetailController("task");
    const loading = controller.loadInitial();

    await Promise.race([
      bothStarted,
      new Promise<never>((_, reject) => setTimeout(() => reject(new Error("fallback reads did not overlap")), 500)),
    ]);
    expect(calls).toEqual(["task", "taskEvents"]);

    resolveTask(task());
    resolveEvents({ events: [], cursor: 0, hasEarlier: false });
    await loading;

    expect(controller.snapshot.task?.id).toBe("task");
    expect(controller.snapshot.loading).toBe(false);
  });

  it("shares one active watcher and keeps the cached snapshot between views", async () => {
    const watched = fakeTransport({
      broker_watch_task: mock().mockResolvedValue(snapshot({ cursor: 5 })),
      broker_stream_status: () => ({ connected: true, cursor: 5, streamFloor: 0, stale: false }) as StreamStatus,
      broker_unwatch_task: () => undefined,
    });
    const { watchTaskDetail } = await freshController(watched);

    const first = watchTaskDetail("task");
    const second = watchTaskDetail("task");
    await flush();
    await flush();

    expect(watched.invoke.mock.calls.filter(([command]) => command === "broker_watch_task")).toHaveLength(1);
    first.dispose();
    await flush();
    expect(watched.invoke.mock.calls.filter(([command]) => command === "broker_unwatch_task")).toHaveLength(0);

    second.dispose();
    await flush();
    expect(watched.invoke.mock.calls.filter(([command]) => command === "broker_unwatch_task")).toHaveLength(1);

    const reopened = watchTaskDetail("task");
    expect(reopened.controller.snapshot.task?.id).toBe("task");
    expect(reopened.controller.snapshot.loading).toBe(true);
    await flush();
    await flush();
    expect(reopened.controller.snapshot.loading).toBe(false);
    expect(watched.invoke.mock.calls.filter(([command]) => command === "broker_watch_task")).toHaveLength(2);
    reopened.dispose();
  });

  it("drops a cached task when resync confirms it was deleted", async () => {
    const watch = mock()
      .mockResolvedValueOnce(snapshot({ cursor: 5 }))
      .mockImplementationOnce(() => {
        throw { message: "missing", status: 404 };
      });
    const watched = fakeTransport({
      broker_watch_task: watch,
      broker_stream_status: () => ({ connected: true, cursor: 5, streamFloor: 0, stale: false }) as StreamStatus,
      broker_unwatch_task: () => undefined,
      broker_call: (args) => {
        const call = (args?.call as { call?: string } | undefined)?.call;
        if (call === "task") throw { message: "missing", status: 404 };
        if (call === "taskEvents") return { events: [], cursor: 0, hasEarlier: false };
        throw new Error(`unexpected broker call ${String(call)}`);
      },
    });
    const { watchTaskDetail, taskDetailCacheStats } = await freshController(watched);

    const first = watchTaskDetail("task");
    await flush();
    await flush();
    first.dispose();
    await flush();
    await flush();

    const reopened = watchTaskDetail("task");
    await flush();
    await flush();
    await flush();

    expect(reopened.controller.snapshot.task).toBeUndefined();
    reopened.controller.withEvents((events) => expect(events).toHaveLength(0));
    expect(taskDetailCacheStats().entries).toBe(0);
    reopened.dispose();
  });

  it("ignores an evicted request response before the replacement watch", async () => {
    let resolveFirst: (value: TaskSnapshot) => void = () => {};
    const watch = mock()
      .mockImplementationOnce(
        () => new Promise<TaskSnapshot>((resolve) => {
          resolveFirst = resolve;
        }),
      )
      .mockResolvedValueOnce(snapshot({ cursor: 7 }));
    const watched = fakeTransport({
      broker_watch_task: watch,
      broker_stream_status: () => ({ connected: true, cursor: 0, streamFloor: 0, stale: false }) as StreamStatus,
      broker_unwatch_task: () => undefined,
    });
    const { watchTaskDetail } = await freshController(watched);

    const first = watchTaskDetail("task");
    await flush();
    first.dispose();
    const reopened = watchTaskDetail("task");
    resolveFirst(snapshot({ cursor: 1 }));
    await flush();
    await flush();
    await flush();

    expect(reopened.controller.snapshot.cursor).toBe(7);
    expect(watched.invoke.mock.calls.filter(([command]) => command === "broker_unwatch_task")).toHaveLength(1);
    reopened.dispose();
  });

  it("bounds retained payload bytes as well as retained task count", async () => {
    const largeEvents = Array.from({ length: 300 }, (_, index) => ({
      ...event(index + 1),
      detail: "x".repeat(20_000),
    }));
    const watched = fakeTransport({
      broker_watch_task: (args) => {
        const id = String(args?.taskId);
        if (id === "large") return snapshot({ task: task({ id }), events: largeEvents, cursor: largeEvents.length });
        return snapshot({ task: task({ id }), events: [event(1)], cursor: 1 });
      },
      broker_stream_status: () => ({ connected: true, cursor: 0, streamFloor: 0, stale: false }) as StreamStatus,
      broker_unwatch_task: () => undefined,
    });
    const { watchTaskDetail, taskDetailCacheStats, TASK_DETAIL_CACHE_MAX_BYTES, TASK_DETAIL_CACHE_MAX_ENTRIES } = await freshController(watched);

    const large = watchTaskDetail("large");
    await flush();
    await flush();
    large.dispose();
    await flush();
    await flush();
    expect(taskDetailCacheStats().bytes).toBe(0);

    for (let index = 0; index < TASK_DETAIL_CACHE_MAX_ENTRIES + 3; index += 1) {
      const item = watchTaskDetail(`small-${index}`);
      await flush();
      await flush();
      item.dispose();
      await flush();
    }
    const stats = taskDetailCacheStats();
    expect(stats.entries).toBeLessThanOrEqual(TASK_DETAIL_CACHE_MAX_ENTRIES);
    expect(stats.bytes).toBeLessThanOrEqual(TASK_DETAIL_CACHE_MAX_BYTES);
  });
});

describe("delta and reconnect wiring", () => {
  it("keeps a newer delta when a resync snapshot returns behind it", async () => {
    let resolveResync: (value: TaskSnapshot) => void = () => {};
    const watched = fakeTransport({
      broker_watch_task: mock()
        .mockResolvedValueOnce(snapshot({ cursor: 5, events: [event(1), event(2), event(3), event(4), event(5)] }))
        .mockImplementationOnce(
          () => new Promise<TaskSnapshot>((resolve) => {
            resolveResync = resolve;
          }),
        ),
      broker_stream_status: () => ({ connected: true, cursor: 5, streamFloor: 0, stale: false }) as StreamStatus,
      broker_unwatch_task: () => undefined,
    });
    const { watchTaskDetail } = await freshController(watched);

    const open = watchTaskDetail("task");
    await flush();
    await flush();
    const syncing = open.controller.resync();
    await flush();
    open.controller.applyDelta({ taskId: "task", fromCursor: 5, cursor: 6, events: [event(6)] });
    resolveResync(snapshot({ cursor: 5, events: [event(1), event(2), event(3), event(4), event(5)] }));
    await syncing;

    expect(open.controller.snapshot.cursor).toBe(6);
    open.controller.withEvents((events) => expect(events.map((item) => item.id)).toEqual([1, 2, 3, 4, 5, 6]));
    open.dispose();
  });

  it("resyncs when a pushed delta reports a gap", async () => {
    const transport = fakeTransport({
      broker_watch_task: mock()
        .mockResolvedValueOnce(snapshot({ cursor: 5 }))
        .mockResolvedValueOnce(snapshot({ cursor: 20 })),
      broker_stream_status: () => ({ connected: true, cursor: 5, streamFloor: 0, stale: false }) as StreamStatus,
      broker_unwatch_task: () => undefined,
    });
    const { watchTaskDetail } = await freshController(transport);

    const { controller, dispose } = watchTaskDetail("task");
    await flush();
    await flush();
    expect(controller.snapshot.cursor).toBe(5);

    const gapDelta: TaskDelta = { taskId: "task", fromCursor: 40, cursor: 41, events: [] };
    transport.emit("oga-task-delta", gapDelta);
    await flush();
    await flush();

    expect(controller.snapshot.cursor).toBe(20);
    dispose();
  });

  it("resyncs when the broker connection returns from a drop", async () => {
    const transport = fakeTransport({
      broker_watch_task: mock()
        .mockResolvedValueOnce(snapshot({ cursor: 5 }))
        .mockResolvedValueOnce(snapshot({ cursor: 30 })),
      broker_stream_status: () => ({ connected: true, cursor: 5, streamFloor: 0, stale: false }) as StreamStatus,
      broker_unwatch_task: () => undefined,
    });
    const { watchTaskDetail } = await freshController(transport);

    const { controller, dispose } = watchTaskDetail("task");
    await flush();
    await flush();
    expect(controller.snapshot.connection).toBe("live");

    transport.emit("oga-broker-status", {
      connected: false,
      cursor: 5,
      streamFloor: 0,
      stale: false,
      error: "down",
    });
    expect(controller.snapshot.connection).toBe("offline");

    transport.emit("oga-broker-status", { connected: true, cursor: 30, streamFloor: 0, stale: false });
    await flush();
    await flush();

    expect(controller.snapshot.connection).toBe("live");
    expect(controller.snapshot.cursor).toBe(30);
    dispose();
  });

  it("unwatching while a resync is in flight leaves the controller consistent", async () => {
    let resolveWatch: (value: TaskSnapshot) => void = () => {};
    const transport = fakeTransport({
      broker_watch_task: mock()
        .mockResolvedValueOnce(snapshot({ cursor: 5 }))
        .mockImplementationOnce(
          () =>
            new Promise<TaskSnapshot>((resolve) => {
              resolveWatch = resolve;
            }),
        ),
      broker_stream_status: () => ({ connected: true, cursor: 5, streamFloor: 0, stale: false }) as StreamStatus,
      broker_unwatch_task: () => undefined,
    });
    const { watchTaskDetail } = await freshController(transport);

    const { controller, dispose } = watchTaskDetail("task");
    await flush();
    await flush();
    expect(controller.snapshot.cursor).toBe(5);

    // A gap delta starts a second, still in-flight resync.
    transport.emit("oga-task-delta", { taskId: "task", fromCursor: 40, cursor: 41, events: [] } as TaskDelta);
    await flush();

    // The view goes away before that resync settles.
    dispose();

    // The in-flight resync now resolves. Applying it must not throw even
    // though nothing is listening for the outcome any more.
    expect(() => resolveWatch(snapshot({ cursor: 99 }))).not.toThrow();
    await flush();
    await flush();

    expect(transport.invoke).toHaveBeenCalledWith("broker_unwatch_task", { taskId: "task" });
  });
});
