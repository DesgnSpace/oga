// A directory tree over a changes panel's files, walked by the tree view
// instead of re-derived from events. Pure and source-agnostic: the same shape
// serves the reported and git views, flat or grouped by turn.

import type { ChangedFileView } from ".";
import {
  GENERATED_GROUP_KEY,
  GENERATED_GROUP_NAME,
  largestFirst,
  pathBaseName,
  type ChangeSort,
} from "./ordering";

export interface TreeFileNode {
  kind: "file";
  name: string;
  path: string;
  file: ChangedFileView;
}

export interface TreeDirNode {
  kind: "dir";
  name: string;
  path: string;
  children: TreeNode[];
}

export type TreeNode = TreeDirNode | TreeFileNode;

/**
 * Builds a directory tree from a changes panel's file lists. Generated files
 * gather under one group at the bottom rather than in the folders they were
 * built into.
 */
export function buildFileTree(
  files: readonly ChangedFileView[],
  generated: readonly ChangedFileView[],
  sort: ChangeSort,
): TreeNode[] {
  const root: TreeDirNode = { kind: "dir", name: "", path: "", children: [] };
  for (const file of files) {
    const segments = file.path.split("/").filter((segment) => segment.length > 0);
    let dir = root;
    let dirPath = "";
    for (const segment of segments.slice(0, -1)) {
      dirPath = dirPath === "" ? segment : `${dirPath}/${segment}`;
      const existing = dir.children.find((child): child is TreeDirNode => child.kind === "dir" && child.name === segment);
      if (existing) {
        dir = existing;
      } else {
        const created: TreeDirNode = { kind: "dir", name: segment, path: dirPath, children: [] };
        dir.children.push(created);
        dir = created;
      }
    }
    dir.children.push(fileNode(pathBaseName(file.path), file));
  }
  sortTree(root.children, sort);

  if (generated.length > 0) {
    const group: TreeDirNode = {
      kind: "dir",
      name: GENERATED_GROUP_NAME,
      path: GENERATED_GROUP_KEY,
      children: generated.map((file) => fileNode(pathBaseName(file.path), file)),
    };
    sortTree(group.children, sort);
    root.children.push(group);
  }

  return root.children;
}

function fileNode(name: string, file: ChangedFileView): TreeFileNode {
  return { kind: "file", name, path: file.path, file };
}

function sortTree(nodes: TreeNode[], sort: ChangeSort): void {
  nodes.sort((a, b) => {
    if (a.kind === "dir" && b.kind === "dir") return a.name.localeCompare(b.name);
    if (a.kind === "file" && b.kind === "file") return compareFiles(a, b, sort);
    return a.kind === "dir" ? -1 : 1;
  });
  for (const node of nodes) {
    if (node.kind === "dir") sortTree(node.children, sort);
  }
}

/**
 * Files in a folder rank by name under folder order and by size under size
 * order, with the name settling a tie. Directories keep their alphabetical order
 * either way: a folder is a place, not a change.
 */
function compareFiles(a: TreeFileNode, b: TreeFileNode, sort: ChangeSort): number {
  if (sort === "size") return largestFirst(a.file, b.file) || a.name.localeCompare(b.name);
  return a.name.localeCompare(b.name);
}
