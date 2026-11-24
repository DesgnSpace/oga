// Sidebar state and its loopback client operations.

import { broker } from "@/bridge/client";
import type {
  BridgeError,
  BrokerSummaryState,
  EventPointer,
  EventBatch,
  ProfileView,
  StateQuery,
  StreamStatus,
  TaskSummary,
} from "@/bridge/types";
import type { EventFrame } from "./event-frame";
import {
  loadSidebarPreferences,
  SIDEBAR_DEFAULT_WIDTH,
  storeSidebarPreferences,
  type SidebarPreferences,
  type TaskArchiveFilter,
} from "./sidebar-preferences";
import { Store } from "./store";
import type { TaskView } from "./task-views";

export const TASK_PAGE_SIZE = 50;
// 15s avoids rereading the list on every event while it keeps moving.
const REREAD_INTERVAL_MS = 15_000;

export type LoadState = "idle" | "loading" | "ready" | "error";
export type ConnectionState = "connecting" | "connected" | "reconnecting" | "offline";
export type EventAction = "none" | "refresh";

/** A live update carries a task's new state but not its question, error, or
 * completion, so a row it moves to another state is stale until the next reread. */
export interface SidebarTask extends TaskSummary {
  staleOutcome?: true;
}

/** True totals across every active task, whatever the list shows. */
export interface TaskOverview {
  /** Keyed by task state. */
  states: Record<string, number>;
  /** Keyed by project folder. */
  projects: Record<string, number>;
}

export interface SidebarState {
  profiles: ProfileView[];
  tasks: SidebarTask[];
  tasksHasMore: boolean;
  /** True totals per state for the list's filters. */
  groupCounts: Record<string, number>;
  overview: TaskOverview;
  view: TaskView;
  loadedPages: number;
  isLoadingMore: boolean;
  loadMoreFailed: boolean;
  archiveFilter: TaskArchiveFilter;
  projectFilter: string | undefined;
  search: string;
  sidebarCollapsed: boolean;
  sidebarWidth: number;
  selectedTask: string | undefined;
  connection: ConnectionState;
  reconnectAttempts: number;
  loadState: LoadState;
  error: string | undefined;
  eventCursor: number;
  lastReread: number | undefined;
  /** Tasks the list was reread for once; one still missing after that is filtered out. */
  soughtTaskIds: ReadonlySet<string>;
}

export function defaultSidebarState(): SidebarState {
  return {
    profiles: [],
    tasks: [],
    tasksHasMore: false,
    groupCounts: {},
    overview: { states: {}, projects: {} },
    view: "recent",
    loadedPages: 1,
    isLoadingMore: false,
    loadMoreFailed: false,
    archiveFilter: "active",
    projectFilter: undefined,
    search: "",
    sidebarCollapsed: false,
    sidebarWidth: SIDEBAR_DEFAULT_WIDTH,
    selectedTask: undefined,
    connection: "connecting",
    reconnectAttempts: 0,
    loadState: "idle",
    error: undefined,
    eventCursor: 0,
    lastReread: undefined,
    soughtTaskIds: new Set(),
  };
}

export function sidebarStateWithPreferences(preferences: SidebarPreferences): SidebarState {
  return applyPreferences(defaultSidebarState(), preferences);
}

export function applyPreferences(state: SidebarState, preferences: SidebarPreferences): SidebarState {
  return { ...state, sidebarCollapsed: preferences.sidebarCollapsed, sidebarWidth: preferences.sidebarWidth };
}

export function sidebarPreferencesFrom(state: SidebarState): SidebarPreferences {
  return { sidebarCollapsed: state.sidebarCollapsed, sidebarWidth: state.sidebarWidth };
}

export function summaryQuery(state: SidebarState): StateQuery {
  const search = searchTerm(state);
  return {
    compact: true,
    archived: state.archiveFilter,
    limit: state.loadedPages * TASK_PAGE_SIZE,
    skipSummaryAggregates: true,
    project: state.projectFilter,
    search: search === "" ? undefined : search,
    group: "status",
  };
}

/** Reads one kind of total across every active task, with the fewest rows. */
export function overviewQuery(group: "status" | "project"): StateQuery {
  return { compact: true, archived: "active", limit: 1, skipSummaryAggregates: true, group };
}

