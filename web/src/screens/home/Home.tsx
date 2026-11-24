// The home screen: recent tasks beside a preview of the one picked, or the
// steps to a first task when there is nothing to show yet.

import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode, type RefObject } from "react";
import type { TaskState } from "@/bridge/types";
import { LoadingState } from "@/components/atoms/ListState";
import { TaskStatusDot, taskStatusLook } from "@/components/atoms/TaskStatusDot";
import { formatCost, taskDuration } from "@/lib/format";
import { handlesClick } from "@/router";
import { taskStatusLabel } from "@/screens/task/format";
import type { SidebarController } from "@/state";
import { searchTerm, type SidebarState, type SidebarTask } from "@/state/sidebar-state";
import { focusTaskSearch } from "@/shell/taskSearch";
import { displayLabel, projectId, projects, taskProjectLabel, type TaskProject } from "@/state/sidebar-projection";
import {
  ArrowUpRightIcon,
  BellIcon,
  CancelIcon,
  CheckCircleIcon,
  CheckIcon,
  ExclamationIcon,
  ExpandIcon,
  FolderIcon,
  HistoryIcon,
  ListIcon,
  PlayIcon,
  QuestionMarkIcon,
  SearchIcon,
  SendIcon,
} from "@/ui/icons";
import { absoluteTime, relativeTime } from "@/ui/time";

/** Enough rows to fill a desktop window; the sidebar lists the rest. */
const HOME_TASK_LIMIT = 12;

/** Below this card width the preview has no room, so a pick opens the task. */
const PREVIEW_MIN_CARD_WIDTH = 640;

/** Below this card width the views column folds into tabs above the list. */
const SIDE_MIN_CARD_WIDTH = 880;

const NEEDS_YOU: ReadonlySet<TaskState> = new Set(["needs_input", "failed", "blocked"]);

const SETTLED: ReadonlySet<TaskState> = new Set(["completed", "failed", "cancelled"]);

type SettingsTab = "workers" | "connections";

export interface HomeProps {
  sidebarController: SidebarController;
  onOpenSettings: (tab: SettingsTab) => void;
  onOpenTask: (id: string) => void;
}

function needsYou(task: SidebarTask): boolean {
  return NEEDS_YOU.has(task.state);
}

/** Tasks that wait on the user lead, then the rest by their last change. */
function recentTasks(tasks: SidebarTask[]): SidebarTask[] {
  return tasks
    .filter((task) => task.archivedAt === undefined)
    .sort((left, right) => {
      const urgency = Number(needsYou(right)) - Number(needsYou(left));
      if (urgency !== 0) return urgency;
      return (Date.parse(right.updatedAt) || 0) - (Date.parse(left.updatedAt) || 0);
    });
}

function narrowsTasks(state: SidebarState): boolean {
  return state.archiveFilter !== "active" || state.projectFilter !== undefined || searchTerm(state) !== "";
}

function countLabel(total: number): string {
  return `${total} ${total === 1 ? "task" : "tasks"}`;
}

function tagLabel(task: SidebarTask): string {
  return task.state === "needs_input" ? "Needs you" : taskStatusLabel(task);
}

function workerLabel(task: SidebarTask, state: SidebarState): string {
  return state.profiles.find((profile) => profile.id === task.profileId)?.label ?? modelLabel(task.model);
}

/** The model without its provider prefix, as the task title bar shows it. */
function modelLabel(model: string): string {
  return model.slice(model.lastIndexOf("/") + 1);
}

function StatusIcon({ task }: { task: SidebarTask }) {
  switch (taskStatusLook(task.state)) {
    case "needs_input":
      return <QuestionMarkIcon size={16} />;
    case "problem":
      return <ExclamationIcon size={16} />;
    case "running":
      return <PlayIcon size={14} />;
    case "waiting":
      return <HistoryIcon size={16} />;
    case "settled":
      return task.state === "cancelled" ? <CancelIcon size={16} /> : <CheckIcon size={16} />;
  }
}

/** What the worker last said, in the words the task already carries. */
function workerWords(task: SidebarTask): string | undefined {
  const look = taskStatusLook(task.state);
  if (look === "needs_input") return task.question;
  if (look === "problem") return task.error ?? task.completion?.reason ?? task.tldr;
  return task.tldr ?? task.completion?.reason;
}

