import { describe, expect, it } from "bun:test";
import { routeFromPath, routePath } from "@/router";

describe("routeFromPath", () => {
  it("parses the root as home", () => {
    expect(routeFromPath("/")).toEqual({ kind: "home" });
  });

  it("parses a task id, ignoring a trailing slash and query string", () => {
    expect(routeFromPath("/tasks/task-123/?view=activity")).toEqual({ kind: "task", id: "task-123" });
  });

  it("parses settings", () => {
    expect(routeFromPath("/settings")).toEqual({ kind: "settings" });
  });

  it("falls back to not-found for anything else", () => {
    expect(routeFromPath("/missing")).toEqual({ kind: "not-found" });
  });

  it("falls back to not-found for a bare /tasks/ with no id", () => {
    expect(routeFromPath("/tasks/")).toEqual({ kind: "not-found" });
  });
});

describe("routePath", () => {
  it("renders each route back to a path", () => {
    expect(routePath({ kind: "home" })).toBe("/");
    expect(routePath({ kind: "task", id: "abc" })).toBe("/tasks/abc");
    expect(routePath({ kind: "settings" })).toBe("/settings");
    expect(routePath({ kind: "not-found" })).toBe("/");
  });
});
