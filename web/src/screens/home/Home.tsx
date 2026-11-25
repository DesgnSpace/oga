// The home screen: the sidebar's tasks, what needs you first, then finished work by day.

import { useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent, type ReactNode } from "react";
import { LoadingState } from "@/components/atoms/ListState";
import { InlineMarkdown } from "@/domain/markdown";
import { TaskStatusDot, taskStatusLook } from "@/components/atoms/TaskStatusDot";
import { handlesClick } from "@/router";
import { ArchiveBranchDialog, type ArchiveBranchTask } from "@/screens/task/Actions";
import { isExplainedWait, taskStatusLabel, waitLabel } from "@/screens/task/format";
import type { SidebarController } from "@/state";
import { displayLabel, matching, projectId, projectName } from "@/state/sidebar-projection";
import { filtersHideTasks, searchTerm, type SidebarState, type SidebarTask } from "@/state/sidebar-state";
import { useTaskArrivals } from "@/state/task-arrivals";
import { projectLabel, projectParents } from "@/state/task-labels";
import { taskOutcomeViews } from "@/state/task-outcome-views";
import { MoreIcon } from "@/ui/icons";
import { absoluteTime, relativeTime } from "@/ui/time";
import { TaskMenu } from "./TaskMenu";

const DAY_MS = 24 * 60 * 60 * 1000;

type SettingsTab = "workers" | "connections";

export interface HomeProps {
  sidebarController: SidebarController;
  onOpenSettings: (tab: SettingsTab) => void;
  onOpenTask: (id: string) => void;
}

function narrowed(state: SidebarState): boolean {
  return filtersHideTasks(state) || searchTerm(state) !== "";
}

export function Home({ sidebarController, onOpenSettings, onOpenTask }: HomeProps) {
  const sidebar = useSyncExternalStore((listener) => sidebarController.subscribe(listener), () => sidebarController.snapshot);
  const hasTasks = sidebar.tasks.length > 0;

  if (sidebar.loadState === "loading" && !hasTasks && !narrowed(sidebar)) {
    return (
      <div id="page-title" className="home-intro">
        <LoadingState label="Loading your tasks…" />
      </div>
    );
  }

  if (sidebar.loadState === "error" && !hasTasks) {
    return (
      <div className="home-intro">
        <h1 id="page-title" className="page-title">Couldn&apos;t load your tasks</h1>
        <p className="app-description">Check that Oga is running, then try again.</p>
        <button className="text-button" type="button" onClick={() => void sidebarController.refresh()}>
          Try again
        </button>
      </div>
    );
  }

  if (sidebar.loadState === "ready" && sidebar.profiles.length === 0) {
    return (
      <div className="home-intro">
        <div className="app-first-run-card">
          <h1 id="page-title" className="page-title">Connect your AI to start delegating</h1>
          <p className="app-description">Choose an AI account for the work you want to hand off.</p>
          <button className="settings-button settings-button-primary" type="button" onClick={() => onOpenSettings("workers")}>
            Connect your AI
          </button>
        </div>
      </div>
    );
  }

  if (sidebar.loadState === "ready" && !hasTasks && !narrowed(sidebar)) return <FirstRun onOpenSettings={onOpenSettings} />;

  return <TaskList controller={sidebarController} sidebar={sidebar} onOpenTask={onOpenTask} />;
}

function SettingsLink({ tab, onOpenSettings, children }: { tab: SettingsTab; onOpenSettings: (tab: SettingsTab) => void; children: ReactNode }) {
  return (
    <a
      href="/settings"
      onClick={(event) => {
        if (!handlesClick(event)) return;
        event.preventDefault();
        onOpenSettings(tab);
      }}
    >
      {children}
    </a>
  );
}

function FirstRun({ onOpenSettings }: { onOpenSettings: (tab: SettingsTab) => void }) {
  return (
    <div className="home-intro">
      <h1 id="page-title" className="page-title">Get your first task running</h1>
      <ol className="app-first-run-steps">
        <li>
          Add a worker in <SettingsLink tab="workers" onOpenSettings={onOpenSettings}>Open worker settings</SettingsLink>.
        </li>
        <li>
          Connect your editor or agent in{" "}
          <SettingsLink tab="connections" onOpenSettings={onOpenSettings}>Open connection settings</SettingsLink>.
        </li>
        <li>Delegate a task from there. It shows up here.</li>
      </ol>
    </div>
  );
}

interface TaskSection {
  label: string;
  tasks: SidebarTask[];
}

function updatedMs(task: SidebarTask): number {
  return Date.parse(task.updatedAt) || 0;
}

/** The sidebar's search and project filter narrow home too; its other filters already shaped the read. */
function listedTasks(state: SidebarState): SidebarTask[] {
  return matching(state.tasks, searchTerm(state))
    .filter((task) => state.projectFilter === undefined || projectId(task) === state.projectFilter)
    .sort((left, right) => updatedMs(right) - updatedMs(left));
}

