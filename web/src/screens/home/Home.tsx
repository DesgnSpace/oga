// The sidebar's current view as one list beside a preview of the picked task.

import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent, type ReactNode } from "react";
import { LoadingState } from "@/components/atoms/ListState";
import { InlineMarkdown } from "@/domain/markdown";
import { TaskStatusDot, taskStatusLook } from "@/components/atoms/TaskStatusDot";
import { handlesClick } from "@/router";
import { ArchiveBranchDialog, type ArchiveBranchTask } from "@/screens/task/Actions";
import { isExplainedWait, taskStatusLabel, waitLabel } from "@/screens/task/format";
import type { SidebarController } from "@/state";
import { displayLabel, matching, projectId, projectLabel, projectName, projectParents } from "@/state/task-labels";
import { searchTerm, type SidebarState, type SidebarTask } from "@/state/sidebar-state";
import { useTaskArrivals } from "@/state/task-arrivals";
import { taskOutcomeViews } from "@/state/task-outcome-views";
import { needsYou, inProgress, taskView, viewIncludes, viewTotal, type TaskView } from "@/state/task-views";
import { MoreIcon } from "@/ui/icons";
import { absoluteTime, relativeTime } from "@/ui/time";
import { TaskMenu } from "./TaskMenu";
import { statusTag, TaskPreview } from "./TaskPreview";

/** Below this width the preview has no room, so a pick opens the task. */
const PREVIEW_MIN_WIDTH = 720;

/** Attention views load older pages on their own until every task they count is listed. */
const SELF_LOADING_VIEWS: ReadonlySet<TaskView> = new Set(["needs-you", "in-progress"]);

const DAY_MS = 24 * 60 * 60 * 1000;

type SettingsTab = "workers" | "connections";

export interface HomeProps {
  sidebarController: SidebarController;
  onOpenSettings: (tab: SettingsTab) => void;
  onOpenTask: (id: string) => void;
}

function useSidebar(controller: SidebarController): SidebarState {
  return useSyncExternalStore((listener) => controller.subscribe(listener), () => controller.snapshot);
}

function narrowed(state: SidebarState): boolean {
  return state.view !== "recent" || state.projectFilter !== undefined || searchTerm(state) !== "";
}

export function Home({ sidebarController, onOpenSettings, onOpenTask }: HomeProps) {
  const sidebar = useSidebar(sidebarController);
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

  return <TaskBoard controller={sidebarController} sidebar={sidebar} onOpenTask={onOpenTask} />;
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
  id: string;
  /** Absent when the view's own title already names the rows. */
  label?: string;
  tasks: SidebarTask[];
}

function updatedMs(task: SidebarTask): number {
  return Date.parse(task.updatedAt) || 0;
}

function listedTasks(state: SidebarState): SidebarTask[] {
  const archived = state.archiveFilter === "only";
  return matching(state.tasks, searchTerm(state))
    .filter((task) => (task.archivedAt !== undefined) === archived)
    .filter((task) => state.projectFilter === undefined || projectId(task) === state.projectFilter)
    .filter((task) => viewIncludes(state.view, task))
    .sort((left, right) => updatedMs(right) - updatedMs(left));
}

function dayLabel(task: SidebarTask, today: number): string {
  const time = updatedMs(task);
  if (time >= today) return "Today";
  if (time >= today - DAY_MS) return "Yesterday";
  return "Earlier";
}

function sectionsFor(tasks: SidebarTask[], view: TaskView, now: Date): TaskSection[] {
  if (view === "needs-you" || view === "in-progress") return [{ id: view, tasks }];
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const sections: TaskSection[] = [];
  const add = (id: string, task: SidebarTask) => {
    const section = sections.find((candidate) => candidate.id === id);
    if (section) section.tasks.push(task);
    else sections.push({ id, label: id, tasks: [task] });
  };
  const waiting = tasks.filter(needsYou);
  const running = tasks.filter(inProgress);
  if (view !== "archived") {
    waiting.forEach((task) => add("Needs you", task));
    running.forEach((task) => add("In progress", task));
  }
  for (const task of tasks) {
    if (view !== "archived" && (needsYou(task) || inProgress(task))) continue;
    add(dayLabel(task, today), task);
  }
  return sections;
}

function countLabel(total: number): string {
  return `${total} ${total === 1 ? "task" : "tasks"}`;
}

function workerLabel(task: SidebarTask, state: SidebarState): string {
  const label = state.profiles.find((profile) => profile.id === task.profileId)?.label;
  return label ?? task.model.slice(task.model.lastIndexOf("/") + 1);
}

interface ListHeading {
  title: string;
  description: string;
}

