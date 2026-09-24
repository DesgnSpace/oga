// Changed-file derivation and the bounded diff model used by the task detail UI.

import type { TaskDiffFileStatus, TaskEventView } from "@/bridge/types";

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
const INPUT_KEYS = ["tool_input", "input", "arguments", "args"];

// A read names a file too, so only these titles count as a change.
const CHANGE_TITLES = new Set([
  "edit file",
  "edit files",
  "write file",
  "write files",
  "create file",
  "delete file",
  "move file",
  "apply patch",
]);

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
  if (old === undefined && next !== undefined && next !== "") return addedWholeFile(path, next);

  for (const key of INPUT_KEYS) {
    if (key in fields) {
      const found = search(fields[key], path, true);
      if (found) return found;
    }
  }

  // A body in a tool input or create event is a write; a file read can carry a body too.
  if ((inInput || fields.type === "create") && path !== undefined) {
    const content = stringValue(fields, CONTENT_KEYS);
    if (content !== undefined) return addedWholeFile(path, content);
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

function addedWholeFile(path: string | undefined, body: string): FileChange {
  return { path, blocks: [collapse(splitLines(body).map((text): DiffLine => ({ kind: "added", text })))] };
}

function namedFile(event: TaskEventView): FileChange | undefined {
  if (event.kind !== "file" || !CHANGE_TITLES.has(event.title.trim().toLowerCase())) return undefined;
  const path = event.presentation?.path;
  return path !== undefined && path !== "" ? { path, blocks: [] } : undefined;
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
      if (!blocks.some((existing) => sameChange(existing, block))) blocks.push(block);
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

/**
 * The lines a block actually changed, which is what identifies the edit. A
 * worker reports one edit several ways — its own diff, the arguments it
 * passed, the patch its tool returned — each wrapping a different amount of
 * untouched context, and counting them all says it edited the file twice.
 */
function changedLines(block: DiffLine[]): DiffLine[] {
  const changed = block.filter((line) => line.kind === "added" || line.kind === "removed");
  return changed.length > 0 ? changed : block;
}

function sameChange(a: DiffLine[], b: DiffLine[]): boolean {
  return blocksEqual(changedLines(a), changedLines(b));
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
  /** The file's ready-to-render unified patch, when git reported one. */
  patch?: string;
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

/**
 * Folds a task's reported edits without rescanning earlier events on append.
 * The event array is stable while the task stream appends; a replay, overlap,
 * earlier page, or cwd change supplies a new array and takes the exact rebuild
 * path instead.
 */
export class RunChangeProjection {
  private source: TaskEventView[] | undefined;
  private cwd: string | undefined;
  private length = 0;
  private result: RunChangeSet = { files: [], unmatched: 0 };
  private files = new Map<string, MutableRunFile>();

  update(events: TaskEventView[], cwd: string): RunChangeSet {
    const canAppend = this.source === events && this.cwd === cwd && events.length >= this.length;
    if (!canAppend) this.reset(cwd);

    for (let index = canAppend ? this.length : 0; index < events.length; index += 1) {
      this.consume(events[index], cwd);
    }
    this.source = events;
    this.cwd = cwd;
    this.length = events.length;
    return this.result;
  }

  private reset(cwd: string): void {
    this.cwd = cwd;
    this.length = 0;
    this.files = new Map();
    this.result = { files: [], unmatched: 0 };
  }

  private consume(event: TaskEventView, cwd: string): void {
    const raw = event.rawText;
    if (raw === undefined) return;
    if (event.kind === "retry" || (event.kind !== "file" && !fileChangeMayContainEdit(raw))) return;
    const change = fileChangeFromRaw(raw) ?? namedFile(event);
    if (!change) return;
    if (change.path === undefined) {
      this.result.unmatched += 1;
      return;
    }

    const path = relativePath(change.path, cwd);
    const existing = this.files.get(path);

    const fresh: DiffLine[][] = [];
    for (const block of change.blocks) {
      const key = diffBlockKey(block);
      const known = existing?.blocksByKey.get(key);
      if (known !== undefined && known.some((existing) => sameChange(existing, block))) continue;
      fresh.push(block);
    }
    // A file named without its lines still belongs in the list the first time
    // it is named; naming it again adds nothing.
    const firstSightingWithoutLines = existing === undefined && change.blocks.length === 0;
    if (fresh.length === 0 && !firstSightingWithoutLines) return;

    const file = existing ?? createMutableRunFile(path);
    if (existing === undefined) {
      this.files.set(path, file);
      this.result.files.push(file.view);
    }
    file.edits += 1;
    for (const block of fresh) {
      const key = diffBlockKey(block);
      const known = file.blocksByKey.get(key);
      file.blocksByKey.set(key, [...(known ?? []), block]);
      file.added += countBlockLines(block, "added");
      file.removed += countBlockLines(block, "removed");
      file.shortened ||= block.some((line) => line.text.includes("…[truncated: kept "));
      if (file.drawn >= RUN_CHANGES_LINE_LIMIT) {
        file.hiddenLines += block.length;
      } else {
        file.drawn += block.length;
        file.kept.push(block);
      }
    }
    file.view.edits = file.edits;
    file.view.added = file.added;
    file.view.removed = file.removed;
    file.view.hiddenLines = file.hiddenLines;
    file.view.shortened = file.shortened;
    file.view.change = { path, blocks: file.kept };
  }
}

interface MutableRunFile {
  blocksByKey: Map<string, DiffLine[][]>;
  kept: DiffLine[][];
  drawn: number;
  hiddenLines: number;
  edits: number;
  added: number;
  removed: number;
  shortened: boolean;
  view: RunFileChanges;
}

function createMutableRunFile(path: string): MutableRunFile {
  const file: MutableRunFile = {
    blocksByKey: new Map(),
    kept: [],
    drawn: 0,
    hiddenLines: 0,
    edits: 0,
    added: 0,
    removed: 0,
    shortened: false,
    view: {
      path,
      change: { path, blocks: [] },
      edits: 0,
      added: 0,
      removed: 0,
      hiddenLines: 0,
      shortened: false,
    },
  };
  return file;
}

function diffBlockKey(block: DiffLine[]): string {
  const lines = changedLines(block);
  let hash = 2_166_136_261;
  for (const line of lines) {
    hash ^= line.kind.charCodeAt(0);
    hash = Math.imul(hash, 16_777_619);
    for (let index = 0; index < line.text.length; index += 1) {
      hash ^= line.text.charCodeAt(index);
      hash = Math.imul(hash, 16_777_619);
    }
  }
  return `${lines.length}:${hash >>> 0}`;
}

function countBlockLines(block: DiffLine[], kind: DiffKind): number {
  return block.reduce((count, line) => count + (line.kind === kind ? 1 : 0), 0);
}

export function collectRunChanges(events: TaskEventView[], cwd: string): RunChangeSet {
  return new RunChangeProjection().update(events, cwd);
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
