import { afterEach, beforeEach, describe, expect, it, mock } from "bun:test";
import { act, cleanup, render, screen } from "@testing-library/react";
import { setTransport, type Transport } from "@/bridge/transport";
import { resetFeedsForTests } from "@/bridge/events";
import { STATUS_EVENT, type StreamStatus, type TaskSummary } from "@/bridge/types";
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

  it("does not mark a task viewed from selection alone", async () => {
    setTransport(transport());
    const selected = mock();
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={selected} />);

    const row = await screen.findByText("second task");
    await act(async () => row.closest("a")!.click());

    expect(taskOutcomeViews.isViewed(task("two", "second task"))).toBe(false);
  });

  it("shows a held task as an amber waiting ring with its reason", async () => {
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
    expect(dot.className).toContain("task-dot-waiting");
    expect(dot.getAttribute("title")).toBe("Waiting for another task");
    expect(row.closest("a")!.textContent).toContain("Waiting for another task");
  });

  it("names the worker that ran the task in the subtitle", async () => {
    setTransport(transport({ profiles: [{ id: "worker", label: "Night Shift" }] }));
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    await screen.findByText("second task");
    expect(screen.getAllByText(/^Night Shift/)).toHaveLength(2);
  });

  it("falls back to a sensible subtitle when no worker is recorded", async () => {
    setTransport(transport({ profiles: [] }));
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    await screen.findByText("second task");
    expect(screen.getAllByText(/^Unknown worker/)).toHaveLength(2);
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
