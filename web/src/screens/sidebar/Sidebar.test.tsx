import { afterEach, beforeEach, describe, expect, it, mock } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { setTransport, type Transport } from "@/bridge/transport";
import { resetFeedsForTests } from "@/bridge/events";
import { EVENT_BATCH_EVENT, STATUS_EVENT, WINDOW_VISIBILITY_EVENT, type EventBatch, type StateQuery, type StreamStatus, type TaskSummary, type WindowVisibility } from "@/bridge/types";
import { resetWindowVisibilityForTests } from "@/lib/window-visibility";
import { SidebarController } from "@/state/sidebar-state";
import { resetTaskOutcomeViewsForTests } from "@/state/task-outcome-views";
import Sidebar from "./Sidebar";

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

const WORKER = { id: "worker", label: "Worker", provider: "claude", model: "sonnet", enabled: true, env: {}, capabilities: [] };

interface Answer {
  tasks: TaskSummary[];
  groupCounts?: Record<string, number>;
}

/** A broker whose summary answers depend on the query, as the real one's do. */
function broker(answer: (query: StateQuery) => Answer, options: { profiles?: unknown[]; listen?: Transport["listen"] } = {}): Transport {
  return {
    invoke: async (command, args) => {
      if (command === "broker_stream_status") {
        // SAFETY: this fixture returns the StreamStatus shape the sidebar reads.
        return { connected: true, cursor: 0, streamFloor: 0, stale: false } as never;
      }
      if (command !== "broker_call") throw { message: `no handler for ${command}` };
      // SAFETY: broker_call requests always carry a call object.
      const call = args?.call as { call: string; query?: StateQuery };
      if (call.call !== "summary") throw { message: `no handler for ${call.call}` };
      const { tasks, groupCounts } = answer(call.query!);
      // SAFETY: this fixture returns the BrokerSummaryState shape SidebarController reads.
      return {
        profiles: options.profiles ?? [WORKER],
        tasks,
        tasksHasMore: false,
        groupCounts,
        profileFailures: [],
        grants: [],
        memoryProjects: [],
      } as never;
    },
    listen: options.listen ?? (() => {}),
  };
}

/** The list read, as opposed to the one-row reads that count every active task. */
function listRead(query: StateQuery): boolean {
  return query.limit !== 1;
}

function views() {
  return within(screen.getByRole("navigation", { name: "Task views" }));
}