function heading(state: SidebarState, parents: ReadonlyMap<string, string>): ListHeading {
  const term = searchTerm(state);
  const project = state.projectFilter;
  const title = project === undefined ? taskView(state.view).label : projectLabel(project, parents);
  if (term !== "") return { title, description: `Titles matching “${term}”` };
  if (project !== undefined) return { title, description: project };
  return { title, description: taskView(state.view).description };
}

function emptyMessage(state: SidebarState): string {
  const term = searchTerm(state);
  if (term !== "") return `No tasks match “${term}”.`;
  if (state.projectFilter !== undefined) return `No tasks in ${projectName(state.projectFilter)}.`;
  switch (state.view) {
    case "needs-you":
      return "Nothing needs you right now.";
    case "in-progress":
      return "Nothing is running right now.";
    case "finished":
      return "No finished tasks yet.";
    case "archived":
      return "No archived tasks.";
    case "recent":
      return "No tasks yet.";
  }
}

function useWidth(): [(element: HTMLDivElement | null) => void, number | undefined] {
  const [width, setWidth] = useState<number>();
  const observer = useRef<ResizeObserver | undefined>(undefined);
  const ref = useCallback((element: HTMLDivElement | null) => {
    observer.current?.disconnect();
    if (!element || !("ResizeObserver" in globalThis)) return;
    observer.current = new ResizeObserver(([entry]) => setWidth(entry?.contentRect.width));
    observer.current.observe(element);
  }, []);
  return [ref, width];
}

interface OpenMenu {
  task: SidebarTask;
  anchor: HTMLElement;
}

