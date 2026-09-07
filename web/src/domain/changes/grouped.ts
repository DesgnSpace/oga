// The changes panel's two extra shapes: the reported view split by the turn
// that produced each file, and a real git diff mapped onto the same
// file-and-hunk model. Both stay pure so the panel only renders.

import type { TaskDiff, TaskEventView } from "@/bridge/types";
import {
  fileChangeFromPatch,
  RunChangeProjection,
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

interface BucketState {
  bucket: Bucket;
  projection: RunChangeProjection;
  unmatched: number;
  turn?: ChangeTurn;
}

/**
 * Keeps the grouped reported view append-only while the panel is visible.
 * Pages, replays, and inferred turn boundaries use the exact rebuild path.
 */
export class RunChangeByTurnProjection {
  private source: TaskEventView[] | undefined;
  private length = 0;
  private cwd: string | undefined;
  private explicitTurns = true;
  private current: BucketState | undefined;
  private cause: string | undefined;
  private ordinal = 0;
  private result: ChangeTurnSet = { turns: [], unmatched: 0 };

  update(events: TaskEventView[], cwd: string): ChangeTurnSet {
    const append = this.canAppend(events, cwd);
    if (!append) this.reset(cwd, events);

    if (append) {
      for (let index = this.length; index < events.length; index += 1) this.consume(events[index], cwd);
    } else {
      const source = this.explicitTurns ? events : deriveTurnIds(events);
      for (const event of source) this.consume(event, cwd);
    }

    this.source = events;
    this.length = events.length;
    this.cwd = cwd;
    return this.result;
  }

  private canAppend(events: TaskEventView[], cwd: string): boolean {
    if (!this.explicitTurns || this.source !== events || this.cwd !== cwd || events.length < this.length) return false;
    for (let index = this.length; index < events.length; index += 1) {
      if (events[index].turnId === undefined) return false;
    }
    return true;
  }

  private reset(cwd: string, events: TaskEventView[]): void {
    this.cwd = cwd;
    this.explicitTurns = events.every((event) => event.turnId !== undefined);
    this.length = 0;
    this.current = undefined;
    this.cause = undefined;
    this.ordinal = 0;
    this.result = { turns: [], unmatched: 0 };
  }

  private consume(event: TaskEventView, cwd: string): void {
    if (event.turnId !== undefined && this.current?.bucket.turnId !== event.turnId) {
      this.ordinal += 1;
      this.current = this.newBucket(
        event,
        event.turnId,
        this.ordinal === 1 ? "First run" : (this.cause ?? `Turn ${this.ordinal}`),
      );
      this.cause = undefined;
    } else if (this.current === undefined) {
      this.current = this.newBucket(event, undefined, EARLIER_LABEL);
    }

    this.current.bucket.events.push(event);
    const changes = this.current.projection.update(this.current.bucket.events, cwd);
    this.result.unmatched += changes.unmatched - this.current.unmatched;
    this.current.unmatched = changes.unmatched;
    if (changes.files.length > 0) {
      if (this.current.turn === undefined) {
        this.current.turn = {
          turnId: this.current.bucket.turnId,
          ordinal: this.current.bucket.ordinal,
          label: this.current.bucket.label,
          at: this.current.bucket.at,
          files: changes.files,
        };
        this.result.turns = [this.current.turn, ...this.result.turns];
      } else {
        this.current.turn.files = changes.files;
      }
    }

    const next = TURN_CAUSES.get(event.type);
    if (next !== undefined) this.cause = next;
  }

  private newBucket(event: TaskEventView, turnId: number | undefined, label: string): BucketState {
    return {
      bucket: { turnId, ordinal: turnId === undefined ? 0 : this.ordinal, label, at: event.createdAt, events: [] },
      projection: new RunChangeProjection(),
      unmatched: 0,
    };
  }
}

/**
 * Splits a run's changed files by the turn that produced them.
 *
 * Turns come from the broker's own `turnId`, never inferred from event
 * contents: an event without one belongs to the turn still open around it.
 */
export function collectRunChangesByTurn(events: TaskEventView[], cwd: string): ChangeTurnSet {
  return new RunChangeByTurnProjection().update(events, cwd);
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
