import { useEffect, useState } from "react";
import type { TaskState } from "@/bridge/types";
import { TaskStatusDot, taskStatusLook } from "@/components/atoms/TaskStatusDot";
import { InlineMarkdown } from "@/domain/markdown";
import { formatCost, taskDuration } from "@/lib/format";
import { taskStatusLabel } from "@/screens/task/format";
import { displayLabel, projectId, projectLabel } from "@/state/task-labels";
import type { SidebarTask } from "@/state/sidebar-state";
import { ArrowUpRightIcon } from "@/ui/icons";

const SETTLED: ReadonlySet<TaskState> = new Set(["completed", "failed", "cancelled"]);

/** Past this many characters a brief is cut, so its summary says so. */
const BRIEF_PREVIEW_LIMIT = 200;

export function statusTag(task: SidebarTask): string {
  return task.state === "needs_input" ? "Needs you" : taskStatusLabel(task);
}

/** Drops a code span the cut left open, so no stray backtick shows. */
function briefSummary(brief: string): string {
  const paragraph = brief.trim().split(/\n\s*\n/)[0] ?? "";
  let line = paragraph.replace(/\s*\n\s*/g, " ").trim();
  const cut = line.length >= BRIEF_PREVIEW_LIMIT || paragraph.length < brief.trim().length;
  if ((line.match(/`/g)?.length ?? 0) % 2 === 1) line = line.slice(0, line.lastIndexOf("`")).trimEnd();
  return cut && !line.endsWith("…") ? `${line}…` : line;
}

function workerNote(task: SidebarTask): string | undefined {
  const look = taskStatusLook(task.state);
  if (look === "needs_input") return task.question;
  if (look === "problem") return task.error ?? task.completion?.reason;
  return undefined;
}

function useSecondTick(active: boolean): void {
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setTick((tick) => tick + 1), 1_000);
    return () => clearInterval(timer);
  }, [active]);
}

export function TaskPreview({ task, worker, parents, alsoWaiting, onPick, onOpen }: {
  task: SidebarTask;
  worker: string;
  parents: ReadonlyMap<string, string>;
  alsoWaiting: SidebarTask[];
  onPick: (id: string) => void;
  onOpen: () => void;
}) {
  const look = taskStatusLook(task.state);
  const running = task.state === "running";
  useSecondTick(running);
  const note = workerNote(task);
  const asked = task.tldr?.trim() || briefSummary(task.promptPreview);
  const facts: Array<{ label: string; value: string; title?: string; tone?: "danger" }> = [
    { label: "Status", value: statusTag(task), tone: look === "problem" ? "danger" : undefined },
    { label: "Worker", value: worker },
    { label: "Project", value: projectLabel(projectId(task), parents), title: projectId(task) },
  ];
  if (task.originCwd !== undefined && task.branch !== undefined) facts.push({ label: "Branch", value: task.branch });
  const time = taskDuration(task.durationMs, task.runningSince, !SETTLED.has(task.state));
  if (time) facts.push({ label: "Time", value: time });
  const cost = formatCost(task.costUsd, task.costUsdEstimated);
  if (cost) facts.push({ label: "Cost", value: cost });

  return (
    <aside className="home-preview" aria-label="Task preview">
      <header className="home-preview-head">
        <TaskStatusDot state={task.state} label={statusTag(task)} />
        <h2 title={displayLabel(task)}><InlineMarkdown source={displayLabel(task)} /></h2>
      </header>
      <div className="home-preview-body">
        {asked && (
          <p className="home-asked">
            <InlineMarkdown source={asked} />
          </p>
        )}
        {note && (
          <p className="home-note" data-look={look}>
            <InlineMarkdown source={note} />
          </p>
        )}
        <dl className="home-facts">
          {facts.map((fact) => (
            <div key={fact.label}>
              <dt>{fact.label}</dt>
              <dd data-tone={fact.tone} title={fact.title ?? fact.value}>{fact.value}</dd>
            </div>
          ))}
        </dl>
        {alsoWaiting.length > 0 && (
          <div className="home-chips" role="group" aria-labelledby="home-chips-label">
            <span id="home-chips-label">Also needs you</span>
            {alsoWaiting.map((other) => (
              <button key={other.id} type="button" title={displayLabel(other)} onClick={() => onPick(other.id)}>
                <InlineMarkdown source={displayLabel(other)} />
              </button>
            ))}
          </div>
        )}
      </div>
      <footer className="home-preview-foot">
        <button className="home-open" type="button" data-primary={task.state === "needs_input" || undefined} onClick={onOpen}>
          {task.state === "needs_input" ? "Open task to answer" : "Open task"}
          <ArrowUpRightIcon size={12} />
        </button>
      </footer>
    </aside>
  );
}
