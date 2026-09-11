// Sidebar state and its loopback client operations.
// Ported from rust/crates/oga-ui/src/state/mod.rs — keep behavior identical.

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
  defaultSidebarPreferences,
  EMPTY_COLLAPSED_GROUPS,
  loadSidebarPreferences,
  SIDEBAR_DEFAULT_WIDTH,
  storeSidebarPreferences,
  toggleCollapsedGroup,
  type CollapsedGroups,
  type SidebarPreferences,
  type TaskArchiveFilter,
  type TaskGrouping,
  type TaskSort,
} from "./sidebar-preferences";
import { Store } from "./store";

export const TASK_PAGE_SIZE = 50;
export const ATTENTION_PAGE_SIZE = 100;
/** The slowest the list is read again in full while the log keeps moving. */
const REREAD_INTERVAL_MS = 15_000;

export type LoadState = "idle" | "loading" | "ready" | "error";
export type ConnectionState = "connecting" | "connected" | "reconnecting" | "offline";
export type EventAction = "none" | "refresh";

export interface SidebarState {
  profiles: ProfileView[];
  tasks: TaskSummary[];
  tasksHasMore: boolean;
  loadedPages: number;
  isLoadingMore: boolean;
  loadMoreFailed: boolean;
  archiveFilter: TaskArchiveFilter;
  projectFilter: string | undefined;
  search: string;
  grouping: TaskGrouping;
  sort: TaskSort;
  collapsed: CollapsedGroups;
  sidebarCollapsed: boolean;
  sidebarWidth: number;
  selectedTask: string | undefined;
  connection: ConnectionState;
  reconnectAttempts: number;
  loadState: LoadState;
  error: string | undefined;
  eventCursor: number;
  /** When the list was last read again in full, so a moving log cannot ask
   * for another one every few seconds. */
  lastReread: number | undefined;
}

export function defaultSidebarState(): SidebarState {
  return {
    profiles: [],
    tasks: [],
    tasksHasMore: false,
    loadedPages: 1,
    isLoadingMore: false,
    loadMoreFailed: false,
    archiveFilter: "active",
    projectFilter: undefined,
    search: "",
    grouping: "parent",
    sort: "priority",
    collapsed: EMPTY_COLLAPSED_GROUPS,
    sidebarCollapsed: false,
    sidebarWidth: SIDEBAR_DEFAULT_WIDTH,
    selectedTask: undefined,
    connection: "connecting",
    reconnectAttempts: 0,
    loadState: "idle",
    error: undefined,
    eventCursor: 0,
    lastReread: undefined,
  };
}

export function sidebarStateWithPreferences(preferences: SidebarPreferences): SidebarState {
  return applyPreferences(defaultSidebarState(), preferences);
}

export function applyPreferences(state: SidebarState, preferences: SidebarPreferences): SidebarState {
  return {
    ...state,
    archiveFilter: preferences.archiveFilter,
    projectFilter: preferences.projectFilter,
    grouping: preferences.grouping,
    sort: preferences.sort,
    collapsed: preferences.collapsed,
    sidebarCollapsed: preferences.sidebarCollapsed,
    sidebarWidth: preferences.sidebarWidth,
  };
}

export function sidebarPreferencesFrom(state: SidebarState): SidebarPreferences {
  return {
    archiveFilter: state.archiveFilter,
    projectFilter: state.projectFilter,
    grouping: state.grouping,
    sort: state.sort,
    collapsed: state.collapsed,
    sidebarCollapsed: state.sidebarCollapsed,
    sidebarWidth: state.sidebarWidth,
  };
}

