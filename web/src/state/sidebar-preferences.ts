// Sidebar filter/grouping/sort choices and collapsed group ids, persisted to
// keys the Rust app used.
//
// Ported from rust/crates/oga-ui/src/state/mod.rs — keep behavior identical.

import { readStorage, writeStorage } from "./storage";

export const SIDEBAR_MIN_WIDTH = 240;
export const SIDEBAR_MAX_WIDTH = 420;
export const SIDEBAR_DEFAULT_WIDTH = 320;

const PROJECT_FILTER_KEY = "taskProjectFilter";
const GROUPING_KEY = "taskGrouping";
const SORT_KEY = "taskSort";
const COLLAPSED_KEY = "collapsedTaskGroups";
const ARCHIVE_FILTER_KEY = "taskArchiveFilter";
const SIDEBAR_COLLAPSED_KEY = "taskSidebarCollapsed";
const SIDEBAR_WIDTH_KEY = "taskSidebarWidth";

export type TaskGrouping = "parent" | "project" | "status" | "none";
export const TASK_GROUPINGS: TaskGrouping[] = ["parent", "project", "status", "none"];

export function taskGroupingLabel(grouping: TaskGrouping): string {
  switch (grouping) {
    case "parent":
      return "With sub-tasks";
    case "project":
      return "Project";
    case "status":
      return "Status";
    case "none":
      return "No grouping";
  }
}

export type TaskSort = "recent" | "priority" | "updated";
export const TASK_SORTS: TaskSort[] = ["recent", "priority", "updated"];

export function taskSortLabel(sort: TaskSort): string {
  switch (sort) {
    case "recent":
      return "Newest first";
    case "priority":
      return "Priority";
    case "updated":
      return "Recently updated";
  }
}

export type TaskArchiveFilter = "active" | "only" | "include";
export const TASK_ARCHIVE_FILTERS: TaskArchiveFilter[] = ["active", "only", "include"];

export function taskArchiveFilterLabel(filter: TaskArchiveFilter): string {
  switch (filter) {
    case "active":
      return "Active";
    case "only":
      return "Archived";
    case "include":
      return "All";
  }
}

// `TaskArchiveFilter` values already are `ArchivedFilter` values; the query
// builder in sidebar-state.ts passes the filter straight through.

// --- Collapsed group ids, kept per grouping mode ---------------------------

export interface CollapsedGroups {
  readonly byMode: Readonly<Record<string, readonly string[]>>;
}

export const EMPTY_COLLAPSED_GROUPS: CollapsedGroups = { byMode: {} };

/** Reads the persisted per-grouping JSON format. */
export function decodeCollapsedGroups(raw: string): CollapsedGroups {
  if (raw === "") return EMPTY_COLLAPSED_GROUPS;

  const firstMeaningful = Array.from(raw).find((character) => !/\s/.test(character));
  if (firstMeaningful === "{" || firstMeaningful === "[") {
    try {
      const decoded = JSON.parse(raw) as unknown;
      if (isRecordOfStringArrays(decoded)) {
        const byMode: Record<string, string[]> = {};
        for (const [mode, ids] of Object.entries(decoded)) {
          if (ids.length > 0) byMode[mode] = ids;
        }
        return { byMode };
      }
    } catch {
      // Falls through to the empty default, matching the malformed-JSON case.
    }
    return EMPTY_COLLAPSED_GROUPS;
  }

  return EMPTY_COLLAPSED_GROUPS;
}

function isRecordOfStringArrays(value: unknown): value is Record<string, string[]> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  return Object.values(value).every(
    (entry) => Array.isArray(entry) && entry.every((item) => typeof item === "string"),
  );
}

export function encodeCollapsedGroups(groups: CollapsedGroups): string {
  const modes = Object.keys(groups.byMode);
  if (modes.length === 0) return "";
  const sorted: Record<string, readonly string[]> = {};
  for (const mode of modes.sort()) sorted[mode] = groups.byMode[mode];
  return JSON.stringify(sorted);
}

