// Ported from rust/crates/oga-ui/src/sidebar/mod.rs `#[cfg(test)] mod tests`.

import { describe, expect, it } from "bun:test";
import type { TaskState, TaskSummary } from "@/bridge/types";
import type { SidebarState } from "./sidebar-state";
import { defaultSidebarState } from "./sidebar-state";
import {
  activeProjectName,
  matching,
  neighborAfterRemoving,
  organize,
  projectionEmptyMessage,
  projectionFromState,
  projectionListedTaskIds,
  projectionTaskCount,
  projectName,
  projects,
  projectTree,
  taskProjectLabel,
  virtualListVisibleRange,
  newVirtualList,
  withScrollOffset,
  withViewportHeight,
} from "./sidebar-projection";

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

describe("projects", () => {
  it("keeps first-seen order and counts", () => {
    const tasks = [
      task("one", "/work/oga", "completed"),
      task("two", "/work/site", "completed"),
      task("three", "/work/oga", "completed"),
    ];
    const list = projects(tasks);
    expect(list.map((project) => project.name)).toEqual(["oga", "site"]);
    expect(list.map((project) => project.count)).toEqual([2, 1]);
  });
});

describe("organize", () => {
  it("falls back to all tasks for a stale project filter", () => {
    const tasks = [task("one", "/work/oga", "completed")];
    const groups = organize(tasks, "/gone", "none", "recent");
    expect(groups[0].tasks).toHaveLength(1);
    expect(activeProjectName(tasks, "/gone")).toBeUndefined();
  });

  it("runs the project filter before grouping", () => {
    const tasks = [
      task("oga-1", "/work/oga", "completed"),
      task("site", "/work/site", "completed"),
      task("oga-2", "/work/oga", "running"),
    ];
    const groups = organize(tasks, "/work/oga", "project", "recent");
    expect(groups).toHaveLength(1);
    expect(groups[0].tasks.map((entry) => entry.id)).toEqual(["oga-1", "oga-2"]);
  });

  it("uses the fixed attention order for status grouping", () => {
    const tasks = [
      task("failed", "/work/oga", "failed"),
      task("running", "/work/oga", "running"),
      task("needs", "/work/oga", "needs_input"),
      task("blocked", "/work/oga", "blocked"),
    ];
    const groups = organize(tasks, undefined, "status", "recent");
    expect(groups.map((group) => group.id)).toEqual(["needs_input", "blocked", "running", "failed"]);
  });

  it("includes children and stops cycles for parent grouping", () => {
    const root = { ...task("root", "/work/oga", "completed"), title: "Ship the page\nwith details" };
    const child = { ...task("child", "/work/oga", "completed"), parentTaskId: "root" };
    const groups = organize([root, child], undefined, "parent", "recent");
    expect(groups).toHaveLength(1);
    expect(groups[0].title).toBe("Ship the page");
    expect(groups[0].tasks.map((entry) => entry.id)).toEqual(["root", "child"]);
  });

  it("nests project siblings under one parent", () => {
    const tasks = [
      task("oga", "/work/mono/oga", "completed"),
      task("site", "/work/mono/site", "completed"),
    ];
    const groups = organize(tasks, undefined, "project", "recent");
    const nodes = projectTree(groups);
    expect(nodes).toHaveLength(1);
    expect(nodes[0].type === "parent" ? nodes[0].id : nodes[0].group.id).toBe("/work/mono");
  });

  it("puts attention before live and settled tasks under priority sort", () => {
    const tasks = [
      task("done", "/work/oga", "completed"),
      task("running", "/work/oga", "running"),
      task("failed", "/work/oga", "failed"),
      task("needs", "/work/oga", "needs_input"),
      task("blocked", "/work/oga", "blocked"),
    ];
    const groups = organize(tasks, undefined, "none", "priority");
    expect(groups[0].tasks.map((entry) => entry.id)).toEqual(["needs", "blocked", "failed", "running", "done"]);
  });

  it("sorts by when work started under newest-first", () => {
    const started = { ...task("started", "/work/oga", "completed"), createdAt: "2026-07-29T08:00:00Z", updatedAt: "2026-07-30T09:00:00Z" };
    const newer = { ...task("newer", "/work/oga", "completed"), createdAt: "2026-07-29T09:00:00Z", updatedAt: "2026-07-29T09:00:00Z" };
    const groups = organize([started, newer], undefined, "none", "recent");
    expect(groups[0].tasks.map((entry) => entry.id)).toEqual(["newer", "started"]);
  });

  it("treats unparseable dates as old under updated sort", () => {
    const invalid = { ...task("invalid", "/work/oga", "completed"), updatedAt: "not-a-date" };
    const old = { ...task("old", "/work/oga", "completed"), updatedAt: "2026-07-29T08:00:00Z" };
    const fresh = { ...task("new", "/work/oga", "completed"), updatedAt: "2026-07-30T08:00:00Z" };
    const groups = organize([invalid, old, fresh], undefined, "none", "updated");
    expect(groups[0].tasks.map((entry) => entry.id)).toEqual(["new", "old", "invalid"]);
  });
});

