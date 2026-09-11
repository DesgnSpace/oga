import { useEffect, useRef, useState } from "react";
import type { Task, TaskEventView } from "@/bridge/types";
import { copyText, shortId } from "@/lib/identifiers";
import { toast } from "@/state/toast";
import { CheckIcon, CopyIcon } from "@/ui/icons";
import { contextUsage } from "./contextUsage";

const COPIED_FLASH_MS = 1_500;

function CopyableDetail({
  label,
  value,
  shorten = false,
  displayValue,
  showLabel = true,
  title,
}: {
  label: string;
  value: string;
  shorten?: boolean;
  displayValue?: string;
  showLabel?: boolean;
  title?: string;
}) {
  const displayedValue = displayValue ?? (shorten ? shortId(value) : value);
  const [copied, setCopied] = useState(false);
  const flashTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(flashTimer.current), []);

  const copy = async () => {
    try {
      await copyText(value);
      setCopied(true);
      clearTimeout(flashTimer.current);
      flashTimer.current = setTimeout(() => setCopied(false), COPIED_FLASH_MS);
    } catch {
      toast.error(`Couldn't copy ${label.toLowerCase()}`);
    }
  };

  return (
    <button
      className={showLabel ? "task-metadata-item" : "task-metadata-item task-metadata-chip"}
      type="button"
      aria-label={copied ? `Copied ${label.toLowerCase()}: ${value}` : `Copy ${label.toLowerCase()}: ${value}`}
      title={title ?? `Copy ${label.toLowerCase()}`}
      onClick={() => void copy()}
    >
      {showLabel && <span className="task-metadata-label">{label}</span>}
      <span className="task-metadata-value">{displayedValue}</span>
      <span className="task-metadata-copy" aria-hidden="true">
        {copied ? <CheckIcon size={11} /> : <CopyIcon size={11} />}
      </span>
    </button>
  );
}

export function TaskMetadata({
  task,
  events,
  contextWindow,
}: {
  task: Task;
  events: TaskEventView[];
  contextWindow: number | undefined;
}) {
  const branch = task.worktree?.branch ?? task.branch;
  const usage = contextUsage(events, contextWindow);

  return (
    <div className="task-metadata" aria-label="Task details">
      <CopyableDetail label="Task" value={task.id} shorten />
      {task.sessionId && <CopyableDetail label="Session" value={task.sessionId} shorten />}
      {branch && <CopyableDetail label="Branch" value={branch} />}
      {task.worktree ? (
        <CopyableDetail
          label="Worktree"
          value={task.worktree.path}
          displayValue={task.worktreeLabel}
          showLabel={false}
          title={task.worktree.path}
        />
      ) : (
        <span className="task-metadata-static">
          <span className="task-metadata-label">Tree</span>
          <span className="task-metadata-value">Main tree</span>
        </span>
      )}
      <CopyableDetail label="Project" value={task.worktree?.originCwd ?? task.cwd} />
      {usage && <ContextDetail usage={usage} />}
    </div>
  );
}

/**
 * How full the worker's context is, in the strip's own quiet register: a fill
 * against the window plus how far it has grown, never a raw number alone.
 * Nothing renders when the run has no usable read or the model names no
 * window — no zero, no guess.
 */
function ContextDetail({
  usage,
}: {
  usage: NonNullable<ReturnType<typeof contextUsage>>;
}) {
  const text =
    usage.deltaPercent > 0
      ? `${usage.displayPercent}% · +${usage.deltaPercent}%`
      : `${usage.displayPercent}%`;
  const since = usage.compacted ? "the last compaction" : "this run started";
  const title =
    usage.delta > 0
      ? `About ${usage.used.toLocaleString()} of ${usage.window.toLocaleString()} tokens used · up ${usage.deltaPercent}% since ${since}`
      : `About ${usage.used.toLocaleString()} of ${usage.window.toLocaleString()} tokens used`;
  return (
    <span className="task-metadata-static" title={title}>
      <span className="task-metadata-label">Context</span>
      <span className="task-metadata-value">{text}</span>
    </span>
  );
}
