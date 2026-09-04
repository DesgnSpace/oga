// Changed-file derivation and the bounded diff model used by the task detail UI.
// Ported from rust/crates/oga-ui/src/changes/mod.rs — keep behavior identical.
// Rendering (the changed-files panel, syntax highlighting) is out of scope here;
// this module only ports the pure event -> file/diff derivation.

import type { EventKind, TaskDiffFileStatus, TaskEventView } from "@/bridge/types";

export type DiffKind = "context" | "added" | "removed" | "skipped";

export interface DiffLine {
  kind: DiffKind;
  text: string;
}

export interface FileChange {
  path?: string;
  blocks: DiffLine[][];
}

const ALIGN_LIMIT = 400;
const MARGIN = 3;
const BLOCK_LIMIT = 200;

const PATH_KEYS = ["file_path", "filePath", "path"];
const OLD_KEYS = ["old_string", "oldString", "old_text", "oldText"];
const NEW_KEYS = ["new_string", "newString", "new_text", "newText"];
const CONTENT_KEYS = ["content", "text"];
const INPUT_KEYS = ["tool_input", "input", "arguments"];

const EDIT_MARKER_KEYS = [
  "old_string",
  "oldString",
  "old_text",
  "oldText",
  "new_string",
  "newString",
  "new_text",
  "newText",
  "structuredPatch",
  "@@ -",
  "*** Begin Patch",
];

/** Cheap gate used before decoding every provider payload in a trace. */
export function fileChangeMayContainEdit(raw: string): boolean {
  return EDIT_MARKER_KEYS.some((key) => raw.includes(key));
}

export function fileChangeFromRaw(raw: string): FileChange | undefined {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return undefined;
  }
  return search(value, undefined, false);
}

/**
 * Reads one file's unified diff into the same blocks an event-derived change
 * carries, so both sources render through one viewer.
 */
export function fileChangeFromPatch(patch: string, path: string): FileChange {
  return unified(patch, path) ?? { path, blocks: [] };
}

export function countDiffLines(change: FileChange, kind: DiffKind): number {
  let count = 0;
  for (const block of change.blocks) {
    for (const line of block) {
      if (line.kind === kind) count += 1;
    }
  }
  return count;
}

export function fileChangeAdded(change: FileChange): number {
  return countDiffLines(change, "added");
}

export function fileChangeRemoved(change: FileChange): number {
  return countDiffLines(change, "removed");
}

/**
 * Builds a line-oriented replacement. Small replacements retain context;
 * large replacements are represented as one removed block and one added block.
 */
