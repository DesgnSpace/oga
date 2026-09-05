// Turns the flat activity stream into a conversation transcript: the original
// request, follow-ups and answers as bubbles, and everything the worker did
// between them as a collapsible block ahead of its reply.
// Ported from rust/crates/oga-ui/src/task_detail/transcript.rs — keep behavior identical.

import type { Task, TaskAttempt, TaskCompletion, TaskEventView, TaskState } from "@/bridge/types";
import {
  ActivityStory,
  deriveTurnIds,
  normalizeAntigravityEvents,
  type ActivityComposition,
  type ActivityEnding,
} from "@/domain/activity";
import type { TurnMarkerKind } from "@/domain/trace";

/** Sentinel id for the request bubble, which has no event of its own. */
export const REQUEST_ID = -1;

/** What opened the turn a bubble carries — absent on the original request. */
export type BubbleKind = Exclude<TurnMarkerKind, "handoff" | "response">;

export interface Bubble {
  id: number;
  text: string;
  at: string;
  kind?: BubbleKind;
  rawText?: string;
}

export interface WorkSegment {
  id: number;
  composition: ActivityComposition;
  cwd: string;
  live: boolean;
  startsExpanded: boolean;
  durationMs?: number;
}

export interface ResponseBlock {
  id: number;
  /** `undefined` means settled with nothing to show — "No response yet". */
  text?: string;
  error: boolean;
  /** An error run that stopped on something only a person can settle, rather than on a failure. */
  awaitingDecision?: boolean;
  /** Set when this reply follows a follow-up, an answer, a steer, or a
   * handoff — never on the response to the original request. */
  marker?: "response";
}

export type TranscriptItem =
  | { type: "bubble"; bubble: Bubble }
  | { type: "work"; segment: WorkSegment }
  | { type: "response"; block: ResponseBlock }
  | { type: "question"; block: ResponseBlock };

const SETTLED_STATES: TaskState[] = ["needs_input", "answered", "blocked", "completed", "failed", "cancelled"];

export function activityIsSettled(state: TaskState): boolean {
  return SETTLED_STATES.includes(state);
}

/**
 * Builds the transcript from the task row and its full activity stream.
 *
 * A resumed task keeps its events in one continuous stream, but each past
 * attempt's own reply lives on `task.attempts`, not in the stream — so the
 * stream is first cut at each attempt's `endedAt`, oldest first, and every
 * attempt gets its own walk: a follow-up or an answer becomes a bubble on
 * its own, "Worker needs input" ends the run in progress and surfaces the
 * question, and everything else accumulates into the run's collapsed
 * activity until one of those markers — or the attempt's own end — closes
 * it out with that attempt's reply. What's left after the last attempt is
 * the run still in progress, or the one the task last settled on.
 */
export function buildTranscript(task: Task, events: TaskEventView[], cache = new WorkSegmentCache()): TranscriptItem[] {
  const items: TranscriptItem[] = [
    { type: "bubble", bubble: { id: REQUEST_ID, text: task.prompt, at: task.createdAt } },
  ];

  let remaining = events;
  (task.attempts ?? []).forEach((attempt, index) => {
    const [ownEvents, rest] = splitAtAttemptEnd(remaining, attempt.endedAt);
    remaining = rest;
    const segment = walkRun(ownEvents, items, task, cache);
    flushRun(segment, items, task, cache, attemptResolver(attempt, index));
  });

  const segment = walkRun(remaining, items, task, cache);
  flushFinal(segment, items, task, cache);
  cache.settle();
  return items;
}

/**
 * Keeps the work segments a previous build produced, so a build that follows
 * new activity re-composes only the turns that activity touched. A segment is
 * reused whole, which is also what lets the view skip re-deriving its rows.
 */
export class WorkSegmentCache {
  private held = new Map<string, CachedSegment>();
  private built = new Map<string, CachedSegment>();

  /** The segment for this turn, composed only if the last build did not have it unchanged. */
  reuse(key: string, turn: TaskEventView[], compose: () => WorkSegment): WorkSegment {
    const previous = this.held.get(key);
    const entry = previous && sameEvents(previous.turn, turn) ? previous : { turn, segment: compose() };
    this.built.set(key, entry);
    return entry.segment;
  }

  /** Drops what this build did not ask for, so a long task cannot grow it forever. */
  settle(): void {
    this.held = this.built;
    this.built = new Map();
  }
}

interface CachedSegment {
  turn: TaskEventView[];
  segment: WorkSegment;
}

/** Merging replaces a revised event with a new object, so identity is the test. */
function sameEvents(left: TaskEventView[], right: TaskEventView[]): boolean {
  if (left.length !== right.length) return false;
  return left.every((event, index) => event === right[index]);
}