export function beginRefresh(state: SidebarState): [SidebarState, StateQuery] {
  const next: SidebarState = {
    ...state,
    loadState: state.tasks.length === 0 ? "loading" : state.loadState,
    connection: state.connection === "connected" ? "connected" : "connecting",
    reconnectAttempts: state.connection === "connected" ? state.reconnectAttempts : 0,
    error: undefined,
  };
  return [next, summaryQuery(next)];
}

export function applySummary(state: SidebarState, summary: BrokerSummaryState): SidebarState {
  const tasksHasMore = summary.tasksHasMore ?? false;
  return {
    ...state,
    profiles: summary.profiles,
    tasks: mergeTasks(state.tasks, summary.tasks, tasksHasMore),
    tasksHasMore,
    groupCounts: summary.groupCounts ?? {},
    loadState: "ready",
    connection: "connected",
    error: undefined,
    loadMoreFailed: false,
    isLoadingMore: false,
  };
}

/** A page holds the most recently updated tasks the filter admits. A settled
 * task stops earning updates, so a full page can drift past one that still
 * belongs on the list; keep those, appended after what the page did return.
 * A task the page had room for and left out no longer matches the filter, so
 * it goes. */
function mergeTasks(existing: SidebarTask[], page: TaskSummary[], truncated: boolean): SidebarTask[] {
  const tail = page[page.length - 1]?.updatedAt;
  if (!truncated || tail === undefined) return page;
  const onPage = new Set(page.map((task) => task.id));
  const pastTheTail = existing.filter(
    (task) => !onPage.has(task.id) && !timestampAtLeast(task.updatedAt, tail),
  );
  return [...page, ...pastTheTail];
}

export function applyRefreshError(state: SidebarState, message: string): SidebarState {
  return {
    ...state,
    error: message,
    connection: "offline",
    loadState: state.tasks.length === 0 ? "error" : state.loadState,
  };
}

export function beginLoadMore(state: SidebarState): [SidebarState, StateQuery] | undefined {
  if (!state.tasksHasMore || state.isLoadingMore) return undefined;
  const next: SidebarState = {
    ...state,
    isLoadingMore: true,
    loadedPages: state.loadedPages + 1,
  };
  return [next, summaryQuery(next)];
}

export function finishLoadMoreError(state: SidebarState, message: string): SidebarState {
  return {
    ...state,
    isLoadingMore: false,
    loadMoreFailed: true,
    loadedPages: Math.max(state.loadedPages - 1, 1),
    error: message,
  };
}

function withArchiveFilter(state: SidebarState, filter: TaskArchiveFilter): SidebarState {
  if (state.archiveFilter === filter) return state;
  // Rows from the other filter must not linger while the new read is out.
  return { ...state, archiveFilter: filter, tasks: [], groupCounts: {}, loadedPages: 1, tasksHasMore: false, loadMoreFailed: false };
}

/** Reports whether the list must be read again. */
export function showView(state: SidebarState, view: TaskView): [SidebarState, boolean] {
  const next = withArchiveFilter({ ...state, view, projectFilter: undefined }, view === "archived" ? "only" : "active");
  return [next, next.archiveFilter !== state.archiveFilter || state.projectFilter !== undefined];
}

/** Reports whether the list must be read again. */
export function showProject(state: SidebarState, project: string): [SidebarState, boolean] {
  const next = withArchiveFilter({ ...state, view: "recent", projectFilter: project }, "active");
  return [next, next.archiveFilter !== state.archiveFilter || state.projectFilter !== project];
}

export function setSearch(state: SidebarState, search: string): SidebarState {
  return { ...state, search };
}

export function searchTerm(state: SidebarState): string {
  return state.search.trim();
}

export function toggleSidebar(state: SidebarState): SidebarState {
  return { ...state, sidebarCollapsed: !state.sidebarCollapsed };
}

export function selectTask(state: SidebarState, id: string): SidebarState {
  return state.selectedTask === id ? state : { ...state, selectedTask: id };
}

export function clearSelection(state: SidebarState): SidebarState {
  return { ...state, selectedTask: undefined };
}

