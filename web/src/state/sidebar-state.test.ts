// Ported from rust/crates/oga-ui/src/state/mod.rs `#[cfg(test)] mod tests`.

import { describe, expect, it } from "bun:test";
import type { BrokerSummaryState, TaskState, TaskSummary } from "@/bridge/types";
import type { EventFrame } from "./event-frame";
import {
  applyConnection,
  applyEventBatch,
  applyEventFrame,
  applyPreferences,
  applySummary,
  beginLoadMore,
  defaultSidebarState,
  filtersActive,
  filtersHideTasks,
  finishLoadMoreError,
  resetFilters,
  searchTerm,
  setArchiveFilter,
  setProjectFilter,
  setSearch,
  setSort,
  sidebarPreferencesFrom,
  summaryQuery,
  TASK_PAGE_SIZE,
  toggleSidebar,
  type SidebarState,
} from "./sidebar-state";

function task(id: string, cwd: string, state: TaskState): TaskSummary {
  return {
    id,
    profileId: "worker",
    model: "sonnet",
    cwd,
    state,
    promptPreview: `prompt ${id}`,
    title: `Task ${id}`,
    createdAt: "2026-07-29T10:00:00Z",
    updatedAt: "2026-07-29T10:00:00Z",
  };
}

describe("summary paging", () => {
  it("grows by one page", () => {
    const state: SidebarState = { ...defaultSidebarState(), tasksHasMore: true };
    expect(summaryQuery(state).limit).toBe(TASK_PAGE_SIZE);
    const begun = beginLoadMore(state);
    expect(begun).toBeDefined();
    const [next, query] = begun!;
    expect(next.loadedPages).toBe(2);
    expect(query.limit).toBe(TASK_PAGE_SIZE * 2);
  });

  it("retries the same page after a load-more failure", () => {
    const state: SidebarState = { ...defaultSidebarState(), tasksHasMore: true };
    const [begun] = beginLoadMore(state)!;
    const failed = finishLoadMoreError(begun, "offline");

    expect(failed.loadMoreFailed).toBe(true);
    expect(failed.loadedPages).toBe(1);
    expect(failed.isLoadingMore).toBe(false);
  });
});

function summaryOf(tasks: TaskSummary[]): BrokerSummaryState {
  return { profiles: [], tasks, tasksHasMore: false, memoryProjects: [] };
}

describe("applySummary", () => {
  it("keeps a cancelled task that fell off the fresh page", () => {
    const cancelled = task("cancelled", "/tmp/project", "cancelled");
    const state: SidebarState = { ...defaultSidebarState(), tasks: [cancelled] };

    // A busier task, not the cancelled one, fills the fresh page's window.
    const next = applySummary(state, summaryOf([task("busy", "/tmp/project", "running")]));

    expect(next.tasks.map((t) => t.id)).toEqual(["busy", "cancelled"]);
  });

  it("keeps a completed task that fell off the fresh page", () => {
    const completed = task("completed", "/tmp/project", "completed");
    const state: SidebarState = { ...defaultSidebarState(), tasks: [completed] };

    const next = applySummary(state, summaryOf([task("busy", "/tmp/project", "running")]));

    expect(next.tasks.map((t) => t.id)).toEqual(["busy", "completed"]);
  });

  it("drops a task once the caller explicitly asks for a different filter", () => {
    const archived = task("archived", "/tmp/project", "completed");
    let state: SidebarState = { ...defaultSidebarState(), tasks: [archived] };

    const [switched] = setArchiveFilter(state, "only");
    expect(switched.tasks).toEqual([]);

    // The merge must not resurrect it from the view the filter switch left behind.
    state = applySummary(switched, summaryOf([]));
    expect(state.tasks).toEqual([]);
  });
});

describe("filters", () => {
  it("only lights up off the default view", () => {
    let state = defaultSidebarState();
    expect(filtersActive(state)).toBe(false);

    state = setSort(state, "recent");
    expect(filtersActive(state)).toBe(true);
    expect(filtersHideTasks(state)).toBe(false);

    state = setProjectFilter(state, "/work/oga");
    expect(filtersHideTasks(state)).toBe(true);
  });

  it("resetting restores the default view and requeries", () => {
    const state: SidebarState = {
      ...defaultSidebarState(),
      archiveFilter: "only",
      grouping: "status",
      sort: "recent",
      projectFilter: "/work/oga",
    };

    const [next, changed] = resetFilters(state);
    expect(changed).toBe(true);
    expect(next).toEqual(defaultSidebarState());
    expect(resetFilters(next)[1]).toBe(false);
  });
});

