// Task grouping, projection, and bounded rendering primitives.
// Ported from rust/crates/oga-ui/src/sidebar/mod.rs — keep behavior identical.

import type { TaskState, TaskSummary } from "@/bridge/types";
import { taskStateLabel } from "@/screens/task/format";
import { collapsedGroupIds, type CollapsedGroups, type TaskGrouping, type TaskSort } from "./sidebar-preferences";
import { filtersHideTasks, searchTerm, type SidebarState } from "./sidebar-state";

export interface TaskProject {
  id: string;
  name: string;
  count: number;
}

export interface TaskGroup {
  id: string;
  title: string | undefined;
  tasks: TaskSummary[];
}

export type ProjectSidebarNode =
  | { type: "group"; group: TaskGroup }
  | { type: "parent"; id: string; title: string; children: TaskGroup[] };

export function projectSidebarNodeId(node: ProjectSidebarNode): string {
  return node.type === "group" ? node.group.id : node.id;
}

export interface SidebarProjection {
  groups: TaskGroup[];
  projectNodes: ProjectSidebarNode[] | undefined;
  projects: TaskProject[];
}

export type TaskUnread = (task: TaskSummary) => boolean;

export function projectionFromState(state: SidebarState, isUnread?: TaskUnread): SidebarProjection {
  const groups = organize(matching(state.tasks, searchTerm(state)), state.projectFilter, state.grouping, state.sort, isUnread);
  const projectNodes = state.grouping === "project" ? projectTree(groups) : undefined;
  return { groups, projectNodes, projects: projects(state.tasks) };
}

export function projectionTaskCount(projection: SidebarProjection): number {
  return projection.groups.reduce((total, group) => total + group.tasks.length, 0);
}

export const NO_TASKS_MESSAGE = "No tasks yet";

/** Explains an empty list: nothing started yet, or nothing left after
 * searching and filtering. `undefined` while the list has rows. */
export function projectionEmptyMessage(projection: SidebarProjection, state: SidebarState): string | undefined {
  if (projectionTaskCount(projection) > 0) return undefined;
  const hasSearch = searchTerm(state) !== "";
  const hasFilters = filtersHideTasks(state);
  if (hasSearch && hasFilters) return "No tasks match your search and filters.";
  if (hasSearch) return "No tasks match your search.";
  if (hasFilters) return "No tasks match your filters.";
  return NO_TASKS_MESSAGE;
}

export function projectionVisibleTasks(
  projection: SidebarProjection,
  collapsed: CollapsedGroups,
  grouping: TaskGrouping,
): TaskSummary[] {
  const collapsedIds = collapsedGroupIds(collapsed, grouping);
  if (projection.projectNodes) {
    return projection.projectNodes.flatMap((node) => visibleTasksInNode(node, collapsedIds));
  }
  return projection.groups.flatMap((group) => visibleTasksInGroup(group, collapsedIds));
}

export function projectionListedTaskIds(
  projection: SidebarProjection,
  collapsed: CollapsedGroups,
  grouping: TaskGrouping,
): string[] {
  return projectionVisibleTasks(projection, collapsed, grouping).map((task) => task.id);
}

export type SidebarRow =
  | { type: "groupHeader"; id: string; title: string; count: number; collapsed: boolean; indented: boolean }
  | { type: "task"; task: TaskSummary; indented: boolean };

export function sidebarRowKey(row: SidebarRow): string {
  if (row.type === "groupHeader") return `group:${row.id}:${row.collapsed}:${row.indented}`;
  const task = row.task;
  return `task:${task.id}:${task.cwd}:${task.profileId}:${task.model}:${task.updatedAt}:${task.state}:${task.title ?? ""}:${row.indented}`;
}

export function projectionRows(
  projection: SidebarProjection,
  collapsed: CollapsedGroups,
  grouping: TaskGrouping,
): SidebarRow[] {
  const collapsedIds = collapsedGroupIds(collapsed, grouping);
  if (projection.projectNodes) {
    return projection.projectNodes.flatMap((node) => rowsForNode(node, collapsedIds));
  }
  return projection.groups.flatMap((group) => rowsForGroup(group, collapsedIds, false));
}

