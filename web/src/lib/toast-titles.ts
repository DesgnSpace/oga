// Toast titles that name what they act on, kept short enough for one line.
// The toast card already ellipsizes its title; this truncates long names
// first so a pasted paragraph cannot widen the copy before CSS clamps it.

const MAX_TOAST_NAME_LENGTH = 60;

export function truncateToastName(name: string, max: number = MAX_TOAST_NAME_LENGTH): string {
  const firstLine = (name.split("\n")[0] ?? "").replace(/\s+/g, " ").trim();
  const characters = Array.from(firstLine);
  if (characters.length <= max) return firstLine;
  return `${characters.slice(0, max - 1).join("")}…`;
}

export interface ToastSubject {
  title?: string;
  tldr?: string;
  prompt?: string;
  promptPreview?: string;
}

/** Best-effort display name for a task-like value, or undefined when unnamed. */
export function toastSubjectName(source: ToastSubject): string | undefined {
  const title = source.title?.trim();
  if (title) return truncateToastName(title);
  const tldr = source.tldr?.trim();
  if (tldr) return truncateToastName(tldr);
  const prompt = source.prompt ?? source.promptPreview ?? "";
  const firstLine = prompt
    .split("\n")
    .map((line) => line.trim())
    .find((line) => line !== "");
  if (firstLine) return truncateToastName(firstLine);
  return undefined;
}

function quoted(name: string | undefined): string | undefined {
  return name ? `"${name}"` : undefined;
}

export type TaskToastAction =
  | "archive"
  | "restore"
  | "stop"
  | "resume"
  | "complete"
  | "move"
  | "reply"
  | "steer"
  | "queue"
  | "remove-follow-up";

export interface TaskToastTitles {
  pending: string;
  success: string;
  failure: string;
}

const TASK_TOAST_COPY: Record<TaskToastAction, { pending: string; success: string; failure: string; generic: string }> = {
  archive: { pending: "Archiving", success: "Archived", failure: "Couldn't archive", generic: "task" },
  restore: { pending: "Restoring", success: "Restored", failure: "Couldn't restore", generic: "task" },
  stop: { pending: "Stopping", success: "Stopped", failure: "Couldn't stop", generic: "task" },
  resume: { pending: "Resuming", success: "Resumed", failure: "Couldn't resume", generic: "task" },
  complete: { pending: "Completing", success: "Marked", failure: "Couldn't complete", generic: "task" },
  move: { pending: "Moving", success: "Moved", failure: "Couldn't move", generic: "task" },
  reply: { pending: "Sending reply to", success: "Reply sent to", failure: "Couldn't send reply to", generic: "task" },
  steer: { pending: "Sending instruction to", success: "Instruction sent to", failure: "Couldn't send instruction to", generic: "task" },
  queue: { pending: "Queueing follow-up for", success: "Follow-up queued for", failure: "Couldn't queue follow-up for", generic: "task" },
  "remove-follow-up": {
    pending: "Removing follow-up from",
    success: "Follow-up removed from",
    failure: "Couldn't remove follow-up from",
    generic: "task",
  },
};

/** Pending/success/failure titles for a task action, naming the task when known. */
export function taskToastTitles(task: ToastSubject, action: TaskToastAction): TaskToastTitles {
  const copy = TASK_TOAST_COPY[action];
  const name = quoted(toastSubjectName(task));
  if (action === "complete") {
    return {
      pending: name ? `Completing ${name}` : `Completing ${copy.generic}`,
      success: name ? `Marked ${name} completed` : "Task marked completed",
      failure: name ? `Couldn't complete ${name}` : "Couldn't complete task",
    };
  }
  if (action === "move") {
    return {
      pending: name ? `Moving ${name}` : "Moving task",
      success: name ? `Moved ${name} to another worker` : "Task moved to another worker",
      failure: name ? `Couldn't move ${name}` : "Couldn't move task",
    };
  }
  return {
    pending: name ? `${copy.pending} ${name}` : `${copy.pending} ${copy.generic}`,
    success: name ? `${copy.success} ${name}` : `Task ${copy.success.toLowerCase()}`,
    failure: name ? `${copy.failure} ${name}` : `${copy.failure} ${copy.generic}`,
  };
}

export type WorkerToastAction = "remove" | "add" | "save" | "update" | "models";

export interface WorkerToastTitles {
  pending: string;
  failure: string;
}

/** Pending/failure titles for a worker action, naming the worker when known. */
export function workerToastTitles(label: string | undefined, action: WorkerToastAction): WorkerToastTitles {
  const clean = label?.trim() ? truncateToastName(label.trim()) : undefined;
  const name = clean ? `"${clean}"` : undefined;
  switch (action) {
    case "remove":
      return { pending: name ? `Removing worker ${name}` : "Removing worker", failure: name ? `Couldn't remove worker ${name}` : "Couldn't remove worker" };
    case "add":
      return { pending: name ? `Adding worker ${name}` : "Adding worker", failure: name ? `Couldn't add worker ${name}` : "Couldn't add worker" };
    case "save":
      return { pending: name ? `Saving worker ${name}` : "Saving worker", failure: name ? `Couldn't save worker ${name}` : "Couldn't save worker" };
    case "update":
      return { pending: "Updating worker", failure: name ? `Couldn't update worker ${name}` : "Couldn't update worker" };
    case "models":
      return {
        pending: "Updating models",
        failure: name ? `Couldn't update models for ${name}` : "Couldn't update model settings",
      };
  }
}