function TaskBoard({ controller, sidebar, onOpenTask }: { controller: SidebarController; sidebar: SidebarState; onOpenTask: (id: string) => void }) {
  // Rows read whether each outcome was seen, so the board redraws when that changes.
  useSyncExternalStore(taskOutcomeViews.subscribe, () => taskOutcomeViews.snapshot, () => 0);
  const arrivedTaskIds = useTaskArrivals(sidebar.tasks);
  const parents = useMemo(
    () => projectParents([...sidebar.tasks.map(projectId), ...Object.keys(sidebar.overview.projects)]),
    [sidebar.tasks, sidebar.overview.projects],
  );
  const tasks = useMemo(
    () => listedTasks(sidebar),
    [sidebar.tasks, sidebar.search, sidebar.view, sidebar.projectFilter, sidebar.archiveFilter],
  );
  const sections = useMemo(() => sectionsFor(tasks, sidebar.view, new Date()), [tasks, sidebar.view]);
  const ordered = useMemo(() => sections.flatMap((section) => section.tasks), [sections]);
  const picked = ordered.find((task) => task.id === sidebar.selectedTask) ?? ordered[0];
  const [boardRef, width] = useWidth();
  const previewFits = width === undefined || width >= PREVIEW_MIN_WIDTH;
  const [menu, setMenu] = useState<OpenMenu>();
  const [branchTask, setBranchTask] = useState<ArchiveBranchTask>();
  const rowRefs = useRef(new Map<string, HTMLButtonElement>());
  const total = viewTotal(sidebar.view, sidebar.groupCounts);
  const { title, description } = heading(sidebar, parents);
  const offline = sidebar.connection !== "connected";
  const loadingView = sidebar.loadState === "loading" && sidebar.tasks.length === 0;
  const missing = total !== undefined && total > tasks.length;

  useEffect(() => {
    if (!SELF_LOADING_VIEWS.has(sidebar.view) || !missing) return;
    if (sidebar.tasksHasMore && !sidebar.isLoadingMore && !sidebar.loadMoreFailed) void controller.loadMore();
  }, [controller, missing, sidebar.view, sidebar.tasksHasMore, sidebar.isLoadingMore, sidebar.loadMoreFailed]);

  const pick = useCallback((id: string) => controller.selectTask(id), [controller]);

  const open = useCallback((id: string) => {
    pick(id);
    onOpenTask(id);
  }, [pick, onOpenTask]);

  const moveFocus = (event: KeyboardEvent<HTMLDivElement>) => {
    if (ordered.length === 0) return;
    const index = ordered.findIndex((task) => task.id === picked?.id);
    let next: number | undefined;
    if (event.key === "ArrowDown") next = Math.min(index + 1, ordered.length - 1);
    if (event.key === "ArrowUp") next = Math.max(index - 1, 0);
    if (event.key === "Home") next = 0;
    if (event.key === "End") next = ordered.length - 1;
    if (next === undefined) return;
    event.preventDefault();
    const target = ordered[next];
    pick(target.id);
    rowRefs.current.get(target.id)?.focus();
  };

  // Archived work waits on nobody, so the archive suggests nothing.
  const alsoWaiting = picked && sidebar.view !== "archived"
    ? ordered.filter((task) => needsYou(task) && task.id !== picked.id).slice(0, 3)
    : [];

  return (
    <div ref={boardRef} className="home" data-preview={previewFits && picked !== undefined}>
      <div className="home-list-pane">
        <header className="home-head">
          <div>
            <h1 id="page-title" className="home-title">{title}</h1>
            <p className="home-subtitle" title={description}>{description}</p>
          </div>
          {total !== undefined && <span className="home-count">{countLabel(total)}</span>}
        </header>

        {loadingView ? (
          <LoadingState label="Loading tasks…" />
        ) : ordered.length === 0 && !(missing && sidebar.tasksHasMore) ? (
          <div className="home-empty">
            <p>{emptyMessage(sidebar)}</p>
            {searchTerm(sidebar) !== "" && (
              <button className="text-button" type="button" onClick={() => controller.setSearch("")}>
                Clear search
              </button>
            )}
          </div>
        ) : (
          <div id="task-list" className="home-sections" onKeyDown={moveFocus}>
            {sections.map((section) => (
              <section key={section.id} aria-label={section.label ?? title}>
                {section.label && <h2 className="home-section-label">{section.label}</h2>}
                <ul className="home-list">
                  {section.tasks.map((task) => (
                    <TaskRow
                      key={task.id}
                      task={task}
                      meta={rowMeta(task, sidebar, parents)}
                      picked={task.id === picked?.id}
                      showPicked={previewFits}
                      arrived={arrivedTaskIds.has(task.id)}
                      menuOpen={menu?.task.id === task.id}
                      rowRef={(element) => {
                        if (element) rowRefs.current.set(task.id, element);
                        else rowRefs.current.delete(task.id);
                      }}
                      onPick={() => (previewFits ? pick(task.id) : open(task.id))}
                      onOpen={() => open(task.id)}
                      onMenu={(anchor) => setMenu({ task, anchor })}
                    />
                  ))}
                </ul>
              </section>
            ))}
          </div>
        )}

        {sidebar.tasksHasMore && (total === undefined || missing) && !loadingView && (
          <div className="home-more">
            {sidebar.loadMoreFailed && <span role="status">Couldn&apos;t load older tasks.</span>}
            <button className="text-button" type="button" disabled={sidebar.isLoadingMore} onClick={() => void controller.loadMore()}>
              {sidebar.isLoadingMore ? "Loading…" : sidebar.loadMoreFailed ? "Try again" : "Show older tasks"}
            </button>
          </div>
        )}
      </div>

      {previewFits && picked && (
        <TaskPreview
          key={picked.id}
          task={picked}
          worker={workerLabel(picked, sidebar)}
          parents={parents}
          alsoWaiting={alsoWaiting}
          onPick={pick}
          onOpen={() => open(picked.id)}
        />
      )}

      {menu && (
        <TaskMenu
          task={menu.task}
          anchor={menu.anchor}
          offline={offline}
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

function rowMeta(task: SidebarTask, state: SidebarState, parents: ReadonlyMap<string, string>): string {
  const parts = [state.projectFilter === undefined ? projectLabel(projectId(task), parents) : undefined, workerLabel(task, state)];
  if (task.originCwd !== undefined && task.branch !== undefined) parts.push(task.branch);
  // A task nobody stopped is waiting for something; the row says what, so it does not read as stalled.
  if (isExplainedWait(task.hold)) parts.push(waitLabel(task.hold));
  return parts.filter((part) => part !== undefined).join(" · ");
}

function TaskRow({ task, meta, picked, showPicked, arrived, menuOpen, rowRef, onPick, onOpen, onMenu }: {
  task: SidebarTask;
  meta: string;
  picked: boolean;
  /** False while there is no preview, so the pick has nothing to point at. */
  showPicked: boolean;
  arrived: boolean;
  menuOpen: boolean;
  rowRef: (element: HTMLButtonElement | null) => void;
  onPick: () => void;
  onOpen: () => void;
  onMenu: (anchor: HTMLElement) => void;
}) {
  const look = taskStatusLook(task.state);
  const title = displayLabel(task);
  const status = taskStatusLabel(task);
  const viewed = taskOutcomeViews.isViewed(task);
  // The dot and the section already say running, finished, or needs you; a word is kept for what went wrong.
  const tag = look === "problem" ? statusTag(task) : undefined;
  return (
    <li
      className="home-row"
      data-picked={(showPicked && picked) || undefined}
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
        tabIndex={picked ? 0 : -1}
        aria-current={showPicked && picked ? "true" : undefined}
        onClick={onPick}
        onDoubleClick={onOpen}
        onKeyDown={(event) => {
          if (event.key !== "Enter") return;
          event.preventDefault();
          onOpen();
        }}
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