/** A dropped stream keeps retrying on its own, so a handful of failures still
 * read as reconnecting. Past that it reads as offline instead of retrying
 * forever silently. */
export const MAX_RECONNECT_ATTEMPTS = 5;

/** Also reports whether the stream came back from a drop, the one case where
 * what the list holds may have fallen behind the broker. */
export function applyConnection(state: SidebarState, status: StreamStatus): [SidebarState, boolean] {
  if (status.connected) {
    const returning = state.connection === "reconnecting" || state.connection === "offline";
    return [{ ...state, connection: "connected", reconnectAttempts: 0 }, returning];
  }
  const reconnectAttempts = state.reconnectAttempts + 1;
  const connection = reconnectAttempts >= MAX_RECONNECT_ATTEMPTS ? "offline" : "reconnecting";
  return [{ ...state, connection, reconnectAttempts, error: status.error }, false];
}

export function applyEventFrame(state: SidebarState, frame: EventFrame): [SidebarState, EventAction] {
  switch (frame.kind) {
    case "ready": {
      const next = { ...state, eventCursor: Math.max(state.eventCursor, frame.frame.cursor) };
      return [next, "refresh"];
    }
    case "task":
      return applyPointer(state, frame.pointer);
    case "cursor":
    case "keepalive": {
      if (frame.frame.cursor <= state.eventCursor) return [state, "none"];
      return rereadIfDue({ ...state, eventCursor: frame.frame.cursor });
    }
    case "unknown":
      return [state, "none"];
  }
}

/** Hands back the same state when the batch changes nothing, so the list is not redrawn. */
export function applyEventBatch(state: SidebarState, batch: EventBatch): [SidebarState, EventAction] {
  let next = batch.cursor > state.eventCursor ? { ...state, eventCursor: batch.cursor } : state;
  let action: EventAction = batch.stale ? "refresh" : "none";
  for (const pointer of batch.pointers) {
    [next, action] = applyPointer(next, pointer);
    if (action === "refresh") break;
  }
  return [next, action];
}

/** A cursor frame says the log passed events that carry no task pointer, and
 * the broker sends them for as long as anything is running. Nothing the list
 * draws has moved, but the fields a pointer does not carry still drift, so
 * the list is reread on a slow beat rather than once per frame. */
function rereadIfDue(state: SidebarState): [SidebarState, EventAction] {
  const now = Date.now();
  if (state.lastReread !== undefined && now - state.lastReread < REREAD_INTERVAL_MS) {
    return [state, "none"];
  }
  return [{ ...state, lastReread: now }, "refresh"];
}

function applyPointer(state: SidebarState, pointer: EventPointer): [SidebarState, EventAction] {
  const hasGap = pointer.cursor > state.eventCursor + 1;
  const eventCursor = Math.max(state.eventCursor, pointer.cursor);
  const index = state.tasks.findIndex((task) => task.id === pointer.taskId);
  if (index === -1) {
    if (state.soughtTaskIds.has(pointer.taskId)) return rereadIfDue({ ...state, eventCursor });
    const soughtTaskIds = new Set(state.soughtTaskIds).add(pointer.taskId);
    return [{ ...state, eventCursor, soughtTaskIds, lastReread: Date.now() }, "refresh"];
  }
  if (hasGap) return [{ ...state, eventCursor }, "refresh"];

  const task = state.tasks[index];
  if (!timestampAtLeast(pointer.at, task.updatedAt)) {
    return [eventCursor === state.eventCursor ? state : { ...state, eventCursor }, "none"];
  }
  const archived = archiveChange(pointer.type);
  const tasks = state.tasks.slice();
  if (archived !== undefined && !filterAdmits(state.archiveFilter, archived)) {
    tasks.splice(index, 1);
    // A task leaving the list changes its group's total.
    return [{ ...state, eventCursor, tasks }, "refresh"];
  }

  let archivedAt = task.archivedAt;
  if (archived === true) archivedAt = pointer.at;
  if (archived === false) archivedAt = undefined;
  const moved = pointer.state !== task.state;
  tasks[index] = {
    ...task,
    state: pointer.state,
    title: pointer.title.trim() !== "" ? pointer.title : task.title,
    updatedAt: pointer.at,
    archivedAt,
  };
  if (moved) tasks[index].staleOutcome = true;
  // The row moved, so the group totals the last read returned are stale too.
  return [{ ...state, eventCursor, tasks }, moved ? "refresh" : "none"];
}