/** What kind of turn a user instruction event opens. */
function bubbleKind(title: string): BubbleKind | undefined {
  switch (title) {
    case "Follow-up queued":
    case "Follow-up started":
    case "Resumed":
      return "resume";
    case "Question answered":
      return "reply";
    case "Instruction sent":
      return "steer";
    default:
      return undefined;
  }
}

/** Whether a run already in progress was opened by a follow-up, an answer, a
 * steer, or a handoff — the response to it is marked; the response to the
 * original request is not. */
function hasMarkedTurn(items: TranscriptItem[]): boolean {
  return items.some((item) => {
    if (item.type === "bubble") return item.bubble.id !== REQUEST_ID;
    if (item.type === "work") return item.segment.composition.blocks.some((block) => block.type === "handoff");
    return false;
  });
}

/** Walks one run's events, peeling off bubbles and questions as they occur, and returns what's left to flush. */
function walkRun(
  events: TaskEventView[],
  items: TranscriptItem[],
  task: Task,
  cache: WorkSegmentCache,
): TaskEventView[] {
  let segment: TaskEventView[] = [];
  for (const event of events) {
    const kind = bubbleKind(event.title);
    if (kind !== undefined) {
      flushRun(segment, items, task, cache);
      segment = [];
      items.push({
        type: "bubble",
        bubble: {
          id: event.id,
          text: event.detail ?? bubbleFallback(kind),
          at: event.createdAt,
          kind,
          rawText: event.rawText,
        },
      });
    } else if (event.title === "Worker needs input") {
      flushRun(segment, items, task, cache);
      segment = [];
      items.push({
        type: "question",
        block: { id: event.id, text: event.detail ?? "Needs your input to continue.", error: false },
      });
    } else {
      segment.push(event);
    }
  }
  return segment;
}

/** Splits events at an attempt's own end: everything up to and including it is that attempt's, the rest carries on. */
function splitAtAttemptEnd(events: TaskEventView[], endedAt: string): [TaskEventView[], TaskEventView[]] {
  const cutoff = Date.parse(endedAt);
  if (Number.isNaN(cutoff)) return [[], events];
  const index = events.findIndex((event) => Date.parse(event.createdAt) > cutoff);
  return index === -1 ? [events, []] : [events.slice(0, index), events.slice(index)];
}

function bubbleFallback(kind: BubbleKind): string {
  if (kind === "resume") return "Sent a follow-up.";
  if (kind === "reply") return "Answered.";
  return "Sent an instruction.";
}

/** Reads a past attempt's reply off the attempt row itself, since that is the one place its curated answer lives. */
function attemptResolver(attempt: TaskAttempt, index: number): (segment: TaskEventView[]) => ResponseBlock | undefined {
  return (segment) => {
    if (attempt.error) {
      return {
        id: attemptResponseId(index),
        text: attempt.error,
        error: true,
        awaitingDecision: awaitsDecision(attempt.completion),
      };
    }
    if (attempt.output !== "") return { id: attemptResponseId(index), text: attempt.output, error: false };
    return messageResponse(segment);
  };
}

/** A run stopped on a permission or an authority the worker never held is waiting on a person, not broken. */
function awaitsDecision(completion: TaskCompletion | undefined): boolean {
  return completion?.code === "permission_denied" || completion?.code === "needs_authority";
}

/** Sentinel ids for past-attempt responses: negative and below `REQUEST_ID`, never a real event id. */
function attemptResponseId(index: number): number {
  return -1000 - index;
}

/** Closes a run that ended mid-stream — its response, if it left one, is read straight off its own events by default. */
function flushRun(
  segment: TaskEventView[],
  items: TranscriptItem[],
  task: Task,
  cache: WorkSegmentCache,
  resolveResponse: (segment: TaskEventView[]) => ResponseBlock | undefined = messageResponse,
): void {
  const response = resolveResponse(segment);
  if (segment.length === 0 && !response) return;
  if (segment.length > 0) pushWork(segment, items, false, false, undefined, task, cache);
  if (response) items.push({ type: "response", block: { ...response, marker: hasMarkedTurn(items) ? "response" : undefined } });
}

/**
 * Closes the run in progress — the one the task is still on, or the one it
 * last settled on. Its response, when there is one, is read off the task row
 * first, since that is the one place a curated final answer lives.
 */
function flushFinal(
  segment: TaskEventView[],
  items: TranscriptItem[],
  task: Task,
  cache: WorkSegmentCache,
): void {
  const live = !activityIsSettled(task.state);
  if (segment.length > 0) {
    pushWork(segment, items, live, true, undefined, task, cache);
  }
  // A task left on `needs_input` already ended the loop on a Question — the
  // segment past it is always empty, and nothing here should follow it.
  if (task.state === "needs_input") return;
  const response = finalResponse(task, segment, live);
  if (response) items.push({ type: "response", block: { ...response, marker: hasMarkedTurn(items) ? "response" : undefined } });
}

