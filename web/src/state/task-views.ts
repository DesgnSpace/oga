import type { TaskState, TaskSummary } from "@/bridge/types";

export type TaskView = "recent" | "needs-you" | "in-progress" | "finished" | "archived";

export interface TaskViewInfo {
  id: TaskView;
  label: string;
  description: string;
  /** The states the view lists; `undefined` lists every state. */
  states?: readonly TaskState[];
}

const NEEDS_YOU_STATES: readonly TaskState[] = ["needs_input", "failed", "blocked"];

const IN_PROGRESS_STATES: readonly TaskState[] = [
  "running",
  "queued",
  "pending",
  "answered",
  "preparing_checkout",
  "removing_checkout",
];

const FINISHED_STATES: readonly TaskState[] = ["completed", "cancelled"];

export const TASK_VIEWS: readonly TaskViewInfo[] = [
  { id: "recent", label: "Recent", description: "What needs you first, then the latest work" },
  { id: "needs-you", label: "Needs you", description: "Questions to answer, and failed or blocked tasks", states: NEEDS_YOU_STATES },
  { id: "in-progress", label: "In progress", description: "Running now or waiting to start", states: IN_PROGRESS_STATES },
  { id: "finished", label: "Finished", description: "Completed or cancelled", states: FINISHED_STATES },
  { id: "archived", label: "Archived", description: "Kept out of the other views. Restore one from its menu." },
];

export function taskView(id: TaskView): TaskViewInfo {
  return TASK_VIEWS.find((view) => view.id === id) ?? TASK_VIEWS[0];
}

export function needsYou(task: Pick<TaskSummary, "state">): boolean {
  return NEEDS_YOU_STATES.includes(task.state);
}

export function inProgress(task: Pick<TaskSummary, "state">): boolean {
  return IN_PROGRESS_STATES.includes(task.state);
}

export function viewIncludes(id: TaskView, task: Pick<TaskSummary, "state">): boolean {
  const states = taskView(id).states;
  return states === undefined || states.includes(task.state);
}

/** The view's true total from per-state counts, or `undefined` before any arrive. */
export function viewTotal(id: TaskView, stateCounts: Readonly<Record<string, number>>): number | undefined {
  const counts = Object.entries(stateCounts);
  if (counts.length === 0) return undefined;
  const states = taskView(id).states;
  if (states === undefined) return counts.reduce((total, [, count]) => total + count, 0);
  return states.reduce((total, state) => total + (stateCounts[state] ?? 0), 0);
}