describe("the sidebar", () => {
  beforeEach(() => {
    resetFeedsForTests();
    resetWindowVisibilityForTests();
    resetTaskOutcomeViewsForTests();
    window.localStorage.clear();
  });
  afterEach(cleanup);

  it("counts what needs you across every project, even while one project is shown", async () => {
    const oga = task("one", "first", { cwd: "/work/oga", state: "needs_input" });
    const money = task("two", "second", { cwd: "/work/money", state: "failed" });
    setTransport(broker((query) => {
      if (query.group === "project") return { tasks: [], groupCounts: { "/work/oga": 1, "/work/money": 3 } };
      if (!listRead(query)) return { tasks: [], groupCounts: { needs_input: 1, failed: 1, running: 2, completed: 40 } };
      // The list read is narrowed to the project shown, totals included.
      const tasks = [oga, money].filter((entry) => query.project === undefined || entry.cwd === query.project);
      const groupCounts: Record<string, number> = {};
      for (const entry of tasks) groupCounts[entry.state] = (groupCounts[entry.state] ?? 0) + 1;
      return { tasks, groupCounts };
    }));
    const showTasks = mock();
    render(<Sidebar sidebarController={new SidebarController()} onShowTasks={showTasks} tasksShown />);

    await waitFor(() => expect(views().getByRole("button", { name: /Needs you/ }).textContent).toContain("2"));
    expect(views().getByRole("button", { name: /In progress/ }).textContent).toContain("2");

    fireEvent.click(await views().findByRole("button", { name: /^oga/ }));
    expect(showTasks).toHaveBeenCalledOnce();
    await waitFor(() => expect(views().getByRole("button", { name: /^oga/ }).getAttribute("aria-current")).toBe("page"));
    expect(views().getByRole("button", { name: /Needs you/ }).textContent).toContain("2");
  });

  it("tells same-named projects apart and keeps their order when one is picked", async () => {
    const first = task("a", "first run", { cwd: "/runs/claude/scope-guard" });
    const second = task("b", "second run", { cwd: "/runs/opencode/scope-guard", updatedAt: "2026-08-31T09:00:00Z" });
    setTransport(broker((query) => {
      if (query.group === "project") return { tasks: [], groupCounts: { [first.cwd]: 1, [second.cwd]: 1 } };
      if (!listRead(query)) return { tasks: [] };
      return { tasks: [first, second].filter((entry) => query.project === undefined || entry.cwd === query.project) };
    }));
    const controller = new SidebarController();
    render(<Sidebar sidebarController={controller} onShowTasks={mock()} tasksShown />);

    const projectButtons = () => views().getAllByRole("button").filter((button) => button.textContent?.startsWith("scope-guard"));
    const projectNames = () => projectButtons().map((button) => button.textContent);
    await waitFor(() => expect(projectNames()).toEqual(["scope-guard claude 1 task", "scope-guard opencode 1 task"]));

    fireEvent.click(projectButtons()[1]);
    await waitFor(() => expect(controller.snapshot.tasks.map((entry) => entry.id)).toEqual(["b"]));
    expect(projectNames()).toEqual(["scope-guard claude 1 task", "scope-guard opencode 1 task"]);
  });

  it("points a new user at worker settings without reading the model catalogue", async () => {
    const calls: string[] = [];
    const empty = broker(() => ({ tasks: [] }), { profiles: [] });
    setTransport({
      ...empty,
      invoke: async (command, args) => {
        // SAFETY: broker_call requests always carry a call object; other commands carry none.
        const call = args?.call as { call: string } | undefined;
        calls.push(call?.call ?? command);
        return empty.invoke(command, args);
      },
    });
    const openSettings = mock();
    render(<Sidebar sidebarController={new SidebarController()} onOpenSettings={openSettings} />);

    fireEvent.click(await screen.findByRole("button", { name: /Add a worker to start/ }));
    expect(openSettings).toHaveBeenCalledWith("workers");
    expect(calls).not.toContain("modelSettings");
  });

  it("lists each new task as it starts, including two started back to back", async () => {
    const tasks = [task("one", "first task")];
    let batchListener: ((batch: EventBatch) => void) | undefined;
    setTransport(broker(() => ({ tasks: [...tasks] }), {
      listen: (event, handle) => {
        if (event !== EVENT_BATCH_EVENT) return;
        // SAFETY: the bridge forwards each event's registered payload type.
        batchListener = handle as (batch: EventBatch) => void;
      },
    }));
    const controller = new SidebarController();
    render(<Sidebar sidebarController={controller} />);
    await waitFor(() => expect(controller.snapshot.tasks).toHaveLength(1));

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
    await waitFor(() => expect(controller.snapshot.tasks.map((entry) => entry.id)).toContain("two"));
    start("three", "third task", 2);
    await waitFor(() => expect(controller.snapshot.tasks.map((entry) => entry.id)).toEqual(["three", "two", "one"]));
  });

  it("keeps a newer stream status when the startup read returns late", async () => {
    let resolveStatus: ((status: StreamStatus) => void) | undefined;
    let statusListener: ((status: StreamStatus) => void) | undefined;
    const statusResult = new Promise<StreamStatus>((resolve) => {
      resolveStatus = resolve;
    });
    const listed = broker(() => ({ tasks: [task("one", "first task"), task("two", "second task")] }));
    setTransport({
      invoke: async (command, args) => {
        if (command === "broker_stream_status") {
          // SAFETY: the status this fixture resolves has the StreamStatus shape the sidebar reads.
          return statusResult as never;
        }
        return listed.invoke(command, args);
      },
      listen: (event, handle) => {
        if (event !== STATUS_EVENT) return;
        // SAFETY: the bridge forwards each event's registered payload type.
        statusListener = handle as (status: StreamStatus) => void;
      },
    });
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} />);
    await waitFor(() => expect(controller.snapshot.tasks).toHaveLength(2));

    await act(async () => {
      statusListener?.({ connected: true, cursor: 0, streamFloor: 0, stale: false });
      resolveStatus?.({ connected: false, cursor: 0, streamFloor: 0, stale: false });
      await Promise.resolve();
    });

    expect(controller.snapshot.connection).toBe("connected");
  });

  it("holds list changes while the window is hidden and catches up when it is shown", async () => {
    let tasks: TaskSummary[] = [task("one", "first task")];
    let reads = 0;
    let batchListener: ((batch: EventBatch) => void) | undefined;
    let visibilityListener: ((visibility: WindowVisibility) => void) | undefined;
    setTransport(broker((query) => {
      if (listRead(query)) reads += 1;
      return { tasks };
    }, {
      listen: (event, handle) => {
        if (event === EVENT_BATCH_EVENT) {
          // SAFETY: the bridge forwards each event's registered payload type.
          batchListener = handle as (batch: EventBatch) => void;
        }
        if (event === WINDOW_VISIBILITY_EVENT) {
          // SAFETY: the bridge forwards each event's registered payload type.
          visibilityListener = handle as (visibility: WindowVisibility) => void;
        }
      },
    }));

    const controller = new SidebarController();
    render(<Sidebar sidebarController={controller} />);
    await waitFor(() => expect(controller.snapshot.tasks).toHaveLength(1));
    const readsWhileVisible = reads;

    act(() => visibilityListener?.({ visible: false }));
    tasks = [task("one", "finished while away", { state: "completed", updatedAt: "2026-08-31T11:00:00Z" })];
    act(() => batchListener?.({
      cursor: 9,
      streamFloor: 0,
      stale: false,
      pointers: [{ id: 9, cursor: 9, taskId: "one", type: "completed", kind: "lifecycle", state: "completed", at: "2026-08-31T11:00:00Z", title: "finished while away", summary: "" }],
    }));
    await act(() => new Promise((resolve) => setTimeout(resolve, 150)));

    expect(controller.snapshot.tasks.map((entry) => entry.promptPreview)).toEqual(["first task"]);
    expect(reads).toBe(readsWhileVisible);

    act(() => visibilityListener?.({ visible: true }));

    await waitFor(() => expect(controller.snapshot.tasks.map((entry) => entry.promptPreview)).toEqual(["finished while away"]));
    expect(reads).toBe(readsWhileVisible + 1);
  });
});