export function diffLines(oldText: string, newText: string): DiffLine[] {
  const before = splitLines(oldText);
  const after = splitLines(newText);
  if (before.length > ALIGN_LIMIT || after.length > ALIGN_LIMIT) {
    return collapse([
      ...before.map((text): DiffLine => ({ kind: "removed", text })),
      ...after.map((text): DiffLine => ({ kind: "added", text })),
    ]);
  }
  return collapse(group(align(before, after)));
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function search(value: unknown, inheritedPath: string | undefined, inInput: boolean): FileChange | undefined {
  if (Array.isArray(value)) {
    const found = value.map((item) => search(item, inheritedPath, inInput)).filter((item): item is FileChange => item !== undefined);
    return merge(found, inheritedPath);
  }
  if (!isObject(value)) return undefined;

  const fields = value;
  const path = stringValue(fields, PATH_KEYS) ?? inheritedPath;

  const old = rawString(fields, OLD_KEYS);
  const next = rawString(fields, NEW_KEYS);
  if (old !== undefined && next !== undefined && (old !== "" || next !== "")) {
    return { path, blocks: [diffLines(old, next)] };
  }

  for (const key of INPUT_KEYS) {
    if (key in fields) {
      const found = search(fields[key], path, true);
      if (found) return found;
    }
  }

  if (inInput && path !== undefined) {
    const content = stringValue(fields, CONTENT_KEYS);
    if (content !== undefined) {
      return {
        path,
        blocks: [collapse(splitLines(content).map((text): DiffLine => ({ kind: "added", text })))],
      };
    }
  }

  if ("structuredPatch" in fields) {
    const found = structuredPatch(fields.structuredPatch, path);
    if (found) return found;
  }
  const patch = rawString(fields, ["patch"]);
  if (patch !== undefined) {
    const found = unified(patch, path);
    if (found) return found;
  }
  const patchText = rawString(fields, ["patchText"]);
  if (patchText !== undefined) {
    const found = applyPatch(patchText, path);
    if (found) return found;
  }

  const found = Object.keys(fields)
    .map((key) => search(fields[key], path, inInput))
    .filter((item): item is FileChange => item !== undefined);
  return merge(found, path);
}

function stringValue(fields: Record<string, unknown>, keys: string[]): string | undefined {
  const value = rawString(fields, keys);
  return value === "" ? undefined : value;
}

function rawString(fields: Record<string, unknown>, keys: string[]): string | undefined {
  for (const key of keys) {
    const value = fields[key];
    if (typeof value === "string") return value;
  }
  return undefined;
}

function structuredPatch(value: unknown, path: string | undefined): FileChange | undefined {
  if (!Array.isArray(value)) return undefined;
  const blocks: DiffLine[][] = [];
  for (const item of value) {
    if (!isObject(item) || !Array.isArray(item.lines)) continue;
    const parsed: DiffLine[] = [];
    for (const line of item.lines) {
      if (typeof line !== "string") continue;
      parsed.push(patchLine(line));
    }
    if (parsed.length > 0) blocks.push(collapse(parsed));
  }
  return blocks.length > 0 ? { path, blocks } : undefined;
}

function patchLine(line: string): DiffLine {
  const marker = line[0];
  if (marker === "+") return { kind: "added", text: line.slice(1) };
  if (marker === "-") return { kind: "removed", text: line.slice(1) };
  if (marker === " ") return { kind: "context", text: line.slice(1) };
  return { kind: "context", text: line };
}

function unified(text: string, inheritedPath: string | undefined): FileChange | undefined {
  if (!text.includes("@@")) return undefined;
  let path = inheritedPath;
  const blocks: DiffLine[][] = [];
  let current: DiffLine[] = [];
  for (const line of text.split("\n")) {
    if (line.startsWith("--- ") || line.startsWith("+++ ")) {
      const header = headerPath(line);
      if (header !== undefined) path = header;
      continue;
    }
    if (line.startsWith("@@")) {
      if (current.length > 0) {
        blocks.push(collapse(current));
        current = [];
      }
      continue;
    }
    if (line.startsWith("\\")) continue;
    const marker = line[0];
    if (marker === "+") current.push({ kind: "added", text: line.slice(1) });
    else if (marker === "-") current.push({ kind: "removed", text: line.slice(1) });
    else if (marker === " ") current.push({ kind: "context", text: line.slice(1) });
  }
  if (current.length > 0) blocks.push(collapse(current));
  return blocks.length > 0 ? { path, blocks } : undefined;
}

/**
 * Codex's `apply_patch` tool call: a custom envelope, not a unified diff —
 * `*** Begin Patch`, one `*** Update File:` / `*** Add File:` / `*** Delete
 * File:` marker per file, `@@` hunk separators with no line numbers, then
 * +/-/space lines, closed by `*** End Patch`.
 */
function applyPatch(text: string, inheritedPath: string | undefined): FileChange | undefined {
  if (!text.includes("*** Begin Patch")) return undefined;
  let path = inheritedPath;
  const blocks: DiffLine[][] = [];
  let current: DiffLine[] = [];
  const flush = () => {
    if (current.length > 0) blocks.push(collapse(current));
    current = [];
  };
  for (const line of text.split("\n")) {
    if (line.startsWith("*** Update File: ") || line.startsWith("*** Add File: ") || line.startsWith("*** Delete File: ")) {
      flush();
      path = line.slice(line.indexOf(": ") + 2).trim() || path;
      continue;
    }
    if (line.startsWith("*** ")) {
      flush();
      continue;
    }
    if (line.startsWith("@@")) {
      flush();
      continue;
    }
    const marker = line[0];
    if (marker === "+") current.push({ kind: "added", text: line.slice(1) });
    else if (marker === "-") current.push({ kind: "removed", text: line.slice(1) });
    else if (marker === " ") current.push({ kind: "context", text: line.slice(1) });
  }
  flush();
  return blocks.length > 0 ? { path, blocks } : undefined;
}

function headerPath(line: string): string | undefined {
  const value = line.slice(4).trim();
  if (value === "" || value === "/dev/null") return undefined;
  if (value.startsWith("a/") || value.startsWith("b/")) return value.slice(2);
  return value;
}

function merge(found: FileChange[], inheritedPath: string | undefined): FileChange | undefined {
  const blocks: DiffLine[][] = [];
  for (const change of found) {
    for (const block of change.blocks) {
      if (!blocks.some((existing) => blocksEqual(existing, block))) blocks.push(block);
    }
  }
  if (blocks.length === 0) return undefined;
  return {
    path: found.find((change) => change.path !== undefined)?.path ?? inheritedPath,
    blocks,
  };
}

function blocksEqual(a: DiffLine[], b: DiffLine[]): boolean {
  return a.length === b.length && a.every((line, index) => line.kind === b[index].kind && line.text === b[index].text);
}

function splitLines(value: string): string[] {
  if (value === "") return [];
  const normalized = value.replace(/\r\n/g, "\n");
  const lines = normalized.split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

function align(before: string[], after: string[]): DiffLine[] {
  if (before.length === 0) return after.map((text): DiffLine => ({ kind: "added", text }));
  if (after.length === 0) return before.map((text): DiffLine => ({ kind: "removed", text }));

  const numbers = new Map<string, number>();
  const number = (line: string): number => {
    let value = numbers.get(line);
    if (value === undefined) {
      value = numbers.size;
      numbers.set(line, value);
    }
    return value;
  };
  const left = before.map(number);
  const right = after.map(number);
  const width = right.length + 1;
  const table = new Array<number>((left.length + 1) * width).fill(0);
  for (let i = left.length - 1; i >= 0; i -= 1) {
    for (let j = right.length - 1; j >= 0; j -= 1) {
      const index = i * width + j;
      table[index] =
        left[i] === right[j]
          ? table[(i + 1) * width + j + 1] + 1
          : Math.max(table[(i + 1) * width + j], table[i * width + j + 1]);
    }
  }

  const result: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < left.length && j < right.length) {
    if (left[i] === right[j]) {
      result.push({ kind: "context", text: before[i] });
      i += 1;
      j += 1;
    } else if (table[(i + 1) * width + j] >= table[i * width + j + 1]) {
      result.push({ kind: "removed", text: before[i] });
      i += 1;
    } else {
      result.push({ kind: "added", text: after[j] });
      j += 1;
    }
  }
  for (; i < before.length; i += 1) result.push({ kind: "removed", text: before[i] });
  for (; j < after.length; j += 1) result.push({ kind: "added", text: after[j] });
  return result;
}

function group(lines: DiffLine[]): DiffLine[] {
  const result: DiffLine[] = [];
  let run: DiffLine[] = [];
  const flush = () => {
    result.push(...run.filter((line) => line.kind === "removed"));
    result.push(...run.filter((line) => line.kind === "added"));
    run = [];
  };
  for (const line of lines) {
    if (line.kind === "context") {
      flush();
      result.push(line);
    } else {
      run.push(line);
    }
  }
  flush();
  return result;
}

function collapse(lines: DiffLine[]): DiffLine[] {
  if (lines.length === 0) return lines;
  const keep = new Array<boolean>(lines.length).fill(false);
  lines.forEach((line, index) => {
    if (line.kind !== "context") {
      const start = Math.max(0, index - MARGIN);
      const end = Math.min(lines.length - 1, index + MARGIN);
      for (let value = start; value <= end; value += 1) keep[value] = true;
    }
  });
  if (!keep.some(Boolean)) return capped(lines);

  const result: DiffLine[] = [];
  let index = 0;
  while (index < lines.length) {
    if (keep[index]) {
      result.push(lines[index]);
      index += 1;
      continue;
    }
    const start = index;
    while (index < lines.length && !keep[index]) index += 1;
    const skipped = index - start;
    if (skipped > 3) {
      result.push({ kind: "skipped", text: `${skipped} unchanged lines` });
    } else {
      result.push(...lines.slice(start, index));
    }
  }
  return capped(result);
}

function capped(lines: DiffLine[]): DiffLine[] {
  if (lines.length <= BLOCK_LIMIT) return lines;
  const rest = lines.length - BLOCK_LIMIT;
  const result = lines.slice(0, BLOCK_LIMIT);
  result.push({ kind: "skipped", text: `${rest} more line${rest === 1 ? "" : "s"}` });
  return result;
}

/** One file's diff as the changes panel draws it, whichever source produced it. */
export interface ChangedFileView {
  path: string;
  change: FileChange;
  added: number;
  removed: number;
  hiddenLines: number;
  shortened: boolean;
  /** How git classified the file. Absent in the event-derived view. */
  status?: TaskDiffFileStatus;
  /** Set when the source left the diff body out because it was too big. */
  tooLarge?: boolean;
}

export interface ChangedFileSet {
  files: ChangedFileView[];
  unmatched: number;
}

export interface RunFileChanges extends ChangedFileView {
  edits: number;
}

export interface RunChangeSet extends ChangedFileSet {
  files: RunFileChanges[];
}

export function runChangeSetAdded(set: ChangedFileSet): number {
  return set.files.reduce((sum, file) => sum + file.added, 0);
}

export function runChangeSetRemoved(set: ChangedFileSet): number {
  return set.files.reduce((sum, file) => sum + file.removed, 0);
}

export function runChangeSetIsEmpty(set: ChangedFileSet): boolean {
  return set.files.length === 0 && set.unmatched === 0;
}

export const RUN_CHANGES_EMPTY: RunChangeSet = { files: [], unmatched: 0 };

/** A run's diffs are bounded to this many lines before the rest counts as hidden. */
export const RUN_CHANGES_LINE_LIMIT = 600;

export function collectRunChanges(events: TaskEventView[], cwd: string): RunChangeSet {
  const order: string[] = [];
  const blocks = new Map<string, DiffLine[][]>();
  const edits = new Map<string, number>();
  let unmatched = 0;

  for (const event of events) {
    const raw = event.rawText;
    if (raw === undefined) continue;
    const skippable: EventKind[] = ["retry"];
    if (skippable.includes(event.kind) || (event.kind !== "file" && !fileChangeMayContainEdit(raw))) continue;
    const change = fileChangeFromRaw(raw);
    if (!change) continue;
    if (change.path === undefined) {
      unmatched += 1;
      continue;
    }
    const path = relativePath(change.path, cwd);
    const stack = blocks.get(path) ?? [];
    blocks.set(path, stack);
    const fresh = change.blocks.filter((block) => !stack.some((existing) => blocksEqual(existing, block)));
    if (fresh.length === 0) continue;
    if (!edits.has(path)) order.push(path);
    stack.push(...fresh);
    edits.set(path, (edits.get(path) ?? 0) + 1);
  }

  const files = order.map((path) => fileEntry(path, blocks.get(path) ?? [], edits.get(path) ?? 0));
  return { files, unmatched };
}

function fileEntry(path: string, blocks: DiffLine[][], edits: number): RunFileChanges {
  const kept: DiffLine[][] = [];
  let drawn = 0;
  let hidden = 0;
  for (const block of blocks) {
    if (drawn >= RUN_CHANGES_LINE_LIMIT) {
      hidden += block.length;
    } else {
      drawn += block.length;
      kept.push(block);
    }
  }
  const flat = blocks.flat();
  const added = flat.filter((line) => line.kind === "added").length;
  const removed = flat.filter((line) => line.kind === "removed").length;
  const shortened = flat.some((line) => line.text.includes("…[truncated: kept "));
  return {
    path,
    change: { path, blocks: kept },
    edits,
    added,
    removed,
    hiddenLines: hidden,
    shortened,
  };
}

export function relativePath(path: string, cwd: string): string {
  const trimmedPath = path.replace(/\/+$/, "");
  const trimmedCwd = cwd.replace(/\/+$/, "");
  if (trimmedCwd !== "") {
    if (trimmedPath === trimmedCwd) return ".";
    const prefix = `${trimmedCwd}/`;
    if (trimmedPath.startsWith(prefix)) return trimmedPath.slice(prefix.length);
  }
  return trimmedPath;
}
