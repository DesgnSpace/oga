import { afterEach, beforeEach, describe, expect, it, mock } from "bun:test";
import { act, cleanup, render, screen } from "@testing-library/react";
import { setTransport, type Transport } from "@/bridge/transport";
import { resetFeedsForTests } from "@/bridge/events";
import { EVENT_BATCH_EVENT, STATUS_EVENT, type EventBatch, type StreamStatus, type TaskSummary } from "@/bridge/types";
import { SidebarController } from "@/state/sidebar-state";
import { resetTaskOutcomeViewsForTests, taskOutcomeViews } from "@/state/task-outcome-views";
import * as sidebarProjection from "@/state/sidebar-projection";
import Sidebar from "./Sidebar";

const projectionFromState = mock(sidebarProjection.projectionFromState);

mock.module("@/state/sidebar-projection", () => ({
  ...sidebarProjection,
  projectionFromState,
}));

function task(id: string, preview: string, extra: Partial<TaskSummary> = {}): TaskSummary {
  return {
    id,
    profileId: "worker",
    model: "sonnet",
    cwd: "/repo",
    state: "running",
    promptPreview: preview,
    createdAt: "2026-08-31T10:00:00Z",
    updatedAt: "2026-08-31T10:00:00Z",
    ...extra,
  };
}

function transport(
  options: { profiles?: Array<{ id: string; label: string }>; tasks?: TaskSummary[] } = {},
): Transport {
  return {
    invoke: async (command: string, args?: Record<string, unknown>) => {
      if (command !== "broker_call") throw { message: `no handler for ${command}` };
      const call = (args?.call as { call: string }).call;
      if (call === "summary") {
        return {
          profiles: options.profiles ?? [],
          tasks: options.tasks ?? [task("one", "first task"), task("two", "second task")],
          tasksHasMore: false,
          profileFailures: [],
          grants: [],
          memoryProjects: [],
        } as never;
      }
      throw { message: `no handler for ${call}` };
    },
    listen: () => {},
  };
}

