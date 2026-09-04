import { describe, expect, it, mock } from "bun:test";
import type { Transport } from "@/bridge/transport";
import type { StreamStatus, Task, TaskDelta, TaskSnapshot } from "@/bridge/types";

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
    ...overrides,
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
  return import("./controller");
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
});

describe("delta and reconnect wiring", () => {
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
