import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, render } from "@testing-library/react";
import type { TaskState } from "@/bridge/types";
import { TaskStatusDot, taskStatusLook } from "./TaskStatusDot";

const ALL_STATES: TaskState[] = [
  "queued",
  "preparing_checkout",
  "removing_checkout",
  "pending",
  "running",
  "needs_input",
  "answered",
  "blocked",
  "completed",
  "failed",
  "cancelled",
];

describe("taskStatusLook", () => {
  afterEach(cleanup);

  it("gives every state a look", () => {
    for (const state of ALL_STATES) expect(taskStatusLook(state)).toBeDefined();
  });

  it("groups waiting states together", () => {
    expect(taskStatusLook("queued")).toBe("waiting");
    expect(taskStatusLook("preparing_checkout")).toBe("waiting");
    expect(taskStatusLook("removing_checkout")).toBe("waiting");
    expect(taskStatusLook("pending")).toBe("waiting");
    expect(taskStatusLook("answered")).toBe("waiting");
  });

  it("groups failed and blocked as a problem", () => {
    expect(taskStatusLook("failed")).toBe("problem");
    expect(taskStatusLook("blocked")).toBe("problem");
  });

  it("groups completed and cancelled as settled", () => {
    expect(taskStatusLook("completed")).toBe("settled");
    expect(taskStatusLook("cancelled")).toBe("settled");
  });

  it("gives running and needs_input their own look", () => {
    expect(taskStatusLook("running")).toBe("running");
    expect(taskStatusLook("needs_input")).toBe("needs_input");
  });
});

describe("TaskStatusDot", () => {
  afterEach(cleanup);

  it("renders the same look for a state regardless of caller", () => {
    const { container: sidebar } = render(<TaskStatusDot state="running" label="Running" unread />);
    const { container: header } = render(<TaskStatusDot state="running" label="Running" />);

    const sidebarClasses = sidebar.querySelector(".task-dot")!.className.split(" ");
    const headerClasses = header.querySelector(".task-dot")!.className.split(" ");
    expect(sidebarClasses).toContain("task-dot-look-running");
    expect(headerClasses).toContain("task-dot-look-running");
  });

  it("fills for an unread outcome and hollows once viewed", () => {
    const { container: unread } = render(<TaskStatusDot state="completed" label="Completed" unread />);
    const { container: viewed } = render(<TaskStatusDot state="completed" label="Completed" unread={false} />);

    expect(unread.querySelector(".task-dot")!.className).toContain("task-dot-unread");
    expect(viewed.querySelector(".task-dot")!.className).toContain("task-dot-viewed");
  });

  it("defaults to viewed, since the header and composer always show the open task", () => {
    const { container } = render(<TaskStatusDot state="failed" label="Failed" />);
    expect(container.querySelector(".task-dot")!.className).toContain("task-dot-viewed");
  });

  it("exposes its status as an accessible label unless a sibling already carries it", () => {
    const { container: labelled } = render(<TaskStatusDot state="failed" label="Failed" />);
    const dot = labelled.querySelector(".task-dot")!;
    expect(dot.getAttribute("role")).toBe("img");
    expect(dot.getAttribute("aria-label")).toBe("Failed");

    const { container: decorative } = render(<TaskStatusDot state="failed" label="Failed" decorative />);
    const hiddenDot = decorative.querySelector(".task-dot")!;
    expect(hiddenDot.getAttribute("aria-hidden")).toBe("true");
    expect(hiddenDot.getAttribute("role")).toBeNull();
  });
});