function rowsForNode(node: ProjectSidebarNode, collapsed: Set<string>): SidebarRow[] {
  if (node.type === "group") return rowsForGroup(node.group, collapsed, false);
  const rows: SidebarRow[] = [
    {
      type: "groupHeader",
      id: node.id,
      title: node.title,
      count: node.children.reduce((total, group) => total + group.tasks.length, 0),
      collapsed: collapsed.has(node.id),
      indented: false,
    },
  ];
  if (!collapsed.has(node.id)) {
    for (const group of node.children) rows.push(...rowsForGroup(group, collapsed, true));
  }
  return rows;
}

function rowsForGroup(group: TaskGroup, collapsed: Set<string>, indented: boolean): SidebarRow[] {
  const rows: SidebarRow[] = [];
  if (group.title !== undefined) {
    rows.push({
      type: "groupHeader",
      id: group.id,
      title: group.title,
      count: group.tasks.length,
      collapsed: collapsed.has(group.id),
      indented,
    });
  }
  if (group.title === undefined || !collapsed.has(group.id)) {
    for (const task of group.tasks) rows.push({ type: "task", task, indented });
  }
  return rows;
}

// --- Virtualised list geometry ---------------------------------------------

export interface VirtualList {
  itemCount: number;
  itemHeight: number;
  viewportHeight: number;
  scrollOffset: number;
  overscan: number;
}

/** Must match `.sidebar-task` and `.sidebar-group` in style.css. */
export const VIRTUAL_LIST_DEFAULT_ITEM_HEIGHT = 44;
export const VIRTUAL_LIST_DEFAULT_VIEWPORT_HEIGHT = 480;
export const VIRTUAL_LIST_DEFAULT_OVERSCAN = 4;

export function newVirtualList(itemCount: number): VirtualList {
  return {
    itemCount,
    itemHeight: VIRTUAL_LIST_DEFAULT_ITEM_HEIGHT,
    viewportHeight: VIRTUAL_LIST_DEFAULT_VIEWPORT_HEIGHT,
    scrollOffset: 0,
    overscan: VIRTUAL_LIST_DEFAULT_OVERSCAN,
  };
}

export function withItemHeight(list: VirtualList, height: number): VirtualList {
  return { ...list, itemHeight: Math.max(height, 1) };
}

export function withViewportHeight(list: VirtualList, height: number): VirtualList {
  return { ...list, viewportHeight: height };
}

export function withScrollOffset(list: VirtualList, offset: number): VirtualList {
  return { ...list, scrollOffset: offset };
}

export function withOverscan(list: VirtualList, overscan: number): VirtualList {
  return { ...list, overscan };
}

export function virtualListTotalHeight(list: VirtualList): number {
  return list.itemHeight * list.itemCount;
}

export function virtualListVisibleRange(list: VirtualList): [number, number] {
  if (list.itemCount === 0) return [0, 0];
  const first = Math.floor(list.scrollOffset / list.itemHeight);
  const visible = Math.floor((list.viewportHeight + list.itemHeight - 1) / list.itemHeight);
  const start = Math.min(Math.max(first - list.overscan, 0), list.itemCount);
  const end = Math.min(first + visible + list.overscan, list.itemCount);
  return [start, Math.max(end, start)];
}

export function virtualListOffsetFor(list: VirtualList, index: number): number {
  return list.itemHeight * index;
}

// --- Organising tasks into groups -------------------------------------------

const MAX_PARENT_DEPTH = 32;
export const HEADING_LIMIT = 38;
/** Longest project a row prints before it crowds out the time and cost. */
const ROW_PROJECT_LIMIT = 22;

function clamped(text: string, limit: number): string {
  const characters = Array.from(text);
  if (characters.length <= limit) return text;
  return `${characters.slice(0, limit - 1).join("")}…`;
}

export function projectName(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  const parts = trimmed.split("/");
  for (let index = parts.length - 1; index >= 0; index -= 1) {
    if (parts[index] !== "") return parts[index];
  }
  return path;
}

function projectId(task: TaskSummary): string {
  return task.originCwd ?? task.cwd;
}

/**
 * Where a task runs, short enough for a row: the project, and for work with a
 * copy of its own the branch that copy holds, so two copies of one project
 * never read as the same place.
 */
export function taskProjectLabel(task: TaskSummary): string {
  const project = projectName(projectId(task));
  const ownCopy = task.originCwd !== undefined && task.branch !== undefined;
  return clamped(ownCopy ? `${project}/${task.branch}` : project, ROW_PROJECT_LIMIT);
}

