// The one task status indicator, shared by the sidebar row, task header, and composer.

import { useState } from "react";
import type { TaskState } from "@/bridge/types";

/** How a state reads to the user. States that mean the same thing share a look. */
export type TaskStatusLook = "waiting" | "running" | "needs_input" | "problem" | "settled";

const LOOK_BY_STATE = {
  queued: "waiting",
  preparing_checkout: "waiting",
  removing_checkout: "waiting",
  pending: "waiting",
  answered: "waiting",
  running: "running",
  needs_input: "needs_input",
  blocked: "problem",
  failed: "problem",
  completed: "settled",
  cancelled: "settled",
} satisfies Record<TaskState, TaskStatusLook>;

export function taskStatusLook(state: TaskState): TaskStatusLook {
  return LOOK_BY_STATE[state];
}

export interface TaskStatusDotProps {
  state: TaskState;
  /** Accessible label and tooltip text, e.g. from `taskStatusLabel`. */
  label: string;
  /** Sidebar-only: a new outcome fills the dot; an opened one leaves it hollow. */
  unread?: boolean;
  /** True when a sibling element already carries the accessible status text. */
  decorative?: boolean;
}

export function TaskStatusDot({ state, label, unread = false, decorative = false }: TaskStatusDotProps) {
  const look = taskStatusLook(state);
  // The mark plays its entrance when the look changes while shown, never on first render.
  const [shownLook, setShownLook] = useState(look);
  const [changes, setChanges] = useState(0);
  if (look !== shownLook) {
    setShownLook(look);
    setChanges(changes + 1);
  }
  const className = [
    "task-dot",
    `task-dot-${state}`,
    `task-dot-look-${look}`,
    unread ? "task-dot-unread" : "task-dot-viewed",
  ].join(" ");
  const changed = changes > 0 ? "true" : undefined;
  if (decorative) return <span key={changes} className={className} data-changed={changed} aria-hidden="true" title={label} />;
  return <span key={changes} className={className} data-changed={changed} role="img" aria-label={label} title={label} />;
}
