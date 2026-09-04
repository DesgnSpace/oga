// The changes panel's two extra shapes: the reported view split by the turn
// that produced each file, and a real git diff mapped onto the same
// file-and-hunk model. Both stay pure so the panel only renders.

import type { TaskDiff, TaskEventView } from "@/bridge/types";
import {
  collectRunChanges,
  fileChangeFromPatch,
  type ChangedFileSet,
  type ChangedFileView,
  type RunFileChanges,
} from ".";
import { deriveTurnIds } from "@/domain/activity";

/** One round of work and the files it changed. */
export interface ChangeTurn {
  /** The broker's turn id. Absent for activity that predates the first turn. */
  turnId?: number;
  /** 1-based position among the task's turns; 0 for pre-turn activity. */
  ordinal: number;
  label: string;
  /** When the turn started. */
  at?: string;
  files: RunFileChanges[];
}

export interface ChangeTurnSet {
  /** Newest turn first. */
  turns: ChangeTurn[];
  unmatched: number;
}

/** What the reader did between turns, named the way the task screen names it. */
const TURN_CAUSES = new Map([
  ["answered", "Your answer"],
  ["steered", "Your instruction"],
  ["steer_accepted", "Your instruction"],
  ["follow_up_queued", "Your follow-up"],
  ["follow_up_started", "Your follow-up"],
  ["resumed", "Your follow-up"],
]);

const EARLIER_LABEL = "Earlier activity";

interface Bucket {
  turnId?: number;
  ordinal: number;
  label: string;
  at?: string;
  events: TaskEventView[];
}

/**
 * Splits a run's changed files by the turn that produced them.
 *
 * Turns come from the broker's own `turnId`, never inferred from event
 * contents: an event without one belongs to the turn still open around it.
 */
export function collectRunChangesByTurn(events: TaskEventView[], cwd: string): ChangeTurnSet {
  const buckets: Bucket[] = [];
  let current: Bucket | undefined;
  let cause: string | undefined;
  let ordinal = 0;

  for (const event of deriveTurnIds(events)) {
    const turnId = event.turnId;
    if (turnId !== undefined && current?.turnId !== turnId) {
      ordinal += 1;
      current = {
        turnId,
        ordinal,
        label: ordinal === 1 ? "First run" : (cause ?? `Turn ${ordinal}`),
        at: event.createdAt,
        events: [],
      };
      buckets.push(current);
      cause = undefined;
    } else if (current === undefined) {
      current = { ordinal: 0, label: EARLIER_LABEL, at: event.createdAt, events: [] };
      buckets.push(current);
    }
    current.events.push(event);
    const next = TURN_CAUSES.get(event.type);
    if (next !== undefined) cause = next;
  }

  let unmatched = 0;
  const turns: ChangeTurn[] = [];
  for (const bucket of buckets) {
    const changes = collectRunChanges(bucket.events, cwd);
    unmatched += changes.unmatched;
    if (changes.files.length === 0) continue;
    turns.push({
      turnId: bucket.turnId,
      ordinal: bucket.ordinal,
      label: bucket.label,
      at: bucket.at,
      files: changes.files,
    });
  }
  turns.reverse();
  return { turns, unmatched };
}

/** Maps the broker's git diff onto the model the panel already renders. */
export function gitChangeSet(diff: TaskDiff): ChangedFileSet {
  return { files: diff.files.map(gitFile), unmatched: 0 };
}

function gitFile(file: TaskDiff["files"][number]): ChangedFileView {
  return {
    path: file.path,
    change: file.patch === undefined ? { path: file.path, blocks: [] } : fileChangeFromPatch(file.patch, file.path),
    added: file.added,
    removed: file.removed,
    hiddenLines: 0,
    shortened: false,
    status: file.status,
    tooLarge: file.tooLarge,
  };
}
