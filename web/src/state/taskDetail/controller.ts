// Task detail controller: loads, follows, and folds updates for one task.
// Ported from rust/crates/oga-ui/src/task_detail/mod.rs — keep behavior identical.

import { broker, streamStatus, unwatchTask, watchTask } from "@/bridge/client";
import { onBrokerStatus, onTaskDelta } from "@/bridge/events";
import type { StreamStatus, TaskDelta, TaskEventPage, TaskEventView, TaskSnapshot } from "@/bridge/types";
import { Store } from "../store";
import {
  absorbPage,
  adopt,
  applyConnection,
  applyDelta,
  defaultTaskDetailState,
  EVENT_PAGE_SIZE,
  INITIAL_EVENT_LIMIT,
  type DeltaOutcome,
  type TaskDetailState,
} from "./state";

export class TaskDetailController {
  private readonly store: Store<TaskDetailState>;
  private events: TaskEventView[] = [];

  constructor(private readonly taskId: string) {
    this.store = new Store(defaultTaskDetailState());
  }

  get snapshot(): TaskDetailState {
    return this.store.snapshot;
  }

  /** Reads the activity in place. Nothing copies it out. */
  withEvents<R>(read: (events: TaskEventView[]) => R): R {
    return read(this.events);
  }

  subscribe(listener: () => void): () => void {
    return this.store.subscribe(listener);
  }

  async loadInitial(): Promise<void> {
    this.store.update((state) => ({ ...state, loading: true, connection: "loading", error: undefined }));
    await this.resync();
  }

  /**
   * Asks the shell to follow this task and takes the state it answers with,
   * which is also how the view recovers from a gap or a dropped stream.
   *
   * Outside the shell there is nothing to follow, so the same reads go
   * straight to the broker and the view stays still.
   */
  async resync(): Promise<void> {
    const result = await watchTask(this.taskId, INITIAL_EVENT_LIMIT);
    if (result.ok) {
      this.adopt(result.value);
    } else {
      await this.loadFromBroker();
    }
  }

  /** Releases the shell's hold on this task when the view goes away. */
  async release(): Promise<void> {
    await unwatchTask(this.taskId);
  }

  private async loadFromBroker(): Promise<void> {
    const [task, page] = await Promise.all([
      broker.task(this.taskId),
      broker.taskEvents(this.taskId, {
        last: INITIAL_EVENT_LIMIT,
        limit: INITIAL_EVENT_LIMIT,
      }),
    ]);
    const pageError = page.ok ? undefined : page.error.message;
    if (page.ok) {
      this.absorbPage(page.value);
    }
    this.store.update((state) => {
      const next: TaskDetailState = { ...state, loading: false };
      if (task.ok) {
        next.task = task.value;
      } else {
        next.error = task.error.message;
      }
      if (next.error === undefined) {
        next.error = pageError;
      }
      next.connection = next.error !== undefined ? "offline" : "live";
      return next;
    });
  }

  private absorbPage(page: TaskEventPage): void {
    const result = absorbPage(this.events, this.store.snapshot, page);
    this.events = result.events;
    this.store.set(result.state);
  }

  private adopt(snapshot: TaskSnapshot): void {
    const result = adopt(this.events, this.store.snapshot, snapshot);
    this.events = result.events;
    this.store.set(result.state);
  }

  async loadEarlier(): Promise<void> {
    const state = this.store.snapshot;
    if (state.loadingEarlier || !state.hasEarlier || state.oldestId === undefined) {
      return;
    }
    const before = state.oldestId;
    this.store.update((s) => ({ ...s, loadingEarlier: true }));
    const result = await broker.taskEvents(this.taskId, {
      before,
      last: EVENT_PAGE_SIZE,
      limit: EVENT_PAGE_SIZE,
    });
    if (result.ok) {
      this.absorbPage(result.value);
      this.store.update((s) => ({ ...s, loadingEarlier: false, error: undefined }));
    } else {
      this.store.update((s) => ({ ...s, loadingEarlier: false, error: result.error.message }));
    }
  }

  /**
   * Folds one update the shell pushed into what is held.
   *
   * The work is the size of the update, never the size of the task.
   */
  applyDelta(delta: TaskDelta): DeltaOutcome {
    const result = applyDelta(this.events, this.store.snapshot, this.taskId, delta);
    this.events = result.events;
    this.store.set(result.state);
    return result.outcome;
  }

  /**
   * Mirrors the shell's broker connection into the activity header, and
   * reports whether the stream came back from a drop, which is the one case
   * where what is held may have fallen behind the broker.
   */
  applyConnection(status: StreamStatus): boolean {
    const [next, returning] = applyConnection(this.store.snapshot, status);
    this.store.set(next);
    return returning;
  }
}

export interface WatchedTaskDetail {
  controller: TaskDetailController;
  /** Unwatches the task on the shell and tears down the subscriptions. */
  dispose: () => void;
}

/**
 * Wires a `TaskDetailController` to the shell for the lifetime of a mounted
 * task detail view: follows the task, applies pushed deltas (resyncing on a
 * gap), and mirrors the broker connection (resyncing on reconnect).
 *
 * Mirrors the effects the Leptos `TaskDetail` component installs on mount
 * and tears down with `on_cleanup`.
 */
export function watchTaskDetail(taskId: string): WatchedTaskDetail {
  const controller = new TaskDetailController(taskId);
  let disposed = false;

  void controller.loadInitial();

  const unsubscribeDelta = onTaskDelta((delta) => {
    if (disposed) return;
    const outcome = controller.applyDelta(delta);
    if (outcome === "gap") {
      void controller.resync();
    }
  });

  const unsubscribeStatus = onBrokerStatus((status) => {
    if (disposed) return;
    const returning = controller.applyConnection(status);
    if (returning) {
      void controller.resync();
    }
  });

  void streamStatus().then((result) => {
    if (disposed) return;
    if (result.ok && !result.value.connected) {
      controller.applyConnection(result.value);
    }
  });

  return {
    controller,
    dispose: () => {
      disposed = true;
      unsubscribeDelta();
      unsubscribeStatus();
      void controller.release();
    },
  };
}
