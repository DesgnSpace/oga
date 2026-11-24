// How tasks and projects are named in lists.

import type { TaskSummary } from "@/bridge/types";

export interface TaskProject {
  /** The project's folder, as the broker groups tasks by it. */
  id: string;
  name: string;
  /** The folders above the project, set only when another project shares its name. */
  parent?: string;
  count: number;
}

export function projectName(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  const parts = trimmed.split("/");
  for (let index = parts.length - 1; index >= 0; index -= 1) {
    if (parts[index] !== "") return parts[index];
  }
  return path;
}

export function projectId(task: Pick<TaskSummary, "cwd" | "originCwd">): string {
  return task.originCwd ?? task.cwd;
}

function parentFolders(path: string): string[] {
  return path.split("/").filter((part) => part !== "").slice(0, -1);
}

/** For each project sharing its folder name, the fewest parent folders that set it apart. */
export function projectParents(ids: readonly string[]): Map<string, string> {
  const byName = new Map<string, string[]>();
  for (const id of new Set(ids)) {
    const name = projectName(id);
    byName.set(name, [...(byName.get(name) ?? []), id]);
  }
  const parents = new Map<string, string>();
  for (const group of byName.values()) {
    if (group.length < 2) continue;
    const folders = group.map(parentFolders);
    folders.forEach((own, index) => {
      const others = folders.filter((_, other) => other !== index);
      let depth = 1;
      const label = () => own.slice(-depth).join("/");
      while (depth < own.length && others.some((parts) => parts.slice(-depth).join("/") === label())) depth += 1;
      parents.set(group[index], label());
    });
  }
  return parents;
}

export function projectLabel(id: string, parents: ReadonlyMap<string, string>): string {
  const parent = parents.get(id);
  return parent ? `${parent}/${projectName(id)}` : projectName(id);
}

/** Projects in the order newest-first `tasks` meet them. */
export function recentProjectIds(tasks: readonly TaskSummary[]): string[] {
  return [...new Set(tasks.map(projectId))];
}

/** Projects with recent work first, in `recent` order, then the rest by name. */
export function projectList(recent: readonly string[], totals: Readonly<Record<string, number>>): TaskProject[] {
  const known = new Set(recent);
  const rest = Object.keys(totals)
    .filter((id) => !known.has(id))
    .sort((left, right) => projectName(left).localeCompare(projectName(right)));
  const ids = [...recent, ...rest];
  const parents = projectParents(ids);
  return ids.map((id) => ({ id, name: projectName(id), parent: parents.get(id), count: totals[id] ?? 0 }));
}

/** Narrows the list to tasks whose title contains `term`, ignoring case. */
export function matching<T extends TaskSummary>(tasks: readonly T[], term: string): T[] {
  if (term === "") return tasks.slice();
  const needle = term.toLowerCase();
  return tasks.filter((task) => displayLabel(task).toLowerCase().includes(needle));
}

export function displayLabel(task: Pick<TaskSummary, "title" | "promptPreview">): string {
  const title = task.title?.trim();
  if (title) return title;
  return task.promptPreview.split("\n")[0] || "Untitled task";
}
