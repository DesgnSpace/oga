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

const MAX_WORK_EXPANSIONS = 512;

export interface TaskDetailViewState {
  scrollTop: number;
  stickToEnd: boolean;
  workExpansion: ReadonlyMap<number, boolean>;
}

interface SyncStart {
  revision: number;
  cursor: number;
  task: TaskDetailState["task"];
}

export class TaskDetailController {
  private readonly store: Store<TaskDetailState>;
  private events: TaskEventView[] = [];
  private active = false;
  private managed = false;
  private activation = 0;
  private commandChain: Promise<void> = Promise.resolve();
  private resyncPromise: Promise<void> | undefined;
  private resyncActivation = -1;
  private unsubscribeDelta: (() => void) | undefined;
  private unsubscribeStatus: (() => void) | undefined;
  private view: TaskDetailViewState = { scrollTop: 0, stickToEnd: true, workExpansion: new Map() };

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

  get viewState(): TaskDetailViewState {
    return this.view;
  }

  setScrollPosition(scrollTop: number, stickToEnd: boolean): void {
    this.view = { ...this.view, scrollTop, stickToEnd };
  }

  setWorkExpansion(id: number, expanded: boolean): void {
    const next = new Map(this.view.workExpansion);
    next.set(id, expanded);
    while (next.size > MAX_WORK_EXPANSIONS) {
      const oldest = next.keys().next().value;
      if (oldest === undefined) break;
      next.delete(oldest);
    }
    this.view = { ...this.view, workExpansion: next };
  }

  async loadInitial(): Promise<void> {
    if (this.managed && !this.active) return;
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
    if (this.managed && !this.active) return;
    const activation = this.activation;
    if (this.resyncPromise !== undefined && this.resyncActivation === activation) {
      await this.resyncPromise;
      return;
    }
    this.store.update((state) => ({ ...state, loading: true, connection: "loading", error: undefined }));
    this.resyncActivation = activation;
    const pending = this.enqueue(() => this.readSnapshot(activation));
    const tracked = pending.finally(() => {
      if (this.resyncPromise === tracked) {
        this.resyncPromise = undefined;
        this.resyncActivation = -1;
      }
    });
    this.resyncPromise = tracked;
    await tracked;
  }

  /** Releases the shell's hold on this task when the view goes away. */
  async release(): Promise<void> {
    await this.deactivate();
  }

  /** Starts one shell watch for all views currently using this task. */
  activate(): void {
    if (this.active) return;
    this.managed = true;
    this.active = true;
    this.activation += 1;
    const activation = this.activation;
    this.store.update((state) => ({
      ...state,
      loading: true,
      loadingEarlier: false,
      connection: "loading",
      error: undefined,
    }));

    this.unsubscribeDelta = onTaskDelta((delta) => {
      if (!this.canApply(activation)) return;
      const outcome = this.applyDelta(delta);
      if (outcome === "gap") void this.resync();
    });
    this.unsubscribeStatus = onBrokerStatus((status) => {
      if (!this.canApply(activation)) return;
      const returning = this.applyConnection(status);
      if (returning) void this.resync();
    });
    void streamStatus().then((result) => {
      if (!this.canApply(activation)) return;
      if (result.ok && !result.value.connected) this.applyConnection(result.value);
    });
    void this.loadInitial();
  }

  /** Stops the shell watch while leaving a last-known snapshot in memory. */
  async deactivate(): Promise<void> {
    if (!this.managed || !this.active) return;
    this.active = false;
    this.activation += 1;
    this.unsubscribeDelta?.();
    this.unsubscribeDelta = undefined;
    this.unsubscribeStatus?.();
    this.unsubscribeStatus = undefined;
    await this.enqueue(async () => {
      await unwatchTask(this.taskId);
    });
  }

  private async readSnapshot(activation: number): Promise<void> {
    if (!this.canApply(activation)) return;
    const start = this.syncStart();
    const result = await watchTask(this.taskId, INITIAL_EVENT_LIMIT);
    if (!this.canApply(activation)) return;
    if (result.ok) {
      if (this.snapshotIsStale(result.value.cursor, start)) {
        this.finishSync();
        return;
      }
      this.adopt(result.value);
      return;
    }
    await this.loadFromBroker(activation, start);
  }

