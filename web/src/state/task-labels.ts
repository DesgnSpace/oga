// Project labels that stay apart when two projects share a folder name.

import { projectName } from "./sidebar-projection";

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
