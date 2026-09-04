// A directory tree over a changes panel's flat file list, walked by the tree
// view instead of re-derived from events. Pure and source-agnostic: the same
// shape serves the reported and git views, flat or grouped by turn.

import type { ChangedFileView } from ".";

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

/** Builds a directory tree from a changes panel's flat file list, sorted with directories first. */
export function buildFileTree(files: ChangedFileView[]): TreeNode[] {
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
    const name = segments[segments.length - 1] ?? file.path;
    dir.children.push({ kind: "file", name, path: file.path, file });
  }
  sortTree(root.children);
  return root.children;
}

function sortTree(nodes: TreeNode[]): void {
  nodes.sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "dir" ? -1 : 1));
  for (const node of nodes) {
    if (node.kind === "dir") sortTree(node.children);
  }
}