  private async loadFromBroker(activation: number, start: SyncStart): Promise<void> {
    const [task, page] = await Promise.all([
      broker.task(this.taskId),
      broker.taskEvents(this.taskId, {
        last: INITIAL_EVENT_LIMIT,
        limit: INITIAL_EVENT_LIMIT,
      }),
    ]);
    if (!this.canApply(activation)) return;
    if (!task.ok && task.error.status === 404) {
      this.clearMissing(task.error.message);
      return;
    }
    const pageCursor = page.ok ? page.value.cursor : undefined;
    if (pageCursor !== undefined && this.snapshotIsStale(pageCursor, start)) {
      this.finishSync();
      return;
    }
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

  private syncStart(): SyncStart {
    const state = this.store.snapshot;
    return { revision: state.revision, cursor: state.cursor, task: state.task };
  }

  private snapshotIsStale(cursor: number, start: SyncStart): boolean {
    const current = this.store.snapshot;
    return cursor < current.cursor ||
      (cursor <= current.cursor && (current.revision !== start.revision || current.task !== start.task));
  }

  private finishSync(): void {
    this.store.update((state) => ({ ...state, loading: false, connection: "live", error: undefined }));
  }

  private clearMissing(error: string): void {
    this.events = [];
    this.store.update((state) => ({
      ...state,
      task: undefined,
      revision: state.revision + 1,
      cursor: 0,
      oldestId: undefined,
      hasEarlier: false,
      loading: false,
      loadingEarlier: false,
      connection: "offline",
      error,
    }));
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
    if (this.managed && !this.active) return;
    const state = this.store.snapshot;
    if (state.loadingEarlier || !state.hasEarlier || state.oldestId === undefined) {
      return;
    }
    const before = state.oldestId;
    const activation = this.activation;
    this.store.update((s) => ({ ...s, loadingEarlier: true }));
    const result = await broker.taskEvents(this.taskId, {
      before,
      last: EVENT_PAGE_SIZE,
      limit: EVENT_PAGE_SIZE,
    });
    if (!this.canApply(activation)) return;
    if (this.store.snapshot.oldestId !== before) {
      this.store.update((s) => ({ ...s, loadingEarlier: false }));
      return;
    }
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

  cacheBytes(): number {
    const task = this.store.snapshot.task;
    if (task === undefined) return 0;
    const encoder = new TextEncoder();
    const bytes = (value: unknown): number => encoder.encode(JSON.stringify(value) ?? "").byteLength;
    return 1_024 +
      this.view.workExpansion.size * 32 +
      this.events.length * 64 +
      bytes(task) +
      this.events.reduce((total, event) => total + bytes(event), 0);
  }

  private canApply(activation: number): boolean {
    return activation === this.activation && (!this.managed || this.active);
  }

  private enqueue(operation: () => Promise<void>): Promise<void> {
    const next = this.commandChain.then(operation, operation);
    this.commandChain = next.catch(() => undefined);
    return next;
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
  let entry = taskDetailEntries.get(taskId);
  if (entry === undefined) {
    entry = { taskId, controller: new TaskDetailController(taskId), references: 0, retained: false, bytes: 0 };
    taskDetailEntries.set(taskId, entry);
  }
  if (entry.references === 0) removeRetained(entry);
  entry.references += 1;
  if (entry.references === 1) entry.controller.activate();
  const controller = entry.controller;
  let disposed = false;

  return {
    controller,
    dispose: () => {
      if (disposed) return;
      disposed = true;
      entry!.references -= 1;
      if (entry!.references !== 0) return;
      void entry!.controller.release().then(() => retain(entry!));
    },
  };
}

export const TASK_DETAIL_CACHE_MAX_ENTRIES = 8;
export const TASK_DETAIL_CACHE_MAX_BYTES = 4 * 1024 * 1024;

interface TaskDetailCacheEntry {
  taskId: string;
  controller: TaskDetailController;
  references: number;
  retained: boolean;
  bytes: number;
}

const taskDetailEntries = new Map<string, TaskDetailCacheEntry>();
let retainedBytes = 0;

function removeRetained(entry: TaskDetailCacheEntry): void {
  if (!entry.retained) return;
  retainedBytes -= entry.bytes;
  entry.bytes = 0;
  entry.retained = false;
}

function retain(entry: TaskDetailCacheEntry): void {
  if (entry.references !== 0 || taskDetailEntries.get(entry.taskId) !== entry) return;
  removeRetained(entry);
  const bytes = entry.controller.cacheBytes();
  if (bytes === 0 || bytes > TASK_DETAIL_CACHE_MAX_BYTES) {
    taskDetailEntries.delete(entry.taskId);
    return;
  }
  entry.bytes = bytes;
  entry.retained = true;
  retainedBytes += bytes;
  taskDetailEntries.delete(entry.taskId);
  taskDetailEntries.set(entry.taskId, entry);
  evict();
}

function evict(): void {
  while (retainedEntryCount() > TASK_DETAIL_CACHE_MAX_ENTRIES || retainedBytes > TASK_DETAIL_CACHE_MAX_BYTES) {
    const oldest = Array.from(taskDetailEntries.entries()).find(([, entry]) => entry.retained && entry.references === 0);
    if (!oldest) return;
    const [taskId, entry] = oldest;
    removeRetained(entry);
    taskDetailEntries.delete(taskId);
  }
}

function retainedEntryCount(): number {
  let count = 0;
  for (const entry of taskDetailEntries.values()) {
    if (entry.retained) count += 1;
  }
  return count;
}

export function taskDetailCacheStats(): {
  entries: number;
  bytes: number;
  maxEntries: number;
  maxBytes: number;
} {
  return {
    entries: retainedEntryCount(),
    bytes: retainedBytes,
    maxEntries: TASK_DETAIL_CACHE_MAX_ENTRIES,
    maxBytes: TASK_DETAIL_CACHE_MAX_BYTES,
  };
}

/** Clears retained entries between transport-isolated tests. */
export function clearTaskDetailCacheForTests(): void {
  for (const [taskId, entry] of taskDetailEntries) {
    if (entry.references !== 0) continue;
    removeRetained(entry);
    taskDetailEntries.delete(taskId);
  }
}
