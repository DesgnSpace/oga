// Task detail state and delta/connection folding.
// Ported from rust/crates/oga-ui/src/task_detail/mod.rs — keep behavior identical.

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

/** What a delta the shell pushed did to the view. */
export type DeltaOutcome =
  /** Already held, or about another task. Nothing to redraw. */
  | "ignored"
  /** Folded in. Redraw. */
  | "applied"
  /** It does not continue from the cursor held, so something was missed and
   * the task has to be read again. */
  | "gap";

/**
 * What the detail view holds about a task, minus its activity.
 *
 * Activity is held separately from this state so that folding an update in,
 * and handing the current state to a view, both cost the same whether the
 * task has ten events or ten thousand. `revision` moves whenever the
 * activity does, which is how a view knows to recompose.
 */
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

/**
 * Folds arriving activity into what is already held.
 *
 * Activity that is wholly newer than everything held is an append, which is
 * the live case and costs what arrived. Anything else — an earlier page, a
 * replay, an overlap, activity that did not arrive in id order — merges by
 * id, which also drops duplicates.
 */
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

/** Activity and state after folding something new in. */
export interface FoldedEvents {
  events: TaskEventView[];
  state: TaskDetailState;
}

/** Folds one page of activity in and moves the marks that came with it. */
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

/**
 * Takes the state the shell answered a watch with.
 *
 * Activity already held is kept when the snapshot reaches back far enough to
 * touch it. When it does not, the two are not one run of history, so what
 * was held is dropped rather than left with a hole in it.
 */
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

/**
 * Folds one update the shell pushed into what is held.
 *
 * The work is the size of the update, never the size of the task.
 */
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