describe("the sidebar", () => {
  beforeEach(() => {
    projectionFromState.mockClear();
    resetTaskOutcomeViewsForTests();
  });
  afterEach(cleanup);

  it("hands a clicked task to the shell", async () => {
    setTransport(transport());
    const selected = mock();
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={selected} />);

    const row = await screen.findByText("second task");
    row.closest("a")!.click();

    expect(selected).toHaveBeenCalledWith("two");
  });

  it("announces unread and viewed outcomes in each task row", async () => {
    setTransport(transport());
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    const row = await screen.findByText("second task");
    const link = row.closest("a")!;
    expect(link.querySelector(".task-dot")?.className).toContain("task-dot-unread");
    expect(link.textContent).toContain("New update");

    act(() => taskOutcomeViews.markViewed(task("two", "second task")));

    expect(link.querySelector(".task-dot")?.className).toContain("task-dot-viewed");
    expect(link.textContent).toContain("Viewed");
  });

  it("keeps the running pulse class after direct navigation marks it viewed", async () => {
    const running = task("running", "running task");
    setTransport(transport({ tasks: [running] }));
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    const row = await screen.findByText("running task");
    const link = row.closest("a")!;
    act(() => taskOutcomeViews.markViewed(running));

    const dot = link.querySelector(".task-dot")!;
    expect(dot.className).toContain("task-dot-look-running");
    expect(dot.className).toContain("task-dot-viewed");
  });

  it("does not mark a task viewed from selection alone", async () => {
    setTransport(transport());
    const selected = mock();
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={selected} />);

    const row = await screen.findByText("second task");
    await act(async () => row.closest("a")!.click());

    expect(taskOutcomeViews.isViewed(task("two", "second task"))).toBe(false);
  });

  it("shows a held task with the waiting look and its reason", async () => {
    const waiting = task("waiting", "waiting task", {
      state: "pending",
      hold: {
        kind: "dependency",
        waitingOn: "blocker",
        note: "Waiting for another task",
        expiresAt: "2026-09-09T10:00:00Z",
      },
    });
    setTransport(transport({ tasks: [waiting] }));
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    const row = await screen.findByText("waiting task");
    const dot = row.closest("a")!.querySelector(".task-dot")!;
    expect(dot.className).toContain("task-dot-look-waiting");
    expect(dot.getAttribute("title")).toBe("Waiting for another task");
    expect(row.closest("a")!.textContent).toContain("Waiting for another task");
  });

  it("reads a task blocked on a dependency as a problem, not as waiting", async () => {
    const blocked = task("blocked", "blocked task", {
      state: "blocked",
      completion: { blocked: true, code: "cancelled", dependencyBlocked: true },
    });
    setTransport(transport({ tasks: [blocked] }));
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    const row = await screen.findByText("blocked task");
    const dot = row.closest("a")!.querySelector(".task-dot")!;
    expect(dot.className).toContain("task-dot-look-problem");
    expect(dot.getAttribute("title")).toBe("Blocked");
  });

  it("names the worker that ran the task in the subtitle", async () => {
    setTransport(transport({ profiles: [{ id: "worker", label: "Night Shift" }] }));
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    await screen.findByText("second task");
    expect(screen.getAllByText(/^Night Shift/)).toHaveLength(2);
  });

  it("omits the worker from the subtitle when it is not recorded", async () => {
    setTransport(transport({ profiles: [] }));
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    await screen.findByText("second task");
    expect(screen.queryByText(/^Unknown worker/)).toBeNull();
  });

  it("rebuilds the projection when an outcome view changes", async () => {
    setTransport(transport());
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    await screen.findByText("second task");
    projectionFromState.mockClear();

    act(() => {
      taskOutcomeViews.markViewed(task("two", "second task"));
    });

    expect(projectionFromState).toHaveBeenCalled();
  });

  it("keeps the projection stable for unrelated state changes", async () => {
    setTransport(transport());
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    await screen.findByText("second task");
    projectionFromState.mockClear();

    act(() => {
      controller.setSidebarWidth(controller.snapshot.sidebarWidth + 1);
      controller.selectTask("two");
      controller.applyConnection({ connected: false, cursor: 0, streamFloor: 0, stale: false });
      controller.applyBatch({ cursor: 1, streamFloor: 0, stale: false, pointers: [] });
    });

    expect(projectionFromState).not.toHaveBeenCalled();
  });

  it("rebuilds the projection when tasks change", async () => {
    setTransport(transport());
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    await screen.findByText("second task");
    projectionFromState.mockClear();

    act(() => {
      controller.update((state) => ({
        ...state,
        tasks: state.tasks.map((entry) => entry.id === "two" ? { ...entry, title: "updated task" } : entry),
      }));
    });

    await screen.findByText("updated task");
    expect(projectionFromState).toHaveBeenCalledTimes(1);
  });

  it("keeps a task row mounted while live task fields change", async () => {
    setTransport(transport());
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    const row = await screen.findByText("second task");
    const link = row.closest("a");
    expect(link!.querySelector(".task-dot")!.className).toContain("task-dot-look-running");

    act(() => {
      controller.update((state) => ({
        ...state,
        tasks: state.tasks.map((entry) => entry.id === "two"
          ? { ...entry, title: "updated task", state: "completed", updatedAt: "2026-09-16T10:01:00Z" }
          : entry),
      }));
    });

    const updated = await screen.findByText("updated task");
    expect(updated.closest("a")).toBe(link);
    expect(screen.getAllByRole("option")).toHaveLength(2);
    expect(link!.querySelector(".task-dot")!.className).toContain("task-dot-look-settled");
  });

  it("points a new user at worker settings without reading the model catalogue", async () => {
    const calls: string[] = [];
    setTransport({
      ...transport({ tasks: [] }),
      invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
        const call = (args?.call as { call: string } | undefined)?.call ?? command;
        calls.push(call);
        if (call === "modelSettings") return { workers: [] } as T;
        return transport({ tasks: [] }).invoke(command, args);
      },
    });

    render(<Sidebar sidebarController={new SidebarController()} onOpenSettings={mock()} />);

    await screen.findByText("No workers yet");
    expect(calls).not.toContain("modelSettings");
  });

  it("lists each new task as it starts, including two started back to back", async () => {
    const tasks = [task("one", "first task")];
    let batchListener: ((batch: EventBatch) => void) | undefined;
    resetFeedsForTests();
    setTransport({
      ...transport(),
      invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
        const call = (args?.call as { call: string } | undefined)?.call;
        if (call !== "summary") return transport().invoke(command, args);
        return { profiles: [], tasks: [...tasks], tasksHasMore: false, profileFailures: [], grants: [], memoryProjects: [] } as T;
      },
      listen: (event, handle) => {
        if (event === EVENT_BATCH_EVENT) batchListener = handle as (batch: EventBatch) => void;
      },
    });
    render(<Sidebar sidebarController={new SidebarController()} />);
    await screen.findByText("first task");

    const start = (id: string, preview: string, cursor: number) => {
      tasks.unshift(task(id, preview, { state: "queued" }));
      act(() => batchListener?.({
        cursor,
        streamFloor: 0,
        stale: false,
        pointers: [{ id: cursor, cursor, taskId: id, type: "created", kind: "lifecycle", state: "queued", at: new Date().toISOString(), title: "", summary: "" }],
      }));
    };
    start("two", "second task", 1);
    await screen.findByText("second task");
    start("three", "third task", 2);
    await screen.findByText("third task");
  });

  it("keeps a newer stream status when the startup read returns late", async () => {
    let resolveStatus: ((status: StreamStatus) => void) | undefined;
    let statusListener: ((status: StreamStatus) => void) | undefined;
    const statusResult = new Promise<StreamStatus>((resolve) => {
      resolveStatus = resolve;
    });
    resetFeedsForTests();
    setTransport({
      invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
        if (command === "broker_stream_status") {
          return statusResult as Promise<T>;
        }
        if (command !== "broker_call") throw { message: `no handler for ${command}` };
        const call = (args?.call as { call: string }).call;
        if (call === "summary") {
          return {
            profiles: [],
            tasks: [task("one", "first task"), task("two", "second task")],
            tasksHasMore: false,
            profileFailures: [],
            grants: [],
            memoryProjects: [],
          } as T;
        }
        if (call === "modelSettings") return { workers: [] } as T;
        throw { message: `no handler for ${call}` };
      },
      listen: (event, handle) => {
        if (event === STATUS_EVENT) statusListener = handle as (status: StreamStatus) => void;
      },
    });
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} />);
    await screen.findByText("second task");

    await act(async () => {
      statusListener?.({ connected: true, cursor: 0, streamFloor: 0, stale: false });
      resolveStatus?.({ connected: false, cursor: 0, streamFloor: 0, stale: false });
      await Promise.resolve();
    });

    expect(controller.snapshot.connection).toBe("connected");
  });
});