describe("virtual list", () => {
  it("keeps the visible range bounded for large lists", () => {
    const list = withScrollOffset(withViewportHeight(newVirtualList(10_000), 400), 5_000);
    const [start, end] = virtualListVisibleRange(list);
    expect(start).toBeGreaterThan(0);
    expect(end - start).toBeLessThan(100);
    expect(end).toBeLessThanOrEqual(10_000);
  });
});

describe("removing a task", () => {
  it("prefers the next neighbor", () => {
    const ids = ["first", "second", "third"];
    expect(neighborAfterRemoving("second", ids)).toBe("third");
    expect(neighborAfterRemoving("third", ids)).toBe("second");
    expect(neighborAfterRemoving("only", ["only"])).toBeUndefined();
  });
});

describe("search", () => {
  it("matches titles whatever the case", () => {
    const shipping = { ...task("shipping", "/work/oga", "running"), title: "Ship the sidebar" };
    const other = { ...task("other", "/work/oga", "running"), title: "Rename the broker" };
    const state: SidebarState = { ...defaultSidebarState(), tasks: [shipping, other], search: "  SIDEBAR ", grouping: "none" };

    const projection = projectionFromState(state);
    expect(projectionListedTaskIds(projection, state.collapsed, state.grouping)).toEqual(["shipping"]);
  });

  it("falls back to the prompt when a task has no title", () => {
    const untitled = { ...task("untitled", "/work/oga", "running"), title: undefined, promptPreview: "Port the sidebar header" };
    expect(matching([untitled], "header")).toHaveLength(1);
  });

  it("keeps every task when the box is empty", () => {
    expect(matching([task("one", "/work/oga", "running")], "")).toHaveLength(1);
  });
});

describe("empty messages", () => {
  it("says whether a search or a filter caused it", () => {
    const empty = defaultSidebarState();
    expect(projectionEmptyMessage(projectionFromState(empty), empty)).toBe(
      "No tasks yet",
    );

    const searched: SidebarState = {
      ...defaultSidebarState(),
      tasks: [task("one", "/work/oga", "running")],
      search: "nothing like this",
    };
    expect(projectionEmptyMessage(projectionFromState(searched), searched)).toBe("No tasks match your search.");

    const filtered: SidebarState = { ...defaultSidebarState(), archiveFilter: "only" };
    expect(projectionEmptyMessage(projectionFromState(filtered), filtered)).toBe("No tasks match your filters.");

    const both: SidebarState = { ...filtered, search: "nothing" };
    expect(projectionEmptyMessage(projectionFromState(both), both)).toBe(
      "No tasks match your search and filters.",
    );
  });

  it("shows no empty message once the list has rows", () => {
    const state: SidebarState = { ...defaultSidebarState(), tasks: [task("one", "/work/oga", "running")] };
    expect(projectionEmptyMessage(projectionFromState(state), state)).toBeUndefined();
  });
});

describe("unread projection", () => {
  it("lists only visible rows", () => {
    const state: SidebarState = {
      ...defaultSidebarState(),
      archiveFilter: "active",
      tasks: [task("one", "/work/oga", "completed")],
    };
    const projection = projectionFromState(state);
    expect(projectionListedTaskIds(projection, state.collapsed, state.grouping)).toEqual(["one"]);
  });
});

it("counts tasks across every group", () => {
  const state: SidebarState = {
    ...defaultSidebarState(),
    tasks: [task("one", "/work/oga", "running"), task("two", "/work/site", "running")],
    grouping: "project",
  };
  expect(projectionTaskCount(projectionFromState(state))).toBe(2);
});

it("names a project from its path", () => {
  expect(projectName("/work/oga/")).toBe("oga");
  expect(projectName("/")).toBe("/");
});

describe("taskProjectLabel", () => {
  it("names the project a task runs in", () => {
    expect(taskProjectLabel(task("one", "/work/oga", "running"))).toBe("oga");
  });

  it("adds the branch when the work has a copy of its own", () => {
    const copy = { ...task("one", "/copies/abc", "running"), originCwd: "/work/oga", branch: "fix-login" };
    expect(taskProjectLabel(copy)).toBe("oga/fix-login");
  });

  it("keeps a long name short enough for a row", () => {
    const copy = {
      ...task("one", "/copies/abc", "running"),
      originCwd: "/work/oga",
      branch: "show-the-project-on-every-task-row",
    };
    expect(taskProjectLabel(copy)).toBe("oga/show-the-project-…");
  });
});
