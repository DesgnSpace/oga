// Short names for toast titles, so a toast names what it acts on.
// Long titles are cut to one line with an ellipsis, keeping toasts narrow.

/** Longest subject shown in a toast title before truncation. */
export const TOAST_SUBJECT_LIMIT = 48;

/** One line, trimmed, capped at TOAST_SUBJECT_LIMIT characters. */
export function truncateSubject(value: string, limit: number = TOAST_SUBJECT_LIMIT): string {
  const line = value.split("\n")[0]?.replace(/\s+/g, " ").trim() ?? "";
  if (Array.from(line).length <= limit) return line;
  return `${Array.from(line).slice(0, limit - 1).join("")}…`;
}

/**
 * The task name for a toast title, or undefined when the task has none.
 * An untitled task falls back to the generic wording at the call site.
 */
export function taskToastName(task: {
  title?: string;
  prompt?: string;
  promptPreview?: string;
}): string | undefined {
  const title = task.title?.trim();
  if (title) return truncateSubject(title);
  const preview = (task.promptPreview ?? task.prompt ?? "").split("\n")[0]?.trim() ?? "";
  if (!preview) return undefined;
  return truncateSubject(preview);
}

/** A worker name for a toast title, or undefined when there is none. */
export function workerToastName(label: string | undefined): string | undefined {
  const name = label?.trim();
  if (!name) return undefined;
  return truncateSubject(name);
}