describe("preferences", () => {
  it("a collapsed sidebar survives a reload", () => {
    const state = toggleSidebar(defaultSidebarState());
    const restored = applyPreferences(defaultSidebarState(), sidebarPreferencesFrom(state));
    expect(restored.sidebarCollapsed).toBe(true);
  });

  it("does not remember the search box between launches", () => {
    const state = setSearch(defaultSidebarState(), " ship ");
    expect(searchTerm(state)).toBe("ship");

    const restored = applyPreferences(defaultSidebarState(), sidebarPreferencesFrom(state));
    expect(searchTerm(restored)).toBe("");
  });
});

describe("event frames", () => {
  it("folds a batch once and refreshes only when a pointer needs it", () => {
    const state: SidebarState = { ...defaultSidebarState(), tasks: [task("task", "/tmp/project", "running")] };
    const [next, action] = applyEventBatch(state, {
      cursor: 2,
      streamFloor: 0,
      stale: false,
      pointers: [
        { id: 1, cursor: 1, taskId: "task", type: "started", kind: "lifecycle", state: "running", at: "2026-07-29T11:00:00Z", title: "Task", summary: "started" },
        { id: 2, cursor: 2, taskId: "task", type: "completed", kind: "lifecycle", state: "completed", at: "2026-07-29T12:00:00Z", title: "Done", summary: "done" },
      ],
    });

    expect(action).toBe("none");
    expect(next.eventCursor).toBe(2);
    expect(next.tasks[0].state).toBe("completed");
  });

  it("updates a known task and cursor from a pointer", () => {
    const state: SidebarState = {
      ...defaultSidebarState(),
      tasks: [task("task", "/tmp/project", "running")],
    };
    const frame: EventFrame = {
      kind: "task",
      pointer: {
        id: 1,
        cursor: 1,
        taskId: "task",
        type: "completed",
        kind: "lifecycle",
        state: "completed",
        at: "2026-07-29T11:00:00.000Z",
        title: "Finished",
        summary: "done",
      },
    };

    const [next, action] = applyEventFrame(state, frame);
    expect(action).toBe("none");
    expect(next.eventCursor).toBe(1);
    expect(next.tasks[0].state).toBe("completed");
    expect(next.tasks[0].title).toBe("Finished");
  });

  it("requests one refresh for repeated unknown-task pointers per reread interval", () => {
    const state = defaultSidebarState();
    const frame: EventFrame = {
      kind: "task",
      pointer: {
        id: 4,
        cursor: 4,
        taskId: "missing",
        type: "completed",
        kind: "lifecycle",
        state: "completed",
        at: "2026-07-29T11:00:00Z",
        title: "Finished",
        summary: "done",
      },
    };
    const [reread, firstAction] = applyEventFrame(state, frame);
    expect(firstAction).toBe("refresh");
    let repeatedState = reread;
    let repeatedRefreshes = 0;
    for (let cursor = 5; cursor <= 104; cursor += 1) {
      const [next, action] = applyEventFrame(repeatedState, { ...frame, pointer: { ...frame.pointer, cursor } });
      repeatedState = next;
      if (action === "refresh") repeatedRefreshes += 1;
    }
    const nextIntervalAction = applyEventFrame({ ...reread, lastReread: Date.now() - 15_000 }, frame)[1];
    expect(repeatedRefreshes).toBe(0);
    expect(nextIntervalAction).toBe("refresh");
  });

  it("reloads the list on a replayed ready frame", () => {
    const state: SidebarState = { ...defaultSidebarState(), eventCursor: 12 };
    const frame: EventFrame = {
      kind: "ready",
      frame: { version: 1, cursor: 40, streamFloor: 41, tasks: [], kinds: [], agents: false, stale: true },
    };

    const [next, action] = applyEventFrame(state, frame);
    expect(action).toBe("refresh");
    expect(next.eventCursor).toBe(40);
  });

  it("rereads the list on a slow beat while the log keeps moving", () => {
    let state = defaultSidebarState();
    const cursor = (value: number): EventFrame => ({ kind: "cursor", frame: { cursor: value } });

    let action: string;
    [state, action] = applyEventFrame(state, cursor(1));
    expect(action).toBe("refresh");
    [state, action] = applyEventFrame(state, cursor(2));
    expect(action).toBe("none");
    [state, action] = applyEventFrame(state, cursor(3));
    expect(action).toBe("none");
    expect(state.eventCursor).toBe(3);

    state = { ...state, lastReread: Date.now() - 15_000 };

    [, action] = applyEventFrame(state, cursor(4));
    expect(action).toBe("refresh");
  });
});

describe("connection", () => {
  it("reads a dropped shell stream as reconnecting", () => {
    let state: SidebarState = { ...defaultSidebarState(), connection: "connected" };

    state = applyConnection(state, {
      connected: false,
      cursor: 0,
      streamFloor: 0,
      stale: false,
      error: "broker restarting",
    });

    expect(state.connection).toBe("reconnecting");
    expect(state.error).toBe("broker restarting");

    state = applyConnection(state, { connected: true, cursor: 4, streamFloor: 0, stale: false });
    expect(state.connection).toBe("connected");
  });
});