export function projects(tasks: TaskSummary[]): TaskProject[] {
  const order: string[] = [];
  const counts = new Map<string, number>();
  for (const task of tasks) {
    const id = projectId(task);
    if (!counts.has(id)) order.push(id);
    counts.set(id, (counts.get(id) ?? 0) + 1);
  }
  return order.map((id) => ({ id, name: projectName(id), count: counts.get(id) ?? 0 }));
}

/** Narrows the list to tasks whose title contains `term`, ignoring case. */
export function matching(tasks: TaskSummary[], term: string): TaskSummary[] {
  if (term === "") return tasks.slice();
  const needle = term.toLowerCase();
  return tasks.filter((task) => displayLabel(task).toLowerCase().includes(needle));
}

export function organize(
  tasks: TaskSummary[],
  project: string | undefined,
  grouping: TaskGrouping,
  sort: TaskSort,
  isUnread?: TaskUnread,
): TaskGroup[] {
  const known = new Set(tasks.map((task) => projectId(task)));
  const activeProject = project !== undefined && known.has(project) ? project : undefined;
  const scoped = tasks.filter((task) => activeProject === undefined || projectId(task) === activeProject);

  switch (grouping) {
    case "none":
      return [{ id: "all", title: undefined, tasks: sorted(scoped, sort, isUnread) }];
    case "project":
      return sortedGroups(
        bucket(scoped, (task) => [projectId(task), projectName(projectId(task))]),
        sort,
        isUnread,
      );
    case "parent":
      return sortedGroups(parentGroups(scoped), sort, isUnread);
    case "status":
      return sortedGroups(statusGroups(scoped), sort, isUnread);
  }
}

export function activeProjectName(tasks: TaskSummary[], project: string | undefined): string | undefined {
  if (project === undefined || !tasks.some((task) => projectId(task) === project)) return undefined;
  return projectName(project);
}

export function projectTree(groups: TaskGroup[]): ProjectSidebarNode[] {
  const groupIds = new Set(groups.map((group) => group.id));
  const childrenByParent = new Map<string, TaskGroup[]>();
  for (const group of groups) {
    const parent = parentDirectory(group.id);
    const children = childrenByParent.get(parent) ?? [];
    children.push(group);
    childrenByParent.set(parent, children);
  }

  const seenParents = new Set<string>();
  const nodes: ProjectSidebarNode[] = [];
  for (const group of groups) {
    const parent = parentDirectory(group.id);
    const siblings = childrenByParent.get(parent);
    if (!siblings) continue;
    if (siblings.length <= 1 || groupIds.has(parent)) {
      nodes.push({ type: "group", group });
      continue;
    }
    if (seenParents.has(parent)) continue;
    seenParents.add(parent);
    nodes.push({ type: "parent", id: parent, title: projectName(parent), children: siblings });
  }
  return nodes;
}

export function visibleTasksInGroup(group: TaskGroup, collapsed: Set<string>): TaskSummary[] {
  if (group.title !== undefined && collapsed.has(group.id)) return [];
  return group.tasks;
}

export function visibleTasksInNode(node: ProjectSidebarNode, collapsed: Set<string>): TaskSummary[] {
  if (node.type === "group") return visibleTasksInGroup(node.group, collapsed);
  if (collapsed.has(node.id)) return [];
  return node.children.flatMap((group) => visibleTasksInGroup(group, collapsed));
}

export function heading(label: string): string {
  const firstLine = (label.split("\n")[0] ?? label).trim();
  return clamped(firstLine, HEADING_LIMIT);
}

export function displayLabel(task: TaskSummary): string {
  const title = task.title?.trim();
  if (title) return title;
  return task.promptPreview.split("\n")[0] || "Untitled task";
}

export function neighborAfterRemoving(id: string, ids: string[]): string | undefined {
  const index = ids.indexOf(id);
  if (index === -1) return ids[0];
  return ids[index + 1] ?? ids[index - 1];
}

function parentGroups(tasks: TaskSummary[]): TaskGroup[] {
  const byId = new Map(tasks.map((task) => [task.id, task]));
  const groups = bucket(tasks, (task) => {
    const rootTask = root(task, byId);
    return [rootTask.id, heading(displayLabel(rootTask))];
  });
  return groups.map((group) => {
    const solo = group.tasks.length === 1 && group.tasks[0].parentTaskId === undefined;
    return solo ? { id: group.id, title: undefined, tasks: group.tasks } : group;
  });
}

