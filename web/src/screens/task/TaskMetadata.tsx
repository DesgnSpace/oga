import { useEffect, useRef, useState } from "react";
import type { Task } from "@/bridge/types";
import { copyText, shortId } from "@/lib/identifiers";
import { toast } from "@/state/toast";
import { CheckIcon, CopyIcon } from "@/ui/icons";

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

export function TaskMetadata({ task }: { task: Task }) {
  const branch = task.worktree?.branch ?? task.branch;

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
        <CopyableDetail label="Tree" value="Main tree" />
      )}
      <CopyableDetail label="Project" value={task.worktree?.originCwd ?? task.cwd} />
    </div>
  );
}
