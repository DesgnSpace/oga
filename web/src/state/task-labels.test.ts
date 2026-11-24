import { describe, expect, it } from "bun:test";
import type { TaskState, TaskSummary } from "@/bridge/types";
import { matching, projectLabel, projectList, projectName, projectParents, recentProjectIds } from "./task-labels";

function task(id: string, cwd: string, state: TaskState = "running"): TaskSummary {
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

describe("search", () => {
  it("matches titles whatever the case", () => {
    const shipping = { ...task("shipping", "/work/oga"), title: "Ship the sidebar" };
    const other = { ...task("other", "/work/oga"), title: "Rename the broker" };
    expect(matching([shipping, other], "SIDEBAR").map((found) => found.id)).toEqual(["shipping"]);
  });

  it("falls back to the prompt when a task has no title", () => {
    const untitled = { ...task("untitled", "/work/oga"), title: undefined, promptPreview: "Port the sidebar header" };
    expect(matching([untitled], "header")).toHaveLength(1);
  });

  it("keeps every task when the box is empty", () => {
    expect(matching([task("one", "/work/oga")], "")).toHaveLength(1);
  });
});

it("names a project from its path", () => {
  expect(projectName("/work/oga/")).toBe("oga");
  expect(projectName("/")).toBe("/");
});

describe("projects that share a folder name", () => {
  const evalRuns = [
    "/evals/runs/1858/claude-sonnet/orchestrate",
    "/evals/runs/1858/opencode-tidepool/orchestrate",
    "/evals/runs/1858/claude-sonnet/fix-bug",
    "/work/oga",
  ];

  it("tells them apart by the nearest parent folder that differs", () => {
    const parents = projectParents(evalRuns);
    expect(projectLabel(evalRuns[0], parents)).toBe("claude-sonnet/orchestrate");
    expect(projectLabel(evalRuns[1], parents)).toBe("opencode-tidepool/orchestrate");
    expect(projectLabel(evalRuns[2], parents)).toBe("fix-bug");
    expect(projectLabel(evalRuns[3], parents)).toBe("oga");
  });

  it("climbs past a parent folder they share, and only for the ones that share it", () => {
    const paths = ["/clients/acme/app", "/clients/globex/app", "/a/x/app", "/b/x/app"];
    const parents = projectParents(paths);
    expect(paths.map((path) => projectLabel(path, parents))).toEqual(["acme/app", "globex/app", "a/x/app", "b/x/app"]);
  });
});

describe("the project list", () => {
  it("lists every project once with its true total, recent work first and the rest by name", () => {
    const tasks = [task("1", "/work/oga"), task("2", "/work/money"), task("3", "/work/oga")];
    const totals = { "/work/oga": 41, "/work/money": 6, "/work/zeta": 2, "/work/abuna": 9 };

    const listed = projectList(recentProjectIds(tasks), totals);

    expect(listed.map((project) => [project.name, project.count])).toEqual([
      ["oga", 41],
      ["money", 6],
      ["abuna", 9],
      ["zeta", 2],
    ]);
  });

  it("gives two same-named projects different labels in the list", () => {
    const totals = { "/runs/a/scope-guard": 1, "/runs/b/scope-guard": 1 };
    const listed = projectList([], totals);
    expect(listed.map((project) => `${project.name} ${project.parent}`)).toEqual(["scope-guard a", "scope-guard b"]);
  });
});