/** The task's new archive standing, or `undefined` when the event left it
 * where it was. */
function archiveChange(eventType: string): boolean | undefined {
  if (eventType === "archived") return true;
  if (eventType === "unarchived") return false;
  return undefined;
}

function filterAdmits(filter: TaskArchiveFilter, archived: boolean): boolean {
  switch (filter) {
    case "active":
      return !archived;
    case "only":
      return archived;
    case "include":
      return true;
  }
}

function timestampAtLeast(left: string, right: string): boolean {
  const leftMs = Date.parse(left);
  const rightMs = Date.parse(right);
  if (!Number.isNaN(leftMs) && !Number.isNaN(rightMs)) return leftMs >= rightMs;
  return left >= right;
}

// --- Controller: broker calls plus the persisted store ---------------------

export class SidebarController {
  private readonly store: Store<SidebarState>;

  constructor(preferences: SidebarPreferences = loadSidebarPreferences()) {
    this.store = new Store(sidebarStateWithPreferences(preferences));
  }

  get snapshot(): SidebarState {
    return this.store.snapshot;
  }

  subscribe(listener: () => void): () => void {
    return this.store.subscribe(listener);
  }

  update(updater: (state: SidebarState) => SidebarState): void {
    this.store.update(updater);
  }

  private persist(): void {
    storeSidebarPreferences(sidebarPreferencesFrom(this.store.snapshot));
  }

  async refresh(): Promise<BridgeError | undefined> {
    const [begun, query] = beginRefresh(this.store.snapshot);
    this.store.set(begun);
    const [result, states, projects] = await Promise.all([
      broker.summary(query),
      broker.summary(overviewQuery("status")),
      broker.summary(overviewQuery("project")),
    ]);
    if (states.ok && projects.ok) {
      const overview = { states: states.value.groupCounts ?? {}, projects: projects.value.groupCounts ?? {} };
      this.store.update((state) => ({ ...state, overview }));
    }
    if (result.ok) {
      this.store.update((state) => applySummary(state, result.value));
      return undefined;
    }
    this.store.update((state) => applyRefreshError(state, result.error.message));
    return result.error;
  }

  showView(view: TaskView): void {
    const [next, requery] = showView(this.store.snapshot, view);
    this.store.set(next);
    if (requery) void this.refresh();
  }

  showProject(project: string): void {
    const [next, requery] = showProject(this.store.snapshot, project);
    this.store.set(next);
    if (requery) void this.refresh();
  }

  async loadMore(): Promise<BridgeError | undefined> {
    const begun = beginLoadMore(this.store.snapshot);
    if (!begun) return undefined;
    const [next, query] = begun;
    this.store.set(next);
    const result = await broker.summary(query);
    if (result.ok) {
      this.store.update((state) => applySummary(state, result.value));
      return undefined;
    }
    this.store.update((state) => finishLoadMoreError(state, result.error.message));
    return result.error;
  }

  setSearch(search: string): void {
    this.store.update((state) => setSearch(state, search));
  }

  toggleSidebar(): void {
    this.store.update(toggleSidebar);
    this.persist();
  }

  setSidebarWidth(width: number): void {
    this.store.update((state) => ({ ...state, sidebarWidth: width }));
    this.persist();
  }

  selectTask(id: string): void {
    this.store.update((state) => selectTask(state, id));
  }

  clearSelection(): void {
    this.store.update(clearSelection);
  }

  /** Folds one paced shell batch into the sidebar's state. */
  applyBatch(batch: EventBatch): EventAction {
    let action: EventAction = "none";
    this.store.update((state) => {
      const [next, taken] = applyEventBatch(state, batch);
      action = taken;
      return next;
    });
    return action;
  }

  /** Records what the shell's broker connection is doing, and reports whether
   * the list has to be read again to catch up with the drop. */
  applyConnection(status: StreamStatus): boolean {
    const [next, returning] = applyConnection(this.store.snapshot, status);
    this.store.set(next);
    return returning;
  }
}
