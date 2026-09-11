// Small display helpers shared across the task detail screen.
// Ported from rust/crates/oga-ui/src/sidebar/view.rs (short_model) and
// rust/crates/oga-ui/src/state/mod.rs (project_name), which are not yet
// exposed from a shared module — reimplemented here in one line each.

import type { TaskHoldView } from "@/bridge/types";
import { absoluteTime, relativeTime } from "@/ui/time";

export function shortModel(model: string): string {
  const parts = model.split("/");
  return parts[parts.length - 1] ?? model;
}

/**
 * The effort chip text: the actual effort when known, else the requested one.
 * When they differ, the tooltip carries the requested value.
 */
export function effortDisplay(
  task: { effort?: string; effortActual?: string },
): { label: string; title?: string } | undefined {
  const label = task.effortActual ?? task.effort;
  if (!label) return undefined;
  const differs = task.effortActual !== undefined && task.effort !== undefined && task.effortActual !== task.effort;
  return { label, title: differs ? `Requested effort: ${task.effort}` : undefined };
}

const TASK_STATE_LABELS: Record<string, string> = {
  queued: "Queued",
  preparing_checkout: "Preparing checkout",
  removing_checkout: "Removing checkout",
  pending: "Waiting",
  running: "Running",
  needs_input: "Needs input",
  answered: "Answered",
  blocked: "Blocked",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
};

export function taskStateLabel(state: string): string {
  return TASK_STATE_LABELS[state] ?? "Unknown";
}

/**
 * True for a wait a row should spell out and a page should offer a way out of:
 * the connection, the account, or a start time somebody picked.
 */
export function isExplainedWait(hold: TaskHoldView | undefined): boolean {
  return hold?.kind === "network" || hold?.kind === "profile_available" || hold?.kind === "time";
}

/** What a waiting task is waiting for, short enough for a row or a pill. */
export function waitLabel(hold: TaskHoldView | undefined): string {
  switch (hold?.kind) {
    case "restart":
      return "Resuming";
    case "network":
      return "Waiting for network";
    case "profile_available":
      return "Waiting for usage";
    case "dependency":
      return "Waiting for another task";
    case "time":
      return hold.until ? `Starts ${absoluteTime(hold.until)}` : "Waiting to start";
    default:
      return TASK_STATE_LABELS.pending;
  }
}

/** What a row is doing, which for a waiting task is why it is waiting. */
export function taskStatusLabel(task: { state: string; hold?: TaskHoldView }): string {
  if (task.state !== "pending" || !task.hold) return taskStateLabel(task.state);
  return waitLabel(task.hold);
}

/**
 * When the task expects to pick up again. A rate limit already names its reset
 * time in the reason, so only a network wait needs this line.
 */
export function nextTryLabel(hold: TaskHoldView | undefined): { label: string; title: string } | undefined {
  if (hold?.kind !== "network" || !hold.until) return undefined;
  const at = new Date(hold.until);
  if (Number.isNaN(at.getTime())) return undefined;
  return { label: `Next try ${relativeTime(at)}`, title: absoluteTime(at) };
}
