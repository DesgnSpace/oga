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
import { displayLabel, taskProjectLabel } from "@/state/sidebar-projection";
import {
  CancelIcon,
  CheckIcon,
  ExclamationIcon,
  ExpandIcon,
  HistoryIcon,
  PlayIcon,
  QuestionMarkIcon,
  SendIcon,
} from "@/ui/icons";
import { absoluteTime, relativeTime } from "@/ui/time";

/** Enough rows to fill a desktop window; the sidebar lists the rest. */
const HOME_TASK_LIMIT = 12;

/** Below this card width the preview has no room, so a pick opens the task. */
const PREVIEW_MIN_CARD_WIDTH = 640;

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

function subtitle(state: SidebarState, waiting: number): string {
  if (narrowsTasks(state)) return "Filtered by the sidebar";
  return waiting > 0 ? "Tasks that need you come first" : "Newest first";
}

function countLabel(total: number, waiting: number): string {
  if (waiting > 0) return `${waiting} ${waiting === 1 ? "needs" : "need"} you`;
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

function HomeBoard({ sidebar, onOpenTask }: { sidebar: SidebarState; onOpenTask: (id: string) => void }) {
  const tasks = useMemo(() => recentTasks(sidebar.tasks), [sidebar.tasks]);
  const shown = tasks.slice(0, HOME_TASK_LIMIT);
  const waiting = tasks.filter(needsYou);
  const [pickedId, setPickedId] = useState<string>();
  const picked = tasks.find((task) => task.id === pickedId) ?? shown[0];
  const [cardRef, cardWidth] = useCardWidth();
  const previewFits = cardWidth === undefined || cardWidth >= PREVIEW_MIN_CARD_WIDTH;

  const pick = (id: string) => {
    if (previewFits) setPickedId(id);
    else onOpenTask(id);
  };

  return (
    <div ref={cardRef} className="home-card" data-preview={previewFits && picked !== undefined}>
      <div className="home-tasks">
        <header className="home-head">
          <div>
            <h1 id="page-title" className="home-title">Recent tasks</h1>
            <p className="home-subtitle">{subtitle(sidebar, waiting.length)}</p>
          </div>
          <span className="home-count">{countLabel(tasks.length, waiting.length)}</span>
        </header>
        {shown.length === 0 ? (
          <p className="home-hint">No tasks match your sidebar search or filters. Clear them to see every task.</p>
        ) : (
          <>
            <ul className="home-list" aria-labelledby="page-title">
              {shown.map((task) => (
                <li key={task.id}>
                  <HomeTaskCard
                    task={task}
                    worker={workerLabel(task, sidebar)}
                    picked={previewFits && task.id === picked?.id}
                    onPick={() => pick(task.id)}
                    onOpen={() => onOpenTask(task.id)}
                  />
                </li>
              ))}
            </ul>
            <p className="home-hint">
              {previewFits ? "Pick a task to see where it stands. Double-click to open it." : "Pick a task to open it."}
              {tasks.length > shown.length && ` The sidebar lists all ${tasks.length}.`}
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

function HomeTaskCard({ task, worker, picked, onPick, onOpen }: {
  task: SidebarTask;
  worker: string;
  picked: boolean;
  onPick: () => void;
  onOpen: () => void;
}) {
  const look = taskStatusLook(task.state);
  return (
    <button
      type="button"
      className="home-task"
      aria-pressed={picked}
      onClick={onPick}
      onDoubleClick={onOpen}
    >
      <span className="home-task-icon" data-look={look} aria-hidden="true">
        <StatusIcon task={task} />
      </span>
      <span className="home-task-text">
        <strong>{displayLabel(task)}</strong>
        <small title={absoluteTime(task.updatedAt)}>
          {worker} · {taskProjectLabel(task)} · {relativeTime(task.updatedAt)}
        </small>
      </span>
      <span className="home-tag" data-look={look}>{tagLabel(task)}</span>
    </button>
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