function useCardWidth(): [RefObject<HTMLDivElement | null>, number | undefined] {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState<number>();
  useEffect(() => {
    const element = ref.current;
    if (!element || !("ResizeObserver" in globalThis)) return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry?.contentRect.width));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [ref, width];
}

export function Home({ sidebarController, onOpenSettings, onOpenTask }: HomeProps) {
  const sidebar = useSyncExternalStore(
    (listener) => sidebarController.subscribe(listener),
    () => sidebarController.snapshot,
  );
  const hasTasks = sidebar.tasks.length > 0;

  if (sidebar.loadState === "loading" && !hasTasks) {
    return (
      <div id="page-title" className="home-intro">
        <LoadingState label="Loading your workspace…" />
      </div>
    );
  }

  if (sidebar.loadState === "error" && !hasTasks) {
    return (
      <div className="home-intro">
        <h1 id="page-title" className="page-title">Your workspace</h1>
        <p className="app-description">Couldn&apos;t load your workspace.</p>
        <button className="text-button" type="button" onClick={() => sidebarController.refresh()}>
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

  if (!hasTasks && !narrowsTasks(sidebar)) return <FirstRun onOpenSettings={onOpenSettings} />;

  return <HomeBoard sidebar={sidebar} onOpenTask={onOpenTask} />;
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

type StatusView = "recent" | "needs-you" | "in-progress" | "finished";

type HomeView = { kind: "status"; id: StatusView } | { kind: "project"; id: string; name: string };

interface StatusViewOption {
  id: StatusView;
  label: string;
  title: string;
  description: string;
  icon: ReactNode;
  matches: (task: SidebarTask) => boolean;
}

const STATUS_VIEWS: StatusViewOption[] = [
  { id: "recent", label: "Recent", title: "Recent tasks", description: "Newest first", icon: <ListIcon />, matches: () => true },
  { id: "needs-you", label: "Needs you", title: "Needs you", description: "Questions, failures, and blocked tasks", icon: <BellIcon />, matches: needsYou },
  { id: "in-progress", label: "In progress", title: "In progress", description: "Running now or waiting to start", icon: <PlayIcon />, matches: inProgress },
  { id: "finished", label: "Finished", title: "Finished", description: "Completed or cancelled", icon: <CheckCircleIcon />, matches: finished },
];

function inProgress(task: SidebarTask): boolean {
  const look = taskStatusLook(task.state);
  return look === "running" || look === "waiting";
}

function finished(task: SidebarTask): boolean {
  return task.state === "completed" || task.state === "cancelled";
}

function statusView(id: StatusView) {
  return STATUS_VIEWS.find((view) => view.id === id) ?? STATUS_VIEWS[0];
}

function viewMatches(view: HomeView, task: SidebarTask): boolean {
  return view.kind === "project" ? projectId(task) === view.id : statusView(view.id).matches(task);
}

interface ViewHeading {
  title: string;
  description: string;
}

function viewHeading(view: HomeView, sidebar: SidebarState, waiting: number): ViewHeading {
  if (view.kind === "project") return { title: view.name, description: "Tasks in this project" };
  const status = statusView(view.id);
  if (narrowsTasks(sidebar)) return { title: status.title, description: "Filtered by the sidebar" };
  if (view.id === "recent" && waiting > 0) return { title: status.title, description: "Tasks that need you come first" };
  return { title: status.title, description: status.description };
}

function HomeBoard({ sidebar, onOpenTask }: { sidebar: SidebarState; onOpenTask: (id: string) => void }) {
  const tasks = useMemo(() => recentTasks(sidebar.tasks), [sidebar.tasks]);
  const taskProjects = useMemo(() => projects(tasks), [tasks]);
  const waiting = tasks.filter(needsYou);
  const [view, setView] = useState<HomeView>({ kind: "status", id: "recent" });
  const viewTasks = tasks.filter((task) => viewMatches(view, task));
  const shown = viewTasks.slice(0, HOME_TASK_LIMIT);
  const [pickedId, setPickedId] = useState<string>();
  const picked = viewTasks.find((task) => task.id === pickedId) ?? tasks.find((task) => task.id === pickedId) ?? shown[0];
  const [cardRef, cardWidth] = useCardWidth();
  const previewFits = cardWidth === undefined || cardWidth >= PREVIEW_MIN_CARD_WIDTH;
  const sideFits = cardWidth === undefined || cardWidth >= SIDE_MIN_CARD_WIDTH;
  const heading = viewHeading(view, sidebar, waiting.length);

  const show = (next: HomeView) => {
    setView(next);
    setPickedId(undefined);
  };
  const pick = (id: string) => {
    if (previewFits) setPickedId(id);
    else onOpenTask(id);
  };

  return (
    <div
      ref={cardRef}
      className="home-card"
      data-side={sideFits}
      data-preview={previewFits && picked !== undefined}
    >
      {sideFits && (
        <HomeSide
          view={view}
          workers={sidebar.profiles.length}
          waiting={waiting.length}
          projects={taskProjects}
          onShow={show}
        />
      )}
      <div className="home-tasks">
        <header className="home-head">
          <div>
            <h1 id="page-title" className="home-title">{heading.title}</h1>
            <p className="home-subtitle">{heading.description}</p>
          </div>
          <span className="home-count">{countLabel(viewTasks.length)}</span>
        </header>
        {!sideFits && (
          <div className="home-tabs" role="group" aria-label="Task views">
            {STATUS_VIEWS.map((status) => (
              <button
                key={status.id}
                type="button"
                aria-pressed={view.kind === "status" && view.id === status.id}
                onClick={() => show({ kind: "status", id: status.id })}
              >
                {status.label}
              </button>
            ))}
          </div>
        )}
        {shown.length === 0 ? (
          <p className="home-hint">{emptyHint(view, sidebar)}</p>
        ) : (
          <>
            <ul className="home-list" aria-labelledby="page-title">
              {shown.map((task) => (
                <li key={task.id}>
                  <HomeTaskCard
                    task={task}
                    worker={workerLabel(task, sidebar)}
                    picked={previewFits && task.id === picked?.id}
                    offerOpen={previewFits}
                    onPick={() => pick(task.id)}
                    onOpen={() => onOpenTask(task.id)}
                  />
                </li>
              ))}
            </ul>
            <p className="home-hint">
              {previewFits ? "Pick a task to see where it stands on the right." : "Pick a task to open it."}
              {viewTasks.length > shown.length && ` The sidebar lists all ${viewTasks.length}.`}
            </p>
          </>
        )}
      </div>
      {previewFits && picked && (
        <TaskPreview
          key={picked.id}
          task={picked}
          worker={workerLabel(picked, sidebar)}
          alsoWaiting={waiting.filter((task) => task.id !== picked.id).slice(0, 3)}
          onPick={setPickedId}
          onOpen={() => onOpenTask(picked.id)}
        />
      )}
    </div>
  );
}

function emptyHint(view: HomeView, sidebar: SidebarState): string {
  if (narrowsTasks(sidebar)) return "No tasks match your sidebar search or filters. Clear them to see every task.";
  if (view.kind === "status" && view.id === "needs-you") return "Nothing needs you right now.";
  return "No tasks here yet.";
}

function HomeSide({ view, workers, waiting, projects, onShow }: {
  view: HomeView;
  workers: number;
  waiting: number;
  projects: TaskProject[];
  onShow: (view: HomeView) => void;
}) {
  return (
    <nav className="home-side" aria-label="Task views">
      <div className="home-workspace">
        <span className="home-avatar" aria-hidden="true">O</span>
        <span>
          <strong>Oga</strong>
          <small>{workers} {workers === 1 ? "worker" : "workers"}</small>
        </span>
      </div>
      <button type="button" className="home-search" onClick={focusTaskSearch}>
        <SearchIcon size={14} />
        <span>Search tasks</span>
        <kbd>⌘K</kbd>
      </button>
      <span className="home-side-label">Tasks</span>
      <ul>
        {STATUS_VIEWS.map((status) => (
          <li key={status.id}>
            <button
              type="button"
              aria-current={view.kind === "status" && view.id === status.id ? "page" : undefined}
              onClick={() => onShow({ kind: "status", id: status.id })}
            >
              {status.icon}
              <span>{status.label}</span>
              {status.id === "needs-you" && waiting > 0 && <em className="home-badge">{waiting}</em>}
            </button>
          </li>
        ))}
      </ul>
      {projects.length > 1 && (
        <>
          <span className="home-side-label">Projects</span>
          <ul>
            {projects.map((project) => (
              <li key={project.id}>
                <button
                  type="button"
                  title={project.id}
                  aria-current={view.kind === "project" && view.id === project.id ? "page" : undefined}
                  onClick={() => onShow({ kind: "project", id: project.id, name: project.name })}
                >
                  <FolderIcon />
                  <span>{project.name}</span>
                  <small>{project.count}</small>
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
    </nav>
  );
}

function HomeTaskCard({ task, worker, picked, offerOpen, onPick, onOpen }: {
  task: SidebarTask;
  worker: string;
  picked: boolean;
  offerOpen: boolean;
  onPick: () => void;
  onOpen: () => void;
}) {
  const look = taskStatusLook(task.state);
  const title = displayLabel(task);
  return (
    <div className="home-task" data-picked={picked}>
      <button type="button" className="home-task-pick" aria-pressed={picked} onClick={onPick} onDoubleClick={onOpen}>
        <span className="home-task-icon" data-look={look} aria-hidden="true">
          <StatusIcon task={task} />
        </span>
        <span className="home-task-text">
          <strong>{title}</strong>
          <small title={absoluteTime(task.updatedAt)}>
            {worker} · {taskProjectLabel(task)} · {relativeTime(task.updatedAt)}
          </small>
        </span>
        <span className="home-tag" data-look={look}>{tagLabel(task)}</span>
      </button>
      {offerOpen && (
        <button type="button" className="home-task-open" aria-label={`Open ${title}`} onClick={onOpen}>
          Open <ArrowUpRightIcon size={12} />
        </button>
      )}
    </div>
  );
}

function TaskPreview({ task, worker, alsoWaiting, onPick, onOpen }: {
  task: SidebarTask;
  worker: string;
  alsoWaiting: SidebarTask[];
  onPick: (id: string) => void;
  onOpen: () => void;
}) {
  const look = taskStatusLook(task.state);
  const words = workerWords(task);
  const facts: Array<{ label: string; value: string; tone?: "danger" }> = [
    { label: "Status", value: tagLabel(task), tone: look === "problem" ? "danger" : undefined },
    { label: "Worker", value: worker },
    { label: "Project", value: taskProjectLabel(task) },
  ];
  const time = taskDuration(task.durationMs, task.runningSince, !SETTLED.has(task.state));
  if (time) facts.push({ label: "Time", value: time });
  const cost = formatCost(task.costUsd, task.costUsdEstimated);
  if (cost) facts.push({ label: "Cost", value: cost });
  const canWrite = task.state !== "preparing_checkout" && task.state !== "removing_checkout";

  return (
    <aside className="home-preview" aria-label="Task preview">
      <header className="home-preview-head">
        <TaskStatusDot state={task.state} label={tagLabel(task)} />
        <strong title={displayLabel(task)}>{displayLabel(task)}</strong>
        <button className="icon-button" type="button" aria-label="Open task" title="Open task" onClick={onOpen}>
          <ExpandIcon />
        </button>
      </header>
      <div className="home-thread">
        <p className="home-bubble">{task.promptPreview}</p>
        {words ? <p className="home-reply-text">{words}</p> : <p className="home-reply-text home-reply-quiet">No update from the worker yet.</p>}
        <dl className="home-facts">
          {facts.map((fact) => (
            <div key={fact.label}>
              <dt>{fact.label}</dt>
              <dd data-tone={fact.tone}>{fact.value}</dd>
            </div>
          ))}
        </dl>
      </div>
      {alsoWaiting.length > 0 && (
        <div className="home-chips" role="group" aria-labelledby="home-chips-label">
          <span id="home-chips-label">Also needs you</span>
          {alsoWaiting.map((other) => (
            <button key={other.id} type="button" title={displayLabel(other)} onClick={() => onPick(other.id)}>
              {displayLabel(other)}
            </button>
          ))}
        </div>
      )}
      {canWrite && (
        <button className="home-compose" type="button" onClick={onOpen}>
          <span>{task.state === "needs_input" ? "Answer in the task" : "Follow up in the task"}</span>
          <span className="home-compose-send" aria-hidden="true">
            <SendIcon size={14} />
          </span>
        </button>
      )}
    </aside>
  );
}
