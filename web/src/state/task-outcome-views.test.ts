import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import type { TaskHoldView, TaskState, TaskSummary } from "@/bridge/types";
import { isTaskWaiting, MAX_TASK_OUTCOME_VIEWS, TaskOutcomeViewStore, taskDotTone } from "./task-outcome-views";

function task(id: string, state: TaskState, extra: Partial<TaskSummary> = {}): TaskSummary {
  return {
    id,
    profileId: "worker",
    model: "sonnet",
    cwd: "/repo",
    state,
    promptPreview: id,
    createdAt: "2026-09-08T10:00:00Z",
    updatedAt: "2026-09-08T10:00:00Z",
    ...extra,
  };
}

describe("task outcome views", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => localStorage.clear());

  it("starts unread and becomes viewed when opened", () => {
    const store = new TaskOutcomeViewStore();
    const completed = task("task", "completed");

    store.observeTasks([completed]);
    expect(store.isViewed(completed)).toBe(false);

    store.markViewed(completed);
    expect(store.isViewed(completed)).toBe(true);
  });

  it("marks a later outcome unread again", () => {
    const store = new TaskOutcomeViewStore();
    const running = task("task", "running");
    const failed = task("task", "failed", { error: "worker stopped" });

    store.markViewed(running);
    store.observeTasks([failed]);

    expect(store.isViewed(failed)).toBe(false);
  });

  it("restores viewed outcomes from storage on startup", () => {
    const completed = task("task", "completed");
    const first = new TaskOutcomeViewStore();
    first.markViewed(completed);

    const restarted = new TaskOutcomeViewStore();

    expect(restarted.isViewed(completed)).toBe(true);
  });

  it("views an outcome that arrives for the active task", () => {
    const store = new TaskOutcomeViewStore();
    const failed = task("task", "failed", { error: "worker stopped" });

    store.setActiveTask(failed.id);
    store.observeTasks([failed]);
    store.clearActiveTask(failed.id);

    expect(store.isViewed(failed)).toBe(true);
  });

  it("keeps rapid task switches isolated", () => {
    const store = new TaskOutcomeViewStore();
    const first = task("first", "running");
    const second = task("second", "completed");

    store.setActiveTask(first.id);
    store.observeTasks([first]);
    store.setActiveTask(second.id);
    store.observeTasks([second]);
    store.clearActiveTask(second.id);

    expect(store.isViewed(first)).toBe(true);
    expect(store.isViewed(second)).toBe(true);

    const firstNeedsInput = task("first", "needs_input", { question: "Choose a path" });
    store.observeTasks([firstNeedsInput]);

    expect(store.isViewed(firstNeedsInput)).toBe(false);
  });

  it("keeps persisted markers bounded", () => {
    const store = new TaskOutcomeViewStore();
    const tasks = Array.from({ length: MAX_TASK_OUTCOME_VIEWS + 1 }, (_, index) => task(`task-${index}`, "completed"));

    store.observeTasks(tasks);

    // SAFETY: the store writes this key as a JSON array of marker records.
    const saved = JSON.parse(localStorage.getItem("taskOutcomeViews") ?? "[]") as unknown[];
    expect(saved).toHaveLength(MAX_TASK_OUTCOME_VIEWS);
  });

  it("assigns semantic tones without collapsing task states", () => {
    expect(taskDotTone("completed")).toBe("success");
    expect(taskDotTone("failed")).toBe("danger");
    expect(taskDotTone("needs_input")).toBe("info");
    expect(taskDotTone("pending")).toBe("warning");
    expect(taskDotTone("blocked")).toBe("warning");
    expect(taskDotTone("running")).toBe("muted");
    expect(taskDotTone("queued")).toBe("muted");
    expect(taskDotTone("cancelled")).toBe("muted");
  });

  it("marks only held or dependency-blocked tasks as waiting", () => {
    const hold: TaskHoldView = {
      kind: "dependency",
      waitingOn: "blocker",
      note: "Waiting for another task",
      expiresAt: "2026-09-09T10:00:00Z",
    };

    expect(isTaskWaiting(task("held", "pending", { hold }))).toBe(true);
    expect(isTaskWaiting(task("pending", "pending"))).toBe(false);
    expect(isTaskWaiting(task("dependency", "blocked", {
      completion: { blocked: true, code: "cancelled", dependencyBlocked: true },
    }))).toBe(true);
    expect(isTaskWaiting(task("worker-error", "blocked", {
      completion: { blocked: true, code: "worker_error" },
    }))).toBe(false);
    expect(isTaskWaiting(task("question", "needs_input", { hold }))).toBe(false);
  });
});