/** Archived work never leads, whatever its state. */
function attentionLabel(task: SidebarTask): string | undefined {
  if (task.archivedAt !== undefined) return undefined;
  switch (taskStatusLook(task.state)) {
    case "needs_input":
    case "problem":
      return "Needs you";
    case "running":
    case "waiting":
      return "In progress";
    case "settled":
      return undefined;
  }
}

function dayLabel(task: SidebarTask, today: number): string {
  const time = updatedMs(task);
  if (time >= today) return "Today";
  if (time >= today - DAY_MS) return "Yesterday";
  return "Earlier";
}

/** `tasks` come newest first, so the day sections fall in order. */
function sectionsFor(tasks: SidebarTask[], now: Date): TaskSection[] {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const sections = new Map<string, SidebarTask[]>([["Needs you", []], ["In progress", []]]);
  for (const task of tasks) {
    const label = attentionLabel(task) ?? dayLabel(task, today);
    sections.set(label, [...(sections.get(label) ?? []), task]);
  }
  return [...sections].filter(([, rows]) => rows.length > 0).map(([label, rows]) => ({ label, tasks: rows }));
}

/** The broker's true total for the sidebar's filters, or `undefined` before the first read. */
function taskTotal(groupCounts: Readonly<Record<string, number>>): number | undefined {
  const counts = Object.values(groupCounts);
  return counts.length === 0 ? undefined : counts.reduce((total, count) => total + count, 0);
}

function countLabel(total: number): string {
  return `${total} ${total === 1 ? "task" : "tasks"}`;
}

function subtitle(state: SidebarState): string {
  const term = searchTerm(state);
  if (term !== "") return `Titles matching “${term}”`;
  const project = state.projectFilter === undefined ? "" : ` in ${projectName(state.projectFilter)}`;
  switch (state.archiveFilter) {
    case "only":
      return `Archived tasks${project}`;
    case "include":
      return `Active and archived tasks${project}`;
    case "active":
      return project === "" ? "What needs you first, then the latest work" : `Tasks${project}`;
  }
}

function emptyMessage(state: SidebarState): string {
  const term = searchTerm(state);
  if (term !== "") return `No tasks match “${term}”.`;
  if (state.archiveFilter === "only") return "No archived tasks.";
  if (state.projectFilter !== undefined) return `No tasks in ${projectName(state.projectFilter)}.`;
  return "No tasks yet.";
}

function workerLabel(task: SidebarTask, state: SidebarState): string {
  const label = state.profiles.find((profile) => profile.id === task.profileId)?.label;
  return label ?? task.model.slice(task.model.lastIndexOf("/") + 1);
}

function rowMeta(task: SidebarTask, state: SidebarState, parents: ReadonlyMap<string, string>): string {
  const parts = [state.projectFilter === undefined ? projectLabel(projectId(task), parents) : undefined, workerLabel(task, state)];
  if (task.originCwd !== undefined && task.branch !== undefined) parts.push(task.branch);
  // A task nobody stopped is waiting for something; the row says what, so it does not read as stalled.
  if (isExplainedWait(task.hold)) parts.push(waitLabel(task.hold));
  return parts.filter((part) => part !== undefined).join(" · ");
}

interface OpenMenu {
  task: SidebarTask;
  anchor: HTMLElement;
}

