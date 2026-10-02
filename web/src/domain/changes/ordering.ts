// The order a reviewer reads changed files in: folder order or by size, and the
// files a machine writes that nobody reads line by line.

import type { ChangedFileView, RunFileChanges } from ".";
import type { ChangeTurn, ChangeTurnSet } from "./grouped";

/** How the panel lists files, either the order they arrive in or the biggest first. */
export type ChangeSort = "folder" | "size";

export const CHANGE_SORTS: readonly ChangeSort[] = ["folder", "size"];

/** The heading the generated files sit under, in the list and in the file tree. */
export const GENERATED_GROUP_NAME = "Generated";

/**
 * What the group's collapsed state is keyed by in the list and in the tree, so
 * one closed-by-default setting covers both. Not a path: the files keep their
 * own, so nothing on disk can collide with it.
 */
export const GENERATED_GROUP_KEY = "oga:generated-files";

export interface OrderedChanges<T> {
  /** The files a reviewer reads. */
  files: T[];
  /** The files a machine wrote. */
  generated: T[];
}

export interface OrderedChangeTurn {
  turn: ChangeTurn;
  files: RunFileChanges[];
  generated: RunFileChanges[];
}

/**
 * Lockfiles: a package manager writes the whole file, and a change in it only
 * ever says which dependency moved.
 */
const LOCKFILES: ReadonlySet<string> = new Set([
  "Cargo.lock",
  "Gemfile.lock",
  "Pipfile.lock",
  "bun.lock",
  "bun.lockb",
  "composer.lock",
  "flake.lock",
  "go.sum",
  "gradle.lockfile",
  "npm-shrinkwrap.json",
  "package-lock.json",
  "packages.lock.json",
  "pnpm-lock.yaml",
  "poetry.lock",
  "uv.lock",
  "yarn.lock",
]);

/** Where a build drops what it made, in the ecosystems Oga runs in. */
const BUILD_DIRS: ReadonlySet<string> = new Set([
  ".build",
  ".gradle",
  ".next",
  ".nuxt",
  ".output",
  ".parcel-cache",
  ".svelte-kit",
  ".turbo",
  ".venv",
  "__pycache__",
  "build",
  "coverage",
  "dist",
  "node_modules",
  "out",
  "target",
  "vendor",
  "venv",
]);

/**
 * Test-runner snapshots, minified bundles and the source maps beside them.
 * `.snap.new` is what a failed snapshot test leaves behind for the next run.
 */
const GENERATED_SUFFIXES: readonly string[] = [
  ".bundle.js",
  ".map",
  ".min.cjs",
  ".min.css",
  ".min.js",
  ".min.mjs",
  ".snap",
  ".snap.new",
];

const GENERATED_DIRS: ReadonlySet<string> = new Set([...BUILD_DIRS, "__snapshots__"]);

export function pathBaseName(path: string): string {
  const segments = path.split("/").filter((segment) => segment !== "");
  return segments[segments.length - 1] ?? path;
}

/** Whether a path names a file no reviewer reads line by line. */
export function isGeneratedFilePath(path: string): boolean {
  const name = pathBaseName(path);
  const segments = path.split("/").filter((segment) => segment !== "");
  if (LOCKFILES.has(name)) return true;
  if (segments.slice(0, -1).some((segment) => GENERATED_DIRS.has(segment))) return true;
  return GENERATED_SUFFIXES.some((suffix) => name.endsWith(suffix));
}

/** How much a file changed, which is what ranks it under size order. */
export function fileChangeSize(file: ChangedFileView): number {
  return file.added + file.removed;
}

/** Biggest change first. Sort is stable, so equal files keep the order they came in. */
export function largestFirst(a: ChangedFileView, b: ChangedFileView): number {
  return fileChangeSize(b) - fileChangeSize(a);
}

/**
 * Splits a file list into the files a reviewer reads and the generated ones, in
 * the chosen order. Folder order is the order the panel already had: the
 * reported order follows the run, the git order follows the branch.
 */
export function orderChangeFiles<T extends ChangedFileView>(files: readonly T[], sort: ChangeSort): OrderedChanges<T> {
  const readable: T[] = [];
  const generated: T[] = [];
  for (const file of files) {
    (isGeneratedFilePath(file.path) ? generated : readable).push(file);
  }
  if (sort === "size") {
    readable.sort(largestFirst);
    generated.sort(largestFirst);
  }
  return { files: readable, generated };
}

/** Applies the same order inside every turn, so grouping never changes the order. */
export function orderChangeTurns(turns: ChangeTurnSet, sort: ChangeSort): OrderedChangeTurn[] {
  return turns.turns.map((turn) => ({ turn, ...orderChangeFiles(turn.files, sort) }));
}
