// Changed-files panel width, persisted the same way as the task sidebar width.

import { readStorage, writeStorage } from "./storage";

export const CHANGED_FILES_MIN_WIDTH = 280;
export const CHANGED_FILES_MAX_WIDTH = 720;
export const CHANGED_FILES_DEFAULT_WIDTH = 380;

const CHANGED_FILES_WIDTH_KEY = "changedFilesPanelWidth";
const CHANGES_SOURCE_KEY = "oga:changes-source";
const CHANGES_GROUP_KEY = "oga:changes-group-by-turn";

export type ChangesSource = "reported" | "git";

export function clampChangedFilesWidth(width: number): number {
  return Math.min(Math.max(width, CHANGED_FILES_MIN_WIDTH), CHANGED_FILES_MAX_WIDTH);
}

export function loadChangedFilesWidth(): number {
  const raw = readStorage(CHANGED_FILES_WIDTH_KEY);
  if (raw === undefined) return CHANGED_FILES_DEFAULT_WIDTH;
  const parsed = Number.parseInt(raw, 10);
  if (!Number.isFinite(parsed)) return CHANGED_FILES_DEFAULT_WIDTH;
  return clampChangedFilesWidth(parsed);
}

export function storeChangedFilesWidth(width: number): void {
  writeStorage(CHANGED_FILES_WIDTH_KEY, String(width));
}

export function loadChangesSource(): ChangesSource {
  return readStorage(CHANGES_SOURCE_KEY) === "git" ? "git" : "reported";
}

export function storeChangesSource(source: ChangesSource): void {
  writeStorage(CHANGES_SOURCE_KEY, source);
}

export function loadChangesGrouped(): boolean {
  return readStorage(CHANGES_GROUP_KEY) === "1";
}

export function storeChangesGrouped(grouped: boolean): void {
  writeStorage(CHANGES_GROUP_KEY, grouped ? "1" : "0");
}