function root(task: TaskSummary, byId: Map<string, TaskSummary>): TaskSummary {
  let current = task;
  const seen = new Set([current.id]);
  for (let depth = 0; depth < MAX_PARENT_DEPTH; depth += 1) {
    const parentId = current.parentTaskId;
    if (parentId === undefined) return current;
    const parent = byId.get(parentId);
    if (!parent) return current;
    if (seen.has(parentId)) return current;
    seen.add(parentId);
    current = parent;
  }
  return current;
}

function bucket(tasks: TaskSummary[], key: (task: TaskSummary) => [string, string]): TaskGroup[] {
  const order: string[] = [];
  const titles = new Map<string, string>();
  const buckets = new Map<string, TaskSummary[]>();
  for (const task of tasks) {
    const [id, title] = key(task);
    if (!buckets.has(id)) {
      order.push(id);
      titles.set(id, title);
      buckets.set(id, []);
    }
    buckets.get(id)!.push(task);
  }
  return order.map((id) => ({ id, title: titles.get(id), tasks: buckets.get(id) ?? [] }));
}

/**
 * How much each state needs the user, most first: needs_input blocks on a
 * reply; failed, cancelled, and blocked all stopped short of finishing and
 * need a decision; the rest are progressing or already done and need
 * nothing right now.
 */
const PRIORITY_ORDER: TaskState[] = [
  "needs_input",
  "failed",
  "cancelled",
  "blocked",
  "running",
  "queued",
  "pending",
  "answered",
  "completed",
];

function statusGroups(tasks: TaskSummary[]): TaskGroup[] {
  const buckets = new Map<TaskState, TaskSummary[]>();
  for (const task of tasks) {
    const bucketTasks = buckets.get(task.state) ?? [];
    bucketTasks.push(task);
    buckets.set(task.state, bucketTasks);
  }
  const groups: TaskGroup[] = [];
  for (const state of PRIORITY_ORDER) {
    const bucketTasks = buckets.get(state);
    if (bucketTasks) groups.push({ id: state, title: taskStateLabel(state), tasks: bucketTasks });
  }
  return groups;
}

function sorted(tasks: TaskSummary[], sort: TaskSort, isUnread?: TaskUnread): TaskSummary[] {
  switch (sort) {
    case "recent":
      return tasks.slice().sort((left, right) => compareTimes(right.createdAt, left.createdAt));
    case "updated":
      return tasks.slice().sort((left, right) => compareTimes(right.updatedAt, left.updatedAt));
    case "priority":
      // New outcomes float above read tasks of the same priority so they are
      // easy to find. Date sorts keep their selected meaning untouched.
      if (isUnread === undefined) {
        return tasks.slice().sort((left, right) => priorityRank(left.state) - priorityRank(right.state));
      }
      return tasks.slice().sort((left, right) => {
        const priority = priorityRank(left.state) - priorityRank(right.state);
        if (priority !== 0) return priority;
        return unreadRank(left, isUnread) - unreadRank(right, isUnread);
      });
  }
}

function unreadRank(task: TaskSummary, isUnread: TaskUnread): number {
  return isUnread(task) ? 0 : 1;
}

function sortedGroups(groups: TaskGroup[], sort: TaskSort, isUnread?: TaskUnread): TaskGroup[] {
  return groups.map((group) => ({ ...group, tasks: sorted(group.tasks, sort, isUnread) }));
}

function parentDirectory(path: string): string {
  const trimmed = path.endsWith("/") && path.length > 1 ? path.slice(0, -1) : path;
  const slash = trimmed.lastIndexOf("/");
  if (slash === -1) return path;
  if (slash === 0) return path;
  return trimmed.slice(0, slash);
}

function priorityRank(state: TaskState): number {
  return PRIORITY_ORDER.indexOf(state);
}

/** Unreadable timestamps sort as the oldest, so a bad row cannot claim the top. */
function compareTimes(left: string, right: string): number {
  const leftMs = Date.parse(left);
  const rightMs = Date.parse(right);
  const leftValid = !Number.isNaN(leftMs);
  const rightValid = !Number.isNaN(rightMs);
  if (leftValid && rightValid) return leftMs - rightMs;
  if (leftValid) return 1;
  if (rightValid) return -1;
  return 0;
}