function TaskList({ controller, sidebar, onOpenTask }: { controller: SidebarController; sidebar: SidebarState; onOpenTask: (id: string) => void }) {
  // Rows read whether each outcome was seen, so the list redraws when that changes.
  useSyncExternalStore(taskOutcomeViews.subscribe, () => taskOutcomeViews.snapshot, () => 0);
  const arrivedTaskIds = useTaskArrivals(sidebar.tasks);
  const parents = useMemo(() => projectParents(sidebar.tasks.map(projectId)), [sidebar.tasks]);
  const tasks = useMemo(() => listedTasks(sidebar), [sidebar.tasks, sidebar.search, sidebar.projectFilter]);
  const sections = useMemo(() => sectionsFor(tasks, new Date()), [tasks]);
  const [focusedId, setFocusedId] = useState<string>();
  const [menu, setMenu] = useState<OpenMenu>();
  const [branchTask, setBranchTask] = useState<ArchiveBranchTask>();
  const rowRefs = useRef(new Map<string, HTMLButtonElement>());
  const ordered = sections.flatMap((section) => section.tasks);
  const focused = ordered.find((task) => task.id === focusedId) ?? ordered[0];
  const total = taskTotal(sidebar.groupCounts);
  const loadingList = sidebar.loadState === "loading" && sidebar.tasks.length === 0;

  const moveFocus = (event: KeyboardEvent<HTMLDivElement>) => {
    const index = ordered.findIndex((task) => task.id === focused?.id);
    let next: number | undefined;
    if (event.key === "ArrowDown") next = Math.min(index + 1, ordered.length - 1);
    if (event.key === "ArrowUp") next = Math.max(index - 1, 0);
    if (event.key === "Home") next = 0;
    if (event.key === "End") next = ordered.length - 1;
    if (next === undefined || ordered.length === 0) return;
    event.preventDefault();
    rowRefs.current.get(ordered[next].id)?.focus();
  };

  return (
    <div className="home">
      <header className="home-head">
        <div>
          <h1 id="page-title" className="home-title">Recent</h1>
          <p className="home-subtitle">{subtitle(sidebar)}</p>
        </div>
        {total !== undefined && <span className="home-count">{countLabel(total)}</span>}
      </header>

      {loadingList ? (
        <LoadingState label="Loading tasks…" />
      ) : ordered.length === 0 ? (
        <div className="home-empty">
          <p>{emptyMessage(sidebar)}</p>
          {searchTerm(sidebar) !== "" ? (
            <button className="text-button" type="button" onClick={() => controller.setSearch("")}>
              Clear search
            </button>
          ) : filtersHideTasks(sidebar) && <p>Try changing the sidebar&apos;s filters.</p>}
        </div>
      ) : (
        <div id="home-tasks" className="home-sections" onKeyDown={moveFocus}>
          {sections.map((section) => (
            <section key={section.label} aria-label={section.label}>
              <h2 className="home-section-label">{section.label}</h2>
              <ul className="home-list">
                {section.tasks.map((task) => (
                  <TaskRow
                    key={task.id}
                    task={task}
                    meta={rowMeta(task, sidebar, parents)}
                    focused={task.id === focused?.id}
                    arrived={arrivedTaskIds.has(task.id)}
                    menuOpen={menu?.task.id === task.id}
                    rowRef={(element) => {
                      if (element) rowRefs.current.set(task.id, element);
                      else rowRefs.current.delete(task.id);
                    }}
                    onFocus={() => setFocusedId(task.id)}
                    onOpen={() => onOpenTask(task.id)}
                    onMenu={(anchor) => setMenu({ task, anchor })}
                  />
                ))}
              </ul>
            </section>
          ))}
        </div>
      )}

      {sidebar.tasksHasMore && (total === undefined || total > tasks.length) && !loadingList && (
        <div className="home-more">
          {sidebar.loadMoreFailed && <span role="status">Couldn&apos;t load older tasks.</span>}
          <button className="text-button" type="button" disabled={sidebar.isLoadingMore} onClick={() => void controller.loadMore()}>
            {sidebar.isLoadingMore ? "Loading…" : sidebar.loadMoreFailed ? "Try again" : "Show older tasks"}
          </button>
        </div>
      )}

      {menu && (
        <TaskMenu
          task={menu.task}
          anchor={menu.anchor}
          offline={sidebar.connection !== "connected"}
          onClose={() => setMenu(undefined)}
          onChanged={() => void controller.refresh()}
          onDeleteBranch={() => {
            const task = menu.task;
            setMenu(undefined);
            if (task.branch !== undefined) setBranchTask({ ...task, branch: task.branch });
          }}
        />
      )}

      {branchTask && (
        <ArchiveBranchDialog
          task={branchTask}
          open
          onClose={() => setBranchTask(undefined)}
          onChanged={() => {
            setBranchTask(undefined);
            void controller.refresh();
          }}
        />
      )}
    </div>
  );
}

function TaskRow({ task, meta, focused, arrived, menuOpen, rowRef, onFocus, onOpen, onMenu }: {
  task: SidebarTask;
  meta: string;
  /** The one row Tab lands on; arrow keys move it. */
  focused: boolean;
  arrived: boolean;
  menuOpen: boolean;
  rowRef: (element: HTMLButtonElement | null) => void;
  onFocus: () => void;
  onOpen: () => void;
  onMenu: (anchor: HTMLElement) => void;
}) {
  const title = displayLabel(task);
  const status = taskStatusLabel(task);
  const viewed = taskOutcomeViews.isViewed(task);
  // The dot and the section already say running, finished, or needs you; a word is kept for what went wrong.
  const tag = taskStatusLook(task.state) === "problem" ? status : undefined;
  return (
    <li
      className="home-row"
      data-menu-open={menuOpen || undefined}
      data-arrived={arrived || undefined}
      onContextMenu={(event) => {
        event.preventDefault();
        onMenu(event.currentTarget);
      }}
    >
      <button
        ref={rowRef}
        className="home-row-main"
        type="button"
        tabIndex={focused ? 0 : -1}
        onFocus={onFocus}
        onClick={onOpen}
      >
        <TaskStatusDot state={task.state} label={status} unread={!viewed} decorative />
        <span className="visually-hidden">{`${status} · ${viewed ? "Viewed" : "New update"}`}</span>
        <span className="home-row-text">
          <strong><InlineMarkdown source={title} /></strong>
          <small>{meta}</small>
        </span>
        {tag && <span className="home-tag">{tag}</span>}
        <time className="home-row-time" dateTime={task.updatedAt} title={absoluteTime(task.updatedAt)}>
          {relativeTime(task.updatedAt)}
        </time>
      </button>
      <button
        className="icon-button home-row-menu"
        type="button"
        aria-label={`Task actions for ${title}`}
        title="Task actions"
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        onClick={(event) => onMenu(event.currentTarget)}
      >
        <MoreIcon />
      </button>
    </li>
  );
}