export function collapsedGroupIds(groups: CollapsedGroups, mode: string): Set<string> {
  return new Set(groups.byMode[mode] ?? []);
}

export function isGroupCollapsed(groups: CollapsedGroups, id: string, mode: string): boolean {
  return (groups.byMode[mode] ?? []).includes(id);
}

const MAX_COLLAPSED_PER_MODE = 200;

export function toggleCollapsedGroup(groups: CollapsedGroups, id: string, mode: string): CollapsedGroups {
  const current = groups.byMode[mode] ?? [];
  const index = current.indexOf(id);
  let next: string[];
  if (index >= 0) {
    next = current.filter((_, position) => position !== index);
  } else {
    next = [...current, id];
    if (next.length > MAX_COLLAPSED_PER_MODE) {
      next = next.slice(next.length - MAX_COLLAPSED_PER_MODE);
    }
  }
  const byMode = { ...groups.byMode };
  if (next.length === 0) {
    delete byMode[mode];
  } else {
    byMode[mode] = next;
  }
  return { byMode };
}

// --- The persisted bundle ---------------------------------------------------

export interface SidebarPreferences {
  archiveFilter: TaskArchiveFilter;
  projectFilter: string | undefined;
  grouping: TaskGrouping;
  sort: TaskSort;
  collapsed: CollapsedGroups;
  sidebarCollapsed: boolean;
  sidebarWidth: number;
}

export function defaultSidebarPreferences(): SidebarPreferences {
  return {
    archiveFilter: "active",
    projectFilter: undefined,
    grouping: "parent",
    sort: "priority",
    collapsed: EMPTY_COLLAPSED_GROUPS,
    sidebarCollapsed: false,
    sidebarWidth: SIDEBAR_DEFAULT_WIDTH,
  };
}

export function loadSidebarPreferences(): SidebarPreferences {
  const preferences = defaultSidebarPreferences();

  const archive = readStorage(ARCHIVE_FILTER_KEY);
  if (archive === "only") preferences.archiveFilter = "only";
  else if (archive === "include") preferences.archiveFilter = "include";
  else if (archive !== undefined) preferences.archiveFilter = "active";

  const project = readStorage(PROJECT_FILTER_KEY);
  preferences.projectFilter = project && project !== "" ? project : undefined;

  const grouping = readStorage(GROUPING_KEY);
  if (grouping === "project" || grouping === "status" || grouping === "none") {
    preferences.grouping = grouping;
  } else if (grouping !== undefined) {
    preferences.grouping = "parent";
  }

  const sort = readStorage(SORT_KEY);
  if (sort === "recent" || sort === "updated") preferences.sort = sort;
  else if (sort !== undefined) preferences.sort = "priority";

  preferences.collapsed = decodeCollapsedGroups(readStorage(COLLAPSED_KEY) ?? "");
  preferences.sidebarCollapsed = readStorage(SIDEBAR_COLLAPSED_KEY) === "1";

  const width = readStorage(SIDEBAR_WIDTH_KEY);
  if (width !== undefined) {
    const parsed = Number.parseInt(width, 10);
    if (Number.isFinite(parsed)) {
      preferences.sidebarWidth = Math.min(Math.max(parsed, SIDEBAR_MIN_WIDTH), SIDEBAR_MAX_WIDTH);
    }
  }

  return preferences;
}

export function storeSidebarPreferences(preferences: SidebarPreferences): void {
  writeStorage(ARCHIVE_FILTER_KEY, preferences.archiveFilter);
  writeStorage(PROJECT_FILTER_KEY, preferences.projectFilter ?? "");
  writeStorage(GROUPING_KEY, preferences.grouping);
  writeStorage(SORT_KEY, preferences.sort);
  writeStorage(COLLAPSED_KEY, encodeCollapsedGroups(preferences.collapsed));
  writeStorage(SIDEBAR_COLLAPSED_KEY, preferences.sidebarCollapsed ? "1" : "0");
  writeStorage(SIDEBAR_WIDTH_KEY, String(preferences.sidebarWidth));
}