function pushWork(
  segment: TaskEventView[],
  items: TranscriptItem[],
  live: boolean,
  openLast: boolean,
  ending: ActivityEnding | undefined,
  task: Task,
  cache: WorkSegmentCache,
): void {
  const turns = splitByTurn(segment);
  turns.forEach((turn, index) => {
    const isLast = index === turns.length - 1;
    const id = turn[0]?.id ?? REQUEST_ID;
    const isLive = live && isLast;
    const startsExpanded = openLast && isLast;
    const turnEnding = isLast ? ending : undefined;
    const key = turnKey(turn, task.cwd, isLive, startsExpanded, turnEnding);
    items.push({
      type: "work",
      segment: cache.reuse(key, turn, () => ({
        id,
        composition: ActivityStory.composeWithState(turn, !isLive, turnEnding),
        cwd: task.cwd,
        live: isLive,
        startsExpanded,
        durationMs: segmentDurationMs(turn),
      })),
    });
  });
}

/** Everything a turn's segment is composed from, so a reused one cannot be stale. */
function turnKey(
  turn: TaskEventView[],
  cwd: string,
  live: boolean,
  startsExpanded: boolean,
  ending: ActivityEnding | undefined,
): string {
  const first = turn[0]?.id ?? REQUEST_ID;
  const last = turn[turn.length - 1]?.id ?? REQUEST_ID;
  const end = ending ? `${ending.state}:${ending.updatedAt}:${ending.error ?? ""}:${ending.reason ?? ""}:${ending.code ?? ""}` : "";
  return `${first}-${last}-${turn.length}-${cwd}-${live ? 1 : 0}-${startsExpanded ? 1 : 0}-${end}`;
}

/**
 * Splits a run's events at turn boundaries, so "Worked for..." reports one
 * turn's duration instead of the whole run's. Events without a turn id (a
 * provider that never sent one, or an older run recorded before turns were
 * tracked) stay in one chunk together — the same single "Worked for..." block
 * shown before turn splitting existed.
 */
function splitByTurn(events: TaskEventView[]): TaskEventView[][] {
  if (events.length === 0) return [events];
  const withTurns = deriveTurnIds(events);
  const chunks: TaskEventView[][] = [];
  let current: TaskEventView[] = [];
  let currentTurn: number | undefined;
  withTurns.forEach((event, index) => {
    if (current.length > 0 && event.turnId !== undefined && event.turnId !== currentTurn) {
      chunks.push(current);
      current = [];
    }
    current.push(events[index]);
    if (event.turnId !== undefined) currentTurn = event.turnId;
  });
  if (current.length > 0) chunks.push(current);
  return chunks;
}

function messageResponse(segment: TaskEventView[]): ResponseBlock | undefined {
  const event = ActivityStory.responseEvent(segment);
  if (!event) return undefined;
  const text = eventText(event);
  return text !== undefined ? { id: event.id, text, error: false } : undefined;
}

function finalResponse(task: Task, segment: TaskEventView[], live: boolean): ResponseBlock | undefined {
  const failure = normalizeAntigravityEvents(segment).find(
    (event) => event.source === "antigravity" && event.phase === "failed",
  );
  if (failure?.detail !== undefined && failure.detail !== "") {
    return { id: REQUEST_ID, text: failure.detail, error: true };
  }
  if (live || task.state === "blocked" || task.state === "failed") return undefined;
  if (task.error !== undefined && task.error !== "") {
    return { id: REQUEST_ID, text: task.error, error: true };
  }
  if (task.output !== "") {
    return { id: REQUEST_ID, text: task.output, error: false };
  }
  const response = messageResponse(segment);
  if (response) return response;
  return !live ? { id: REQUEST_ID, text: undefined, error: false } : undefined;
}

/** The title labels the event; it is not something the worker said. A response
    with no body is not a response. */
function eventText(event: TaskEventView): string | undefined {
  const text = event.presentation?.text ?? event.detail;
  return text !== undefined && text.trim() !== "" ? text : undefined;
}

function segmentDurationMs(events: TaskEventView[]): number | undefined {
  const first = events[0];
  const last = events[events.length - 1];
  if (!first || !last) return undefined;
  const from = Date.parse(first.createdAt);
  const to = Date.parse(last.createdAt);
  if (Number.isNaN(from) || Number.isNaN(to)) return undefined;
  const duration = to - from;
  return duration >= 0 ? duration : undefined;
}