export function summaryQuery(state: SidebarState): StateQuery {
  return {
    compact: true,
    archived: state.archiveFilter,
    limit: state.loadedPages * TASK_PAGE_SIZE,
    skipSummaryAggregates: true,
  };
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
function mergeTasks(existing: TaskSummary[], page: TaskSummary[], truncated: boolean): TaskSummary[] {
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

export function setArchiveFilter(state: SidebarState, filter: TaskArchiveFilter): [SidebarState, boolean] {
  if (state.archiveFilter === filter) return [state, false];
  return [
    { ...state, archiveFilter: filter, tasks: [], loadedPages: 1, tasksHasMore: false, loadMoreFailed: false },
    true,
  ];
}

export function setProjectFilter(state: SidebarState, project: string | undefined): SidebarState {
  return { ...state, projectFilter: project && project !== "" ? project : undefined };
}

export function setGrouping(state: SidebarState, grouping: TaskGrouping): SidebarState {
  return { ...state, grouping };
}

export function setSort(state: SidebarState, sort: TaskSort): SidebarState {
  return { ...state, sort };
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

/** True when any popover choice differs from the default view. */
export function filtersActive(state: SidebarState): boolean {
  const defaults = defaultSidebarPreferences();
  return (
    state.archiveFilter !== defaults.archiveFilter ||
    state.projectFilter !== undefined ||
    state.grouping !== defaults.grouping ||
    state.sort !== defaults.sort
  );
}

/** True when a filter is holding tasks back, ignoring the search box. */
export function filtersHideTasks(state: SidebarState): boolean {
  const defaults = defaultSidebarPreferences();
  return state.archiveFilter !== defaults.archiveFilter || state.projectFilter !== undefined;
}

/** Restores the default view. Reports whether the broker query changed. */
export function resetFilters(state: SidebarState): [SidebarState, boolean] {
  const defaults = defaultSidebarPreferences();
  const [next, changed] = setArchiveFilter(
    { ...state, projectFilter: undefined, grouping: defaults.grouping, sort: defaults.sort },
    defaults.archiveFilter,
  );
  return [next, changed];
}

export function toggleGroup(state: SidebarState, id: string): SidebarState {
  return { ...state, collapsed: toggleCollapsedGroup(state.collapsed, id, state.grouping) };
}

export function selectTask(state: SidebarState, id: string): [SidebarState, boolean] {
  if (!state.tasks.some((task) => task.id === id)) return [state, false];
  return [{ ...state, selectedTask: id }, true];
}

export function clearSelection(state: SidebarState): SidebarState {
  return { ...state, selectedTask: undefined };
}

/** A dropped stream keeps retrying on its own, so a handful of failures still
 * read as reconnecting. Past that it reads as offline instead of retrying
 * forever silently. */
export const MAX_RECONNECT_ATTEMPTS = 5;

export function applyConnection(state: SidebarState, status: StreamStatus): SidebarState {
  if (status.connected) return { ...state, connection: "connected", reconnectAttempts: 0 };
  const reconnectAttempts = state.reconnectAttempts + 1;
  const connection = reconnectAttempts >= MAX_RECONNECT_ATTEMPTS ? "offline" : "reconnecting";
  return { ...state, connection, reconnectAttempts, error: status.error };
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

export function applyEventBatch(state: SidebarState, batch: EventBatch): [SidebarState, EventAction] {
  let next = { ...state, eventCursor: Math.max(state.eventCursor, batch.cursor) };
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
  if (index === -1) return rereadIfDue({ ...state, eventCursor });
  if (hasGap) return [{ ...state, eventCursor }, "refresh"];

  const task = state.tasks[index];
  if (!timestampAtLeast(pointer.at, task.updatedAt)) {
    return [{ ...state, eventCursor }, "none"];
  }
  const archived = archiveChange(pointer.type);
  const tasks = state.tasks.slice();
  if (archived !== undefined && !filterAdmits(state.archiveFilter, archived)) {
    tasks.splice(index, 1);
    return [{ ...state, eventCursor, tasks }, "none"];
  }

  let archivedAt = task.archivedAt;
  if (archived === true) archivedAt = pointer.at;
  if (archived === false) archivedAt = undefined;
  tasks[index] = {
    ...task,
    state: pointer.state,
    title: pointer.title.trim() !== "" ? pointer.title : task.title,
    updatedAt: pointer.at,
    archivedAt,
  };
  return [{ ...state, eventCursor, tasks }, "none"];
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
    const result = await broker.summary(query);
    if (result.ok) {
      this.store.update((state) => applySummary(state, result.value));
      return undefined;
    }
    this.store.update((state) => applyRefreshError(state, result.error.message));
    return result.error;
  }

  async setArchiveFilter(filter: TaskArchiveFilter): Promise<BridgeError | undefined> {
    const [next, changed] = setArchiveFilter(this.store.snapshot, filter);
    this.store.set(next);
    if (!changed) return undefined;
    this.persist();
    return this.refresh();
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

  setProjectFilter(project: string | undefined): void {
    this.store.update((state) => setProjectFilter(state, project));
    this.persist();
  }

  setGrouping(grouping: TaskGrouping): void {
    this.store.update((state) => setGrouping(state, grouping));
    this.persist();
  }

  setSort(sort: TaskSort): void {
    this.store.update((state) => setSort(state, sort));
    this.persist();
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

  toggleGroup(id: string): void {
    this.store.update((state) => toggleGroup(state, id));
    this.persist();
  }

  resetFilters(): boolean {
    const [next, changed] = resetFilters(this.store.snapshot);
    this.store.set(next);
    if (changed) this.persist();
    return changed;
  }

  selectTask(id: string): boolean {
    const [next, ok] = selectTask(this.store.snapshot, id);
    this.store.set(next);
    return ok;
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

  /** Records what the shell's broker connection is doing. */
  applyConnection(status: StreamStatus): void {
    this.store.update((state) => applyConnection(state, status));
  }
}
