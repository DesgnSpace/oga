// Which files a reviewer has marked read, per task, kept in their own browser.

import * as React from "react";
import type { ChangedFileView } from "@/domain/changes";
import { readStorage, writeStorage } from "./storage";

const REVIEWED_FILES_KEY = "oga:reviewed-files";

/** How many tasks keep their marks; the least recently touched go first. */
const MAX_TASKS = 32;

interface TaskMarks {
  /** By file path, the fingerprint of the diff the reviewer saw. */
  files: Record<string, string>;
  touchedAt: number;
}

type StoredMarks = Record<string, TaskMarks>;

let marks: StoredMarks | undefined;
let version = 0;
const listeners = new Set<() => void>();

const fingerprints = new WeakMap<ChangedFileView, string>();

/** A worker's later edit to a file changes this, which is what drops the mark. */
function changeFingerprint(file: ChangedFileView): string {
  const cached = fingerprints.get(file);
  if (cached !== undefined) return cached;
  let hash = 2_166_136_261; // FNV-1a offset basis
  const mix = (text: string) => {
    for (let index = 0; index < text.length; index += 1) {
      hash ^= text.charCodeAt(index);
      hash = Math.imul(hash, 16_777_219); // FNV-1a prime
    }
  };
  if (file.patch !== undefined) {
    mix(file.patch);
  } else {
    for (const block of file.change.blocks) {
      for (const line of block) {
        mix(line.kind);
        mix(line.text);
      }
    }
  }
  mix(`${file.added}:${file.removed}`);
  const fingerprint = (hash >>> 0).toString(36);
  fingerprints.set(file, fingerprint);
  return fingerprint;
}

function isTaskMarks(value: unknown): value is TaskMarks { // oxlint-disable-line anti-slop/no-unknown-parameters -- localStorage JSON is untrusted input
  // oxlint-disable-next-line anti-slop/no-runtime-typeof -- persisted JSON must be narrowed at this boundary
  if (typeof value !== "object" || value === null) return false;
  // SAFETY: the object check above establishes that this persisted JSON value can be read as a record.
  const task = value as Partial<TaskMarks>;
  // oxlint-disable-next-line anti-slop/no-runtime-typeof -- persisted JSON field validation
  if (typeof task.touchedAt !== "number" || !Number.isFinite(task.touchedAt)) return false;
  // oxlint-disable-next-line anti-slop/no-runtime-typeof -- persisted JSON field validation
  if (typeof task.files !== "object" || task.files === null) return false;
  return Object.values(task.files).every(
    // oxlint-disable-next-line anti-slop/no-runtime-typeof -- persisted JSON field validation
    (fingerprint) => typeof fingerprint === "string",
  );
}

function load(): StoredMarks {
  const raw = readStorage(REVIEWED_FILES_KEY);
  if (raw === undefined) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    // oxlint-disable-next-line anti-slop/no-runtime-typeof -- persisted JSON must be narrowed at this boundary
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
    const entries = Object.entries(parsed).filter((entry): entry is [string, TaskMarks] => isTaskMarks(entry[1]));
    return bounded(Object.fromEntries(entries));
  } catch {
    return {};
  }
}

function bounded(stored: StoredMarks): StoredMarks {
  const entries = Object.entries(stored);
  if (entries.length <= MAX_TASKS) return stored;
  return Object.fromEntries(
    entries
      .sort(([, left], [, right]) => right.touchedAt - left.touchedAt)
      .slice(0, MAX_TASKS),
  );
}

function stored(): StoredMarks {
  marks ??= load();
  return marks;
}

function commit(next: StoredMarks): void {
  marks = bounded(next);
  writeStorage(REVIEWED_FILES_KEY, JSON.stringify(marks));
  version += 1;
  for (const listener of listeners) listener();
}

function save(taskId: string, files: Record<string, string>): void {
  commit({ ...stored(), [taskId]: { files, touchedAt: Date.now() } });
}

function mark(taskId: string, file: ChangedFileView): void {
  save(taskId, { ...stored()[taskId]?.files, [file.path]: changeFingerprint(file) });
}

function unmark(taskId: string, file: ChangedFileView): void {
  const current = stored()[taskId];
  if (current === undefined) return;
  const next = { ...current.files };
  delete next[file.path];
  save(taskId, next);
}

function isMarked(taskId: string, file: ChangedFileView): boolean {
  return stored()[taskId]?.files[file.path] === changeFingerprint(file);
}

export interface ReviewedFiles {
  /** Whether the file's diff is still the one the reviewer marked. */
  isReviewed: (file: ChangedFileView) => boolean;
  toggle: (file: ChangedFileView) => void;
  count: (files: readonly ChangedFileView[]) => number;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function snapshot(): number {
  return version;
}

export function useReviewedFiles(taskId: string): ReviewedFiles {
  const revision = React.useSyncExternalStore(subscribe, snapshot, snapshot);
  return React.useMemo(
    () => ({
      isReviewed: (file: ChangedFileView) => isMarked(taskId, file),
      toggle: (file: ChangedFileView) =>
        isMarked(taskId, file) ? unmark(taskId, file) : mark(taskId, file),
      count: (files: readonly ChangedFileView[]) => files.filter((file) => isMarked(taskId, file)).length,
    }),
    [taskId, revision],
  );
}

export function resetReviewedFilesForTests(): void {
  commit({});
}
