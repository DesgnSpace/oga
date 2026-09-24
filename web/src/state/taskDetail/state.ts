// Task detail state and delta/connection folding.

import type { StreamStatus, Task, TaskDelta, TaskEventPage, TaskEventView, TaskSnapshot } from "@/bridge/types";

export const INITIAL_EVENT_LIMIT = 150;
export const EVENT_PAGE_SIZE = 150;

export type DetailConnection = "loading" | "live" | "offline";

export function connectionLabel(connection: DetailConnection): string {
  switch (connection) {
    case "loading":
      return "Loading activity…";
    case "live":
      return "Live updates";
    case "offline":
      return "Updates unavailable";
  }
}

export type DeltaOutcome =
  /** Already held, or about another task. Nothing to redraw. */
  | "ignored"
  /** Folded in. Redraw. */
  | "applied"
  /** It does not continue from the cursor held, so something was missed and
   * the task has to be read again. */
  | "gap";

export interface TaskDetailState {
  task: Task | undefined;
  revision: number;
  cursor: number;
  oldestId: number | undefined;
  hasEarlier: boolean;
  loading: boolean;
  loadingEarlier: boolean;
  connection: DetailConnection;
  error: string | undefined;
}

export function defaultTaskDetailState(): TaskDetailState {
  return {
    task: undefined,
    revision: 0,
    cursor: 0,
    oldestId: undefined,
    hasEarlier: false,
    loading: true,
    loadingEarlier: false,
    connection: "loading",
    error: undefined,
  };
}

export function mergeEvents(held: TaskEventView[], arriving: TaskEventView[]): TaskEventView[] {
  if (arriving.length === 0) return held;
  const newest = held.length > 0 ? held[held.length - 1].id : Number.NEGATIVE_INFINITY;
  let ordered = true;
  for (let i = 1; i < arriving.length; i++) {
    if (!(arriving[i - 1].id < arriving[i].id)) {
      ordered = false;
      break;
    }
  }
  if (ordered && arriving[0].id > newest) {
    // The controller owns this array. Keep its identity stable so subscribers
    // can use the revision as the invalidation signal without copying history.
    held.push(...arriving);
    return held;
  }
  const merged = new Map<number, TaskEventView>();
  for (const event of held) merged.set(event.id, event);
  for (const event of arriving) merged.set(event.id, event);
  return Array.from(merged.values()).sort((a, b) => a.id - b.id);
}

export interface FoldedEvents {
  events: TaskEventView[];
  state: TaskDetailState;
}

export function absorbPage(events: TaskEventView[], state: TaskDetailState, page: TaskEventPage): FoldedEvents {
  const nextEvents = mergeEvents(events, page.events);
  const firstId = nextEvents.length > 0 ? nextEvents[0].id : undefined;
  const cursor = page.cursor !== undefined ? Math.max(state.cursor, page.cursor) : state.cursor;
  const oldestId =
    page.oldestId !== undefined
      ? state.oldestId !== undefined
        ? Math.min(state.oldestId, page.oldestId)
        : page.oldestId
      : (firstId ?? state.oldestId);
  const hasEarlier = page.hasEarlier !== undefined ? page.hasEarlier : state.hasEarlier;
  return {
    events: nextEvents,
    state: { ...state, cursor, oldestId, hasEarlier, revision: state.revision + 1 },
  };
}

export function adopt(events: TaskEventView[], state: TaskDetailState, snapshot: TaskSnapshot): FoldedEvents {
  const newest = events.length > 0 ? events[events.length - 1] : undefined;
  const joinsUp = newest === undefined || snapshot.oldestId === undefined || snapshot.oldestId <= newest.id + 1;
  const snapshotIsBehind = snapshot.cursor < state.cursor;
  const restarted = events.length > 0 && !joinsUp && !snapshotIsBehind;
  const held = restarted ? [] : events;
  const starting = held.length === 0;
  const nextEvents = mergeEvents(held, snapshot.events);
  return {
    events: nextEvents,
    state: {
      ...state,
      task: snapshot.task,
      cursor: Math.max(state.cursor, snapshot.cursor),
      oldestId: starting ? snapshot.oldestId : state.oldestId,
      hasEarlier: starting ? snapshot.hasEarlier : state.hasEarlier,
      loading: false,
      error: undefined,
      connection: "live",
      revision: state.revision + 1,
    },
  };
}

export interface AppliedDelta {
  events: TaskEventView[];
  state: TaskDetailState;
  outcome: DeltaOutcome;
}

export function applyDelta(
  events: TaskEventView[],
  state: TaskDetailState,
  taskId: string,
  delta: TaskDelta,
): AppliedDelta {
  if (delta.taskId !== taskId) {
    return { events, state, outcome: "ignored" };
  }
  if (state.task === undefined) {
    return { events, state, outcome: "ignored" };
  }
  if (delta.fromCursor > state.cursor) {
    return { events, state, outcome: "gap" };
  }
  if (delta.cursor <= state.cursor && delta.task === undefined) {
    return { events, state, outcome: "ignored" };
  }
  const arriving = delta.events.length > 0;
  const nextEvents = arriving ? mergeEvents(events, delta.events) : events;
  const nextState: TaskDetailState = {
    ...state,
    task: delta.task ?? state.task,
    cursor: Math.max(state.cursor, delta.cursor),
    error: undefined,
    revision: arriving ? state.revision + 1 : state.revision,
  };
  return { events: nextEvents, state: nextState, outcome: "applied" };
}

/**
 * Mirrors the shell's broker connection into the activity header, and
 * reports whether the stream came back from a drop, which is the one case
 * where what is held may have fallen behind the broker.
 *
 * `streamFloor`/`stale` on `StreamStatus` are not consulted here — matching
 * the Rust controller, only `connected` drives this state.
 */
export function applyConnection(state: TaskDetailState, status: StreamStatus): [TaskDetailState, boolean] {
  if (!status.connected) {
    return [{ ...state, connection: "offline", error: status.error }, false];
  }
  const returning = state.connection === "offline";
  return [{ ...state, connection: "live" }, returning];
}
