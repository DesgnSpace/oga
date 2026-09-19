// Pure activity composition for the task detail trace.
//
// The timeline is keyed by what the broker already knows about a run's shape:
// a turn id, a tool call id, a parent call id, an agent id. Nothing here reads
// a row's words to decide where it belongs, so a provider that renames a call
// halfway through ("Terminal" → the command it ran, "Preparing file…" → the
// file it wrote) still gets exactly one row for it.

import type { TaskEventPresentation, TaskEventView, TaskState } from "@/bridge/types";
import {
  ogaCall,
  ogaResultSummary,
  ogaSubject,
  ogaTitle,
  ogaVerb,
  type OgaObject,
} from "@/domain/oga";
import { absoluteTime } from "@/ui/time";

export interface ActivityComposition {
  blocks: ActivityBlock[];
  technical: TaskEventView[];
}

export interface ActivityEnding {
  taskId: string;
  state: TaskState;
  error?: string;
  reason?: string;
  code?: string;
  updatedAt: string;
}

/** A stretch of thinking with no work in between, folded into one row. */
export interface ReasoningPulse {
  id: number;
  /** Every block the provider let us read, in order. Absent when all were redacted. */
  text?: string;
  tokens?: number;
  seconds?: number;
}

/**
 * How a node ended. `interrupted` is a call the worker opened and never
 * closed on a turn that has since settled — it did not finish, and it is not
 * still running either.
 */
export type ActivityStatus = "running" | "needs_input" | "failed" | "interrupted" | "done";

/** The timeline's top level: the run's turns, plus what sits outside them. */
export type ActivityBlock =
  | { type: "turn"; turn: ActivityTurn }
  | { type: "receipt"; event: TaskEventView; thinkingTokens: number; usageWindow?: string }
  | { type: "handoff"; boundary: HandoffBoundary };

/** One round of work, keyed by the broker's turn id. */
export interface ActivityTurn {
  /** `turn:<ordinal>` — stable across rebuilds and never an event id. */
  id: string;
  turnId?: number;
  /** 0-based position among the composition's turns. */
  ordinal: number;
  /** The first thing the worker said this turn, as one line. */
  title?: string;
  /** The message `title` was read off, so a heading never repeats a row. */
  titleFrom?: number;
  segments: ActivitySegment[];
  events: TaskEventView[];
  status: ActivityStatus;
}

/** A stretch of work inside a turn, opened by the worker saying something. */
export interface ActivitySegment {
  /** `segment:<turn ordinal>:<index>`. */
  id: string;
  /** The words this stretch opened with, and the first of `nodes` when set. */
  lead?: TaskEventView;
  nodes: ActivityNode[];
}

export type ActivityNode =
  | { type: "call"; call: ActivityCall }
  | { type: "message"; id: string; event: TaskEventView }
  | { type: "thinking"; id: string; pulse: ReasoningPulse }
  | { type: "notice"; id: string; event: TaskEventView }
  | { type: "subagent"; subagent: ActivitySubagent };

/** One tool call, however many rows the provider sent for it. */
export interface ActivityCall {
  /** `call:<turn ordinal>:<action id>`. */
  id: string;
  actionId: string;
  /** The call as it last described itself. */
  event: TaskEventView;
  /** Every row that carried this call's id, in arrival order. */
  events: TaskEventView[];
  /** Calls the provider reported as this one's children. */
  children: ActivityCall[];
  status: ActivityStatus;
  startedAt: string;
  endedAt?: string;
}

/** A subagent run, bracketed by the pair of rows that share its agent id. */
export interface ActivitySubagent {
  /** `subagent:<agent id>`. */
  id: string;
  label: string;
  start: TaskEventView;
  nodes: ActivityNode[];
  status: ActivityStatus;
}

/** The rows a node stands for — what a reader would count as its activity. */
function nodeEvents(node: ActivityNode): TaskEventView[] {
  switch (node.type) {
    case "call":
      return callEvents(node.call);
    case "message":
    case "notice":
      return [node.event];
    case "thinking":
      return [];
    case "subagent":
      return [node.subagent.start, ...node.subagent.nodes.flatMap(nodeEvents)];
  }
}

function callEvents(call: ActivityCall): TaskEventView[] {
  return [call.event, ...call.children.flatMap(callEvents)];
}

/** Every call in a turn, including the ones nested under another call. */
function turnCalls(turn: ActivityTurn): ActivityCall[] {
  const withChildren = (call: ActivityCall): ActivityCall[] => [call, ...call.children.flatMap(withChildren)];
  const collect = (nodes: ActivityNode[]): ActivityCall[] =>
    nodes.flatMap((node) => {
      if (node.type === "call") return withChildren(node.call);
      if (node.type === "subagent") return collect(node.subagent.nodes);
      return [];
    });
  return turn.segments.flatMap((segment) => collect(segment.nodes));
}

/** Every call in a composition, in the order the worker opened them. */
export function compositionCalls(blocks: ActivityBlock[]): ActivityCall[] {
  return blocks.flatMap((block) => (block.type === "turn" ? turnCalls(block.turn) : []));
}

/** How much work a stretch holds: the tool calls a reader would have counted. */
export function nodesCallCount(nodes: ActivityNode[]): number {
  const countCall = (call: ActivityCall): number => 1 + call.children.reduce((sum, child) => sum + countCall(child), 0);
  return nodes.reduce((count, node) => {
    if (node.type === "call") return count + countCall(node.call);
    if (node.type === "subagent") return count + nodesCallCount(node.subagent.nodes);
    return count;
  }, 0);
}

export function nodesDurationMs(nodes: ActivityNode[]): number | undefined {
  const events = nodes.flatMap(nodeEvents);
  return spanMs(events[0]?.createdAt, events[events.length - 1]?.createdAt);
}

export function turnDurationMs(turn: ActivityTurn): number | undefined {
  return spanMs(turn.events[0]?.createdAt, turn.events[turn.events.length - 1]?.createdAt);
}

export function callDurationMs(call: ActivityCall): number | undefined {
  return spanMs(call.startedAt, call.endedAt);
}

function spanMs(start: string | undefined, end: string | undefined): number | undefined {
  if (start === undefined || end === undefined) return undefined;
  const from = eventTime(start);
  const to = eventTime(end);
  if (from === undefined || to === undefined) return undefined;
  return to - from > 0 ? to - from : undefined;
}

export type HandoffBriefTier = "verbatim" | "digest";

export interface HandoffBoundary {
  chain: string;
  earlierRuns: HandoffRun[];
  hiddenEventCount: number;
  briefTier?: HandoffBriefTier;
}

export interface HandoffRun {
  endedBy?: Hop;
  blocks: ActivityBlock[];
}

export interface Hop {
  fromId: string;
  toId: string;
  fromLabel: string;
  toLabel: string;
  display: string;
  context?: string;
}

function hopFromDetail(detail: string | undefined): Hop {
  if (detail === undefined) return hopUnknown("Unknown worker");
  const separator = " → ";
  const index = detail.indexOf(separator);
  if (index === -1) return hopUnknown(detail);
  const route = detail.slice(0, index);
  const toAndContext = detail.slice(index + separator.length);
  const contextSeparator = " · ";
  const contextIndex = toAndContext.indexOf(contextSeparator);
  const to = contextIndex === -1 ? toAndContext : toAndContext.slice(0, contextIndex);
  const context = contextIndex === -1 ? undefined : toAndContext.slice(contextIndex + contextSeparator.length);
  const from = route;
  const fromLabel = providerLabel(from);
  const toLabel = providerLabel(to);
  return { fromId: from, toId: to, fromLabel, toLabel, display: `${fromLabel} → ${toLabel}`, context };
}

function hopUnknown(value: string): Hop {
  return { fromId: value, toId: value, fromLabel: value, toLabel: value, display: value };
}

function hopChain(hops: Hop[]): string {
  const first = hops[0];
  if (!first) return "";
  const labels = [first.fromLabel, ...hops.map((hop) => hop.toLabel)];
  const route = labels.join(" → ");
  const context = hops.map((hop) => hop.context).filter((value): value is string => value !== undefined);
  return context.length > 0 ? `${route} · ${[...new Set(context)].join(", ")}` : route;
}

export const ActivityStory = {
  compose(events: TaskEventView[]): ActivityComposition {
    return composeWithState(events, false, undefined, true);
  },
  composeWithState,
  responseEvent(events: TaskEventView[]): TaskEventView | undefined {
    return [...normalizeAntigravityEvents(events)].reverse().find((event) => event.kind === "message" && event.minor !== true);
  },
  isTechnical,
};

/**
 * Holds the composition a segment last produced, so a rebuild that saw no new
 * activity hands back the same object instead of walking the run again.
 */
export class ActivityStoryProjection {
  private events: TaskEventView[] | undefined;
  private composition: ActivityComposition | undefined;
  private key = "";
  private reused = 0;
  private composed = 0;

  /** Updates served straight from the held composition. */
  get incrementalCount(): number {
    return this.reused;
  }

  /** Updates that had to walk the run again. */
  get fallbackCount(): number {
    return this.composed;
  }

  update(
    events: TaskEventView[],
    settled: boolean,
    ending: ActivityEnding | undefined,
    showReceipt = settled,
  ): ActivityComposition {
    const key = `${settled}|${showReceipt}|${ending === undefined ? "" : JSON.stringify(ending)}`;
    if (this.composition !== undefined && this.key === key && sameEvents(this.events, events)) {
      this.reused += 1;
      return this.composition;
    }
    this.composed += 1;
    this.composition = ActivityStory.composeWithState(events, settled, ending, showReceipt);
    // A caller appends to the array it handed over, so the held copy is what
    // makes "nothing new arrived" answerable at all.
    this.events = events.slice();
    this.key = key;
    return this.composition;
  }
}

function sameEvents(left: TaskEventView[] | undefined, right: TaskEventView[]): boolean {
  if (left === undefined || left.length !== right.length) return false;
  return left.every((event, index) => event === right[index]);
}

function composeWithState(
  rawEvents: TaskEventView[],
  settled: boolean,
  ending: ActivityEnding | undefined,
  showReceipt = settled,
): ActivityComposition {
  const events = deriveTurnIds(withoutDuplicateHookCalls(normalizeAntigravityEvents(rawEvents)));
  const boundaries = events
    .map((event, index) => [index, event] as const)
    .filter(([, event]) => event.type === "handed_off");
  if (boundaries.length === 0) {
    return composeTimeline(events, settled, ending, showReceipt);
  }

  const hops = boundaries.map(([, event]) => hopFromDetail(event.detail));
  const lastBoundary = boundaries[boundaries.length - 1];
  const briefTier = events
    .slice(lastBoundary[0] + 1)
    .map(handoffBriefTier)
    .find((tier): tier is HandoffBriefTier => tier !== undefined);
  const segments: TaskEventView[][] = [];
  let start = 0;
  for (const [index] of boundaries) {
    segments.push(events.slice(start, index));
    start = index + 1;
  }
  segments.push(events.slice(start));
  const composed = segments.map((segment, index) =>
    composeTimeline(
      segment,
      index + 1 !== segments.length || settled,
      index + 1 === segments.length ? ending : undefined,
      showReceipt,
    ),
  );
  const earlierRuns: HandoffRun[] = composed
    .slice(0, Math.max(composed.length - 1, 0))
    .map((composition, index) => ({ endedBy: hops[index], blocks: composition.blocks }));
  const hiddenEventCount = segments
    .slice(0, Math.max(segments.length - 1, 0))
    .reduce((sum, segment) => sum + segment.length, 0);
  const blocks: ActivityBlock[] = [
    { type: "handoff", boundary: { chain: hopChain(hops), earlierRuns, hiddenEventCount, briefTier } },
  ];
  const current = composed[composed.length - 1];
  if (current) blocks.push(...current.blocks);
  return {
    blocks,
    technical: composed.flatMap((composition) => composition.technical),
  };
}

// ---------------------------------------------------------------------------
// Building the timeline
// ---------------------------------------------------------------------------

interface TurnDraft {
  turnId?: number;
  ordinal: number;
  segments: SegmentDraft[];
  events: TaskEventView[];
}

interface SegmentDraft {
  lead?: TaskEventView;
  nodes: ActivityNode[];
}

function composeTimeline(
  rawEvents: TaskEventView[],
  settled: boolean,
  ending: ActivityEnding | undefined,
  showReceipt: boolean,
): ActivityComposition {
  const events = mergeSkipNotices(
    terminalOutcome(withoutRedundantTurnFailure(rawEvents.map(retryMessage)), ending),
  );
  const usageWindow = usageWindowSummary(events);
  const receiptId = showReceipt ? findLast(events, (event) => event.kind === "usage")?.id : undefined;
  const responseId = settled ? closingMessageId(events) : undefined;
  const calls = foldCalls(events);

  const technical: TaskEventView[] = [];
  const drafts: TurnDraft[] = [];
  let current: TurnDraft | undefined;
  let receipt: TaskEventView | undefined;
  let thinkingTokens = 0;
  let pulseTokens = 0;
  let pulse: TaskEventView[] = [];

  const turnFor = (event: TaskEventView): TurnDraft => {
    if (current !== undefined && (event.turnId === undefined || event.turnId === current.turnId)) {
      return current;
    }
    current = { turnId: event.turnId, ordinal: drafts.length, segments: [], events: [] };
    drafts.push(current);
    return current;
  };
  const place = (event: TaskEventView, node: ActivityNode) => {
    const turn = turnFor(event);
    const segment = turn.segments[turn.segments.length - 1];
    if (segment === undefined) turn.segments.push({ nodes: [node] });
    else segment.nodes.push(node);
  };
  const flushPulse = () => {
    const first = pulse[0];
    if (!first) return;
    const last = pulse[pulse.length - 1];
    const text = mergeReasoning(pulse);
    const tokens = pulseTokens;
    pulse = [];
    pulseTokens = 0;
    place(first, {
      type: "thinking",
      id: `thinking:${first.id}`,
      pulse: {
        id: first.id,
        text,
        tokens: tokens > 0 ? tokens : undefined,
        seconds: eventDurationSeconds(first.createdAt, last.createdAt),
      },
    });
  };

  for (const event of events) {
    const tokens = event.presentation?.tokensThinking ?? 0;
    thinkingTokens += tokens;
    pulseTokens += tokens;
    if (event.id === receiptId) {
      flushPulse();
      receipt = event;
      continue;
    }
    const fold = calls.byEvent.get(event.id);
    if (fold !== undefined && fold.opener !== event.id) continue;
    if (isTechnical(fold?.row ?? event)) {
      technical.push(fold?.row ?? event);
      continue;
    }
    if (isThinkingPulse(event)) {
      pulse.push(event);
      continue;
    }
    flushPulse();
    if (event.id === responseId) continue;

    if (fold !== undefined) {
      turnFor(event).events.push(event);
      if (calls.nested.has(fold.key)) continue;
      place(event, { type: "call", call: builtCall(fold, calls) });
      continue;
    }

    const turn = turnFor(event);
    turn.events.push(event);
    if (event.kind === "message") {
      turn.segments.push({ lead: event, nodes: [{ type: "message", id: `message:${event.id}`, event }] });
      continue;
    }
    place(event, { type: "notice", id: `notice:${event.id}`, event });
  }
  flushPulse();

  const blocks: ActivityBlock[] = drafts
    .map((draft, index) => finishTurn(draft, settled || index + 1 < drafts.length))
    .filter((turn): turn is ActivityTurn => turn !== undefined)
    .map((turn) => ({ type: "turn" as const, turn }));
  if (receipt) blocks.push({ type: "receipt", event: receipt, thinkingTokens, usageWindow });
  return { blocks, technical };
}

/**
 * Closes a turn: a subagent's stretch moves under the row that started it,
 * and once the turn itself has closed, a call it never ended reads as
 * interrupted rather than as work still in flight.
 */
function finishTurn(draft: TurnDraft, closed: boolean): ActivityTurn | undefined {
  const segments = draft.segments
    .map((segment, index) => ({
      id: `segment:${draft.ordinal}:${index}`,
      lead: segment.lead,
      nodes: closed ? markInterrupted(nestSubagents(segment.nodes)) : nestSubagents(segment.nodes),
    }))
    .filter((segment) => segment.nodes.length > 0);
  if (segments.length === 0) return undefined;
  const named = turnTitle(segments);
  return {
    id: `turn:${draft.ordinal}`,
    turnId: draft.turnId,
    ordinal: draft.ordinal,
    title: named?.title,
    titleFrom: named?.id,
    segments,
    events: draft.events,
    status: turnStatus(segments, closed),
  };
}

/** A turn is named by the first thing the worker said while running it. */
function turnTitle(segments: ActivitySegment[]): { title: string; id: number } | undefined {
  for (const segment of segments) {
    for (const node of segment.nodes) {
      if (node.type !== "message") continue;
      const title = titleFromMessage(node.event);
      if (title !== undefined) return { title, id: node.event.id };
    }
  }
  return undefined;
}

function titleFromMessage(event: TaskEventView): string | undefined {
  const text = eventText(event);
  if (text === undefined) return undefined;
  const line = (text.split("\n").find((value) => value.trim() !== "") ?? "").trim();
  return line === "" ? undefined : clip(line, 80);
}

function turnStatus(segments: ActivitySegment[], closed: boolean): ActivityStatus {
  const flatten = (nodes: ActivityNode[]): ActivityNode[] =>
    nodes.flatMap((node) => (node.type === "subagent" ? [node, ...flatten(node.subagent.nodes)] : [node]));
  const nodes = flatten(segments.flatMap((segment) => segment.nodes));
  if (nodes.some((node) => node.type === "notice" && node.event.type === "needs_input")) return "needs_input";
  if (nodes.some(nodeFailed)) return "failed";
  if (!closed && nodes.some(nodeRunning)) return "running";
  return "done";
}

function nodeFailed(node: ActivityNode): boolean {
  if (node.type === "call") return node.call.status === "failed";
  if (node.type === "notice") return node.event.phase === "failed";
  return false;
}

function nodeRunning(node: ActivityNode): boolean {
  return node.type === "call" && node.call.status === "running";
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

/** The agent's own record of a call, on a turn that ran over ACP. */
const AGENT_CALL_TYPES = new Set(["agent.tool_call", "agent.tool_call_update"]);

const CALL_KINDS = new Set<TaskEventView["kind"]>(["tool", "command", "file"]);

/** A heartbeat that only says a call is still going; its row already says so. */
const TOOL_PROGRESS_TYPE = "agent.tool_progress";

interface CallFold {
  key: string;
  actionId: string;
  turnId?: number;
  /** The id of the row that opened this call. */
  opener: number;
  row: TaskEventView;
  events: TaskEventView[];
}

interface CallFolds {
  /** Every row that carried a call id, pointed at the call it belongs to. */
  byEvent: Map<number, CallFold>;
  /** The calls a delegating call reported as its own, by the parent's key. */
  childrenOf: Map<string, CallFold[]>;
  /** The keys of calls that hang under another call rather than on their own. */
  nested: Set<string>;
}

function callId(event: TaskEventView): string | undefined {
  return event.actionId === undefined || event.actionId === "" ? undefined : event.actionId;
}

/**
 * Whether a row belongs to a call already open. A provider posts some rows
 * before it has told us which turn it is on, and an unknown turn is not a
 * different turn — it is the same call still reporting.
 */
function sameCall(fold: CallFold, event: TaskEventView): boolean {
  return fold.turnId === undefined || event.turnId === undefined || fold.turnId === event.turnId;
}

/**
 * Folds every row a provider sent for one tool call into a single record,
 * keyed by that call's own id. A resumed session replays the whole prior
 * transcript, so a call that already finished arrives again as a fresh start
 * under its original id; that replay lands back on the record it already has
 * rather than opening a second one.
 */
function foldCalls(events: TaskEventView[]): CallFolds {
  const byEvent = new Map<number, CallFold>();
  const byAction = new Map<string, CallFold>();
  const opened: CallFold[] = [];
  for (const event of events) {
    const actionId = callId(event);
    if (actionId === undefined) continue;
    const fold = byAction.get(actionId);
    if (fold !== undefined && sameCall(fold, event) && !reopens(fold.row, event)) {
      fold.events.push(event);
      fold.turnId = fold.turnId ?? event.turnId;
      byEvent.set(event.id, fold);
      if (!replays(fold.row, event)) fold.row = settleAction(fold.row, event);
      continue;
    }
    // A heartbeat only says a call is still going; it never opens one.
    if (event.type === TOOL_PROGRESS_TYPE) continue;
    const call: CallFold = {
      key: `${actionId}#${event.id}`,
      actionId,
      turnId: event.turnId,
      opener: event.id,
      row: event,
      events: [event],
    };
    byAction.set(actionId, call);
    byEvent.set(event.id, call);
    opened.push(call);
  }

  const childrenOf = new Map<string, CallFold[]>();
  const nested = new Set<string>();
  const first = new Map<string, CallFold>();
  for (const call of opened) if (!first.has(call.actionId)) first.set(call.actionId, call);
  for (const call of opened) {
    const parent = parentActionId(call.row);
    const owner = parent === undefined ? undefined : first.get(parent);
    if (owner === undefined || owner === call) continue;
    childrenOf.set(owner.key, [...(childrenOf.get(owner.key) ?? []), call]);
    nested.add(call.key);
  }
  return { byEvent, childrenOf, nested };
}

function builtCall(fold: CallFold, folds: CallFolds): ActivityCall {
  const last = fold.events[fold.events.length - 1] ?? fold.row;
  const terminal = fold.row.phase === "completed" || fold.row.phase === "failed";
  return {
    id: `call:${fold.turnId ?? 0}:${fold.actionId}`,
    actionId: fold.actionId,
    event: fold.row,
    events: fold.events,
    children: (folds.childrenOf.get(fold.key) ?? []).map((child) => builtCall(child, folds)),
    status: fold.row.phase === "failed" ? "failed" : terminal ? "done" : "running",
    startedAt: fold.events[0]?.createdAt ?? fold.row.createdAt,
    endedAt: terminal ? last.createdAt : undefined,
  };
}

/**
 * A call that has not reported a terminal phase on a turn that already closed
 * never finished; it did not keep running either.
 */
function markInterrupted(nodes: ActivityNode[]): ActivityNode[] {
  const call = (value: ActivityCall): ActivityCall => ({
    ...value,
    status: value.status === "running" ? "interrupted" : value.status,
    children: value.children.map(call),
  });
  return nodes.map((node) => {
    if (node.type === "subagent") {
      const status = node.subagent.status === "running" ? "interrupted" : node.subagent.status;
      return { ...node, subagent: { ...node.subagent, status, nodes: markInterrupted(node.subagent.nodes) } };
    }
    return node.type === "call" ? { type: "call", call: call(node.call) } : node;
  });
}

// ---------------------------------------------------------------------------
// Subagents
// ---------------------------------------------------------------------------

export const SUBAGENT_STARTED_TITLE = "Subagent started";
export const SUBAGENT_FINISHED_TITLE = "Subagent finished";

/**
 * Subagent runs nest by bracketing: a start opens a group, its stop closes it,
 * and a start that arrives while another group is still open sits inside it.
 * A row that names the run it belongs to (a hook `agent_id`, or a call whose
 * parent is the launching call) goes there even when the groups interleave;
 * any other row goes to the innermost open group. A start that never stops
 * closes with everything that followed it, still marked as running.
 */
function nestSubagents(nodes: ActivityNode[]): ActivityNode[] {
  interface Frame {
    id: string;
    start: TaskEventView;
    parent: Frame | undefined;
    nodes: ActivityNode[];
    belongs: (event: TaskEventView) => boolean;
    done: boolean;
  }
  const root: ActivityNode[] = [];
  const open: Frame[] = [];
  const wrap = (frame: Frame): ActivityNode => ({
    type: "subagent",
    subagent: {
      id: `subagent:${frame.id}`,
      label: subagentLabel(frame.start),
      start: frame.start,
      nodes: frame.nodes,
      status: frame.done ? "done" : "running",
    },
  });
  const close = (frame: Frame) => {
    const index = open.indexOf(frame);
    open.splice(index, 1);
    for (const inner of open.slice(index)) {
      if (inner.parent === frame) inner.parent = frame.parent;
    }
    (frame.parent && open.includes(frame.parent) ? frame.parent.nodes : root).push(wrap(frame));
  };

  for (const node of nodes) {
    const boundary = node.type === "notice" ? subagentBoundary(node.event) : undefined;
    if (node.type === "notice" && boundary?.role === "start" && !open.some((frame) => frame.id === boundary.id)) {
      open.push({
        id: boundary.id,
        start: node.event,
        parent: open.at(-1),
        nodes: [],
        belongs: subagentMembership(node.event, boundary.id),
        done: false,
      });
      continue;
    }
    if (boundary?.role === "stop") {
      const frame = innermost(open, (candidate) => candidate.id === boundary.id);
      if (frame !== undefined) {
        frame.nodes.push(node);
        frame.done = true;
        close(frame);
        continue;
      }
    }
    const events = nodeEvents(node);
    const owner = innermost(open, (frame) => events.some(frame.belongs)) ?? open.at(-1);
    (owner?.nodes ?? root).push(node);
  }
  while (open.length > 0) close(open[open.length - 1]);
  return root;
}

function innermost<T>(frames: T[], matches: (frame: T) => boolean): T | undefined {
  for (let index = frames.length - 1; index >= 0; index--) {
    if (matches(frames[index])) return frames[index];
  }
  return undefined;
}

function subagentBoundary(event: TaskEventView): { id: string; role: "start" | "stop" } | undefined {
  if (event.title === "SubagentStart" || event.title === "SubagentStop" || event.title === "Spawned subagent") {
    const id = rawString(event, "agent_id");
    if (id === undefined) return undefined;
    return { id, role: event.title === "SubagentStop" ? "stop" : "start" };
  }
  if (event.title === SUBAGENT_STARTED_TITLE || event.title === SUBAGENT_FINISHED_TITLE) {
    // The streamed pair shares `task_id`; the hook pair shares `agent_id`
    // under the same normalized titles since Rust aligns them.
    const id = rawString(event, "task_id") ?? rawString(event, "agent_id");
    return id === undefined ? undefined : { id, role: event.title === SUBAGENT_STARTED_TITLE ? "start" : "stop" };
  }
  return undefined;
}

/**
 * A hook pair stamps every row it owns with the same `agent_id`. A streamed
 * pair does not: it names the tool call that launched the subagent, and the
 * subagent's own calls point back at it as their parent.
 */
function subagentMembership(start: TaskEventView, id: string): (event: TaskEventView) => boolean {
  if (rawString(start, "agent_id") !== undefined) {
    return (event) => rawString(event, "agent_id") === id;
  }
  const toolUseId = rawString(start, "tool_use_id");
  return (event) => toolUseId !== undefined && parentActionId(event) === toolUseId;
}

function subagentLabel(event: TaskEventView): string {
  const name = rawString(event, "agent_type") ?? (event.title === SUBAGENT_STARTED_TITLE ? event.detail : undefined);
  return name !== undefined && name !== "" ? `Subagent · ${name}` : "Subagent";
}

// ---------------------------------------------------------------------------
// What belongs in the story, and what is bookkeeping
// ---------------------------------------------------------------------------

export const ACTIVITY_SKIPPED_TITLE = "Some activity was not recorded";
/** Every provider's reasoning blocks share this title; it is how they fold. */
export const REASONING_TITLE = "Thinking";
const USAGE_WINDOW_TITLE = "Usage window";

/** Broker rows that record how a run was managed, not what it did. */
const BOOKKEEPING_TYPES = new Set([
  "created",
  "queued",
  "started",
  "worker_spawned",
  "completed",
  "archived",
  "unarchived",
  "session_captured",
  "session_reused",
  "worker_stderr",
  "heartbeat",
  "handoff_brief",
]);

/** The broker's own record of output it could not keep. */
const SKIPPED_TYPES = new Set(["line_dropped", "event_dropped", "events_truncated"]);

function isTechnical(event: TaskEventView): boolean {
  if (event.minor === true) return true;
  if (BOOKKEEPING_TYPES.has(event.type) || event.type === TOOL_PROGRESS_TYPE) return true;
  // A raw payload has no words a reader can use; it stays behind the toggle.
  if (event.kind === "raw") return true;
  if (event.kind === "usage") return true;
  // A usage-window notice earns its place only when it says what was limited
  // and for how long; the bare marker repeats without telling a reader
  // anything.
  if (event.title === USAGE_WINDOW_TITLE && (event.detail === undefined || event.detail === event.title)) {
    return true;
  }
  return event.kind === "tool" && event.detail === undefined && event.presentation === undefined;
}

function handoffBriefTier(event: TaskEventView): HandoffBriefTier | undefined {
  if (event.type !== "handoff_brief") return undefined;
  const tier = event.detail?.split(/\s+/, 1)[0];
  return tier === "verbatim" || tier === "digest" ? tier : undefined;
}

function isThinkingPulse(event: TaskEventView): boolean {
  return event.kind === "reasoning" && event.title === REASONING_TITLE;
}

/**
 * The readable part of one stretch of thinking.
 *
 * Providers that stream reasoning resend the whole block on every update, so a
 * block that starts with the one before it replaces that one instead of piling
 * up beside it. Blocks the provider redacted contribute nothing here — they
 * still lengthen the stretch, they just cannot be read.
 */
function mergeReasoning(events: TaskEventView[]): string | undefined {
  const blocks: string[] = [];
  for (const event of events) {
    const text = event.detail?.trim();
    if (text === undefined || text === "") continue;
    const previous = blocks[blocks.length - 1];
    if (previous !== undefined && text.startsWith(previous)) blocks[blocks.length - 1] = text;
    else blocks.push(text);
  }
  return blocks.length > 0 ? blocks.join("\n\n") : undefined;
}

/** Whether anything in this run is worth turning "Show thinking" on for. */
export function compositionHasThinking(composition: ActivityComposition): boolean {
  const inNodes = (nodes: ActivityNode[]): boolean =>
    nodes.some((node) => (node.type === "subagent" ? inNodes(node.subagent.nodes) : node.type === "thinking"));
  return composition.blocks.some(
    (block) => block.type === "turn" && block.turn.segments.some((segment) => inNodes(segment.nodes)),
  );
}

/**
 * Output goes missing a line at a time, so a lossy run can raise the notice
 * dozens of times. A reader needs to know once, with the total.
 */
function mergeSkipNotices(events: TaskEventView[]): TaskEventView[] {
  const counts = new Map<string, number>();
  let first: number | undefined;
  events.forEach((event, index) => {
    if (!SKIPPED_TYPES.has(event.type)) return;
    if (first === undefined) first = index;
    const skipped = parseSkipped(event.detail);
    if (skipped) counts.set(skipped.noun, (counts.get(skipped.noun) ?? 0) + skipped.count);
  });
  if (first === undefined) return events;

  const totals = [...counts]
    .map(([noun, count]) => `${count} ${noun}${count === 1 ? "" : "s"} skipped`)
    .join(" · ");
  const text = totals === "" ? ACTIVITY_SKIPPED_TITLE : `${ACTIVITY_SKIPPED_TITLE} — ${totals}`;
  return events.flatMap((event, index) => {
    if (!SKIPPED_TYPES.has(event.type)) return [event];
    if (index !== first) return [];
    return [{
      ...event,
      detail: totals === "" ? undefined : totals,
      presentation: { type: "signal" as const, text, level: "warning" as const },
    }];
  });
}

function parseSkipped(detail: string | undefined): { count: number; noun: string } | undefined {
  const match = detail?.match(/^(\d+) (line|event)s? skipped$/);
  return match ? { count: Number(match[1]), noun: match[2] } : undefined;
}

/**
 * The window a run is closest to filling, read off the last notice the provider
 * sent, so the receipt says how much headroom is left before the next run.
 */
function usageWindowSummary(events: TaskEventView[]): string | undefined {
  const notice = findLast(events, (event) => event.title === USAGE_WINDOW_TITLE);
  const windows = rawObject(rawObject(rawObject(notice && rawValue(notice))?.rate_limit_info)?.unifiedWindows);
  if (!windows) return undefined;
  let busiest: { utilization: number; resetsAt?: number } | undefined;
  for (const value of Object.values(windows)) {
    const window = rawObject(value);
    const utilization = window?.utilization;
    if (typeof utilization !== "number") continue;
    if (busiest && busiest.utilization >= utilization) continue;
    const resetsAt = window?.resetsAt;
    busiest = { utilization, resetsAt: typeof resetsAt === "number" ? resetsAt : undefined };
  }
  if (!busiest) return undefined;
  const percent = `${USAGE_WINDOW_TITLE} ${Math.round(busiest.utilization * 100)}%`;
  const resets = busiest.resetsAt === undefined ? undefined : absoluteTime(new Date(busiest.resetsAt * 1000));
  return resets === undefined ? percent : `${percent} · resets ${resets}`;
}

function closingMessageId(events: TaskEventView[]): number | undefined {
  const index = findLastIndex(
    events,
    (event) => event.kind === "message" && event.minor !== true && eventText(event) !== undefined,
  );
  if (index === -1) return undefined;
  const rest = events.slice(index + 1);
  // A run tidies up after it speaks — session bookkeeping, the worker's own
  // logging, the row that closes the task — and none of that makes the words
  // before it something other than the closing answer. A failure does, and
  // keeps the answer in the trace where it happened.
  return rest.every((event) => isTechnical(event) || event.kind === "usage" || event.kind === "lifecycle")
    ? events[index].id
    : undefined;
}

// ---------------------------------------------------------------------------
// Normalizing what providers send
// ---------------------------------------------------------------------------

/**
 * Drops the hook copies of calls the worker already reported itself.
 *
 * A worker whose hooks post to Oga reports every tool call twice on a turn
 * that also streams its own: once as a hook, once as the agent's own row. The
 * agent's row is the fuller one — it carries the diff, the files, and the
 * agent's own words for the call — so the hook copy goes. Hooks that are not
 * about a call at all say something no other row does and stay.
 */
export function withoutDuplicateHookCalls(events: TaskEventView[]): TaskEventView[] {
  const agentTurns = new Set<number | undefined>();
  const agentCalls = new Set<string>();
  for (const event of events) {
    if (!AGENT_CALL_TYPES.has(event.type)) continue;
    agentTurns.add(event.turnId);
    if (event.actionId !== undefined) agentCalls.add(`${event.turnId}:${event.actionId}`);
  }
  if (agentTurns.size === 0) return events;
  // A hook is posted without a turn, so it belongs to the turn the task was
  // last in, and only the agent's own copy of that same call replaces it.
  let turn: number | undefined;
  return events.filter((event) => {
    turn = event.turnId ?? turn;
    if (event.type !== "agent.hook" || !CALL_KINDS.has(event.kind)) return true;
    if (event.turnId !== undefined) return !agentTurns.has(event.turnId);
    return !agentCalls.has(`${turn}:${event.actionId}`);
  });
}

/** Converts Antigravity's raw step protocol into the same rows other providers use. */
export function normalizeAntigravityEvents(events: TaskEventView[]): TaskEventView[] {
  const normalized: TaskEventView[] = [];
  let response: OpenResponse | undefined;
  for (const event of events) {
    if (event.source !== "antigravity") {
      normalized.push(event);
      continue;
    }
    const payload = parseAntigravityPayload(event.rawText);
    const step = payload?.step_update;
    const stepType = stringValue(step?.step_type);
    if (payload?.event === "step_update" && step) {
      if (stepType === "agent_response") {
        response = streamResponse(normalized, response, event, step);
        continue;
      }
      if (stepType === "user_input" || stepType === "system_message") continue;
      if (stepType === "error_message") {
        const detail = stringValue(step.message) ?? stringValue(step.text) ?? stringValue(step.content) ?? event.detail;
        if (detail === undefined) continue;
        normalized.push({ ...event, kind: "error", phase: "failed", title: "Error", detail });
        continue;
      }
      if (stepType === "tool") {
        const info = step.tool_info;
        const name = stringValue(step.tool_name) ?? stringValue(step.toolName) ?? event.title;
        const parameters = info?.parameters ?? {};
        const output = stringValue(info?.output);
        const error = info?.error ?? step.error;
        const state = stringValue(step.state)?.toLowerCase();
        // SAFETY: Antigravity parameters are the parsed JSON object passed to the Oga call.
        const oga = ogaCall(name, parameters as OgaObject);
        const mapped = oga
          ? {
              kind: "tool" as const,
              detail: ogaSubject(oga.operation, oga.input),
              presentation: {
                type: "tool" as const,
                text: ogaSubject(oga.operation, oga.input),
                outcome: ogaResultSummary(oga.operation, output) ?? event.presentation?.outcome,
              },
            }
          : antigravityToolPresentation(name, parameters, output);
        const label = oga
          ? ogaTitle(oga.operation, oga.input)
          : name === "call_mcp_tool"
            ? [stringValue(parameters.ServerName), stringValue(parameters.ToolName)].filter(Boolean).join("/") || name
            : name;
        const result = stringValue(error?.message)
          ?? event.result
          ?? (oga ? ogaResultSummary(oga.operation, output) : output);
        normalized.push({
          ...event,
          kind: mapped?.kind ?? event.kind,
          title: label,
          verb: event.verb ?? (oga ? ogaVerb(oga.operation, oga.input, state !== "active") : event.verb),
          detail: mapped?.detail ?? (oga ? undefined : Object.keys(parameters).length > 0 ? JSON.stringify(parameters) : event.detail),
          result,
          presentation: mapped?.presentation ?? event.presentation,
          phase: state === "error" || state === "failed" ? "failed" : state === "done" || state === "completed" ? "completed" : "started",
          actionId: `antigravity:step:${String(step.step_index ?? event.id)}`,
        });
        continue;
      }
    }
    const result = payload?.result;
    if (payload?.event === "result" && stringValue(result?.status)?.toUpperCase() === "ERROR") {
      const reason = stringValue(result?.error) ?? event.detail;
      normalized.push({ ...event, kind: "error", phase: "failed", title: "Provider error", detail: reason, result: reason });
      continue;
    }
    normalized.push(event);
  }
  return normalized;
}

/** The message row a response step is being written into, by where it sits. */
interface OpenResponse {
  step: number | undefined;
  turnId: number | undefined;
  index: number;
}

/**
 * Writes one piece of a response step into its message row. A response streams
 * as pieces of one step, which read as one message; the step's finished report
 * adds its last piece and closes the row.
 */
function streamResponse(
  rows: TaskEventView[],
  open: OpenResponse | undefined,
  event: TaskEventView,
  step: AntigravityStep,
): OpenResponse | undefined {
  if (open !== undefined && open.step === step.step_index && open.turnId === event.turnId) {
    const row = rows[open.index];
    const text = `${row.presentation?.text ?? ""}${step.text_delta ?? ""}`;
    rows[open.index] = {
      ...row,
      detail: text,
      presentation: row.presentation && { ...row.presentation, text },
      complete: event.kind !== "message",
    };
    return open;
  }
  if (event.kind !== "message") return open;
  rows.push(event);
  return { step: step.step_index, turnId: event.turnId, index: rows.length - 1 };
}

function antigravityToolPresentation(
  name: string,
  parameters: AntigravityParameters,
  output: string | undefined,
): { kind: "file" | "tool" | "command"; detail?: string; presentation: TaskEventView["presentation"] } | undefined {
  const path = stringValue(parameters.AbsolutePath) ?? stringValue(parameters.DirectoryPath) ?? stringValue(parameters.TargetFile);
  if (["view_file", "list_dir"].includes(name)) {
    return { kind: "file", presentation: { type: "file", path }, detail: path };
  }
  if (["grep_search", "find_by_name"].includes(name)) {
    const query = stringValue(parameters.Query) ?? stringValue(parameters.Pattern);
    return { kind: "tool", presentation: { type: "tool", text: query }, detail: query };
  }
  if (["replace_file_content", "multi_replace", "write_to_file"].includes(name)) {
    return { kind: "file", presentation: { type: "file", path }, detail: path };
  }
  if (name === "run_command") {
    const command = stringValue(parameters.CommandLine);
    return { kind: "command", presentation: { type: "command", command }, detail: command ?? output };
  }
  return undefined;
}

interface AntigravityParameters {
  AbsolutePath?: string;
  DirectoryPath?: string;
  TargetFile?: string;
  Query?: string;
  SearchPath?: string;
  Pattern?: string;
  SearchDirectory?: string;
  CommandLine?: string;
  ServerName?: string;
  ToolName?: string;
  Arguments?: OgaObject;
}

interface AntigravityStep {
  step_type?: string;
  step_index?: number;
  state?: string;
  text_delta?: string;
  tool_name?: string;
  toolName?: string;
  tool_info?: { parameters?: AntigravityParameters; output?: string; error?: { message?: string } };
  message?: string;
  text?: string;
  content?: string;
  error?: { message?: string };
}

interface AntigravityPayload {
  event?: string;
  step_update?: AntigravityStep;
  result?: { status?: string; error?: string };
}

function parseAntigravityPayload(raw: string | undefined): AntigravityPayload | undefined {
  if (raw === undefined) return undefined;
  try {
    // SAFETY: provider payloads are decoded into this boundary shape before use.
    return JSON.parse(raw) as AntigravityPayload;
  } catch {
    return undefined;
  }
}

/**
 * An ACP update carries only the fields that changed, so one that names no
 * kind of its own is describing the call the row already holds. A page
 * boundary or a replay separates such an update from its opening row, which
 * leaves it looking like a call that never started — and opening a second row
 * for it would show the same work twice.
 */
function isPartialAgentUpdate(event: TaskEventView): boolean {
  return event.type === "agent.tool_call_update" && rawString(event, "kind") === undefined;
}

function reopens(row: TaskEventView, event: TaskEventView): boolean {
  if (isPartialAgentUpdate(event) || replays(row, event)) return false;
  return (row.phase === "completed" || row.phase === "failed") && event.phase === "started" && event.minor !== true;
}

/**
 * Resuming a session replays the whole prior transcript, so every tool call
 * that already finished arrives a second time as a fresh start under its
 * original id. It is the same call, and merging its replay back in would undo
 * the outcome the row already carries.
 */
function replays(row: TaskEventView, event: TaskEventView): boolean {
  return (
    (row.phase === "completed" || row.phase === "failed") &&
    event.phase === "started" &&
    row.title === event.title &&
    eventSubject(row) === eventSubject(event)
  );
}

function settleAction(first: TaskEventView, later: TaskEventView): TaskEventView {
  if (AGENT_CALL_TYPES.has(later.type) && rawString(later, "kind") === undefined) return settleAgentCall(first, later);
  if (later.minor === true) {
    const merged: TaskEventView = { ...first };
    const outcome = later.presentation?.outcome;
    if (outcome !== undefined && !restates(merged.detail, outcome) && merged.presentation?.outcome !== outcome) {
      if (merged.presentation) {
        merged.presentation = { ...merged.presentation, outcome };
      } else {
        merged.detail = [merged.detail, outcome].filter((value): value is string => value !== undefined).join(" · ");
      }
    }
    if (later.phase === "failed") merged.phase = "failed";
    if (
      later.type === TOOL_PROGRESS_TYPE &&
      merged.phase === "started" &&
      later.presentation?.durationMs !== undefined
    ) {
      const durationMs = later.presentation.durationMs;
      merged.presentation = merged.presentation
        ? { ...merged.presentation, durationMs }
        : { type: "tool", durationMs };
    }
    return merged;
  }
  if (isOutcomeOnly(later.presentation) && isSubject(first.presentation)) {
    const merged: TaskEventView = { ...first };
    const outcome = later.presentation?.outcome;
    if (outcome !== undefined && merged.presentation) {
      merged.presentation = { ...merged.presentation, outcome };
    }
    if (later.phase === "failed") {
      merged.phase = "failed";
    } else if (later.complete === true) {
      // A worker that reports only how a call ended still ended it: the row
      // keeps the subject it opened with and stops reading as work in flight.
      merged.phase = later.phase;
      merged.complete = true;
      if (later.verb !== undefined) merged.verb = later.verb;
    }
    return merged;
  }
  const merged: TaskEventView = { ...later, id: first.id, createdAt: first.createdAt };
  if (merged.detail === undefined) merged.detail = first.detail;
  if (merged.presentation === undefined) merged.presentation = first.presentation;
  if (merged.rawText === undefined) merged.rawText = first.rawText;
  if (merged.presentation?.outcome === undefined && first.presentation?.outcome !== undefined && merged.presentation) {
    merged.presentation = { ...merged.presentation, outcome: first.presentation.outcome };
  }
  if (first.phase === "failed") merged.phase = "failed";
  if ((merged.phase === "completed" || merged.phase === "failed") && merged.presentation) {
    merged.presentation = { ...merged.presentation, durationMs: undefined };
  }
  return merged;
}

/**
 * Whether an update brought the call's diff, output, or result along, or only
 * moved its status: a status-only update must not replace the payload the
 * row expands into.
 */
function carriesPayload(update: TaskEventView): boolean {
  return update.result !== undefined
    || update.presentation?.change !== undefined
    || update.presentation?.outcome !== undefined;
}

/** How an ACP call row reads once it finishes, by how it read while running. */
const SETTLED_AGENT_VERBS = new Map([
  ["Deleting", "Deleted"],
  ["Moving", "Moved"],
  ["Searching", "Searched"],
  ["Running", "Ran"],
  ["Fetching", "Fetched"],
  ["Changing", "Changed"],
  ["Using", "Used"],
]);

/**
 * An ACP update carries only the fields that changed, so one that does not
 * name the call's kind leaves the call what it already was: it moves the row
 * on and reports how it went, and the kind, file, and input stay.
 */
function settleAgentCall(first: TaskEventView, later: TaskEventView): TaskEventView {
  const verb = later.complete === true && first.verb !== undefined
    ? (SETTLED_AGENT_VERBS.get(first.verb) ?? first.verb)
    : first.verb;
  // An update that says nothing about how the call is going leaves it where it
  // was; a call that already finished never goes back to running.
  const settled = first.phase === "completed" || first.phase === "failed";
  const reverts = settled && later.phase === "started";
  return {
    ...first,
    phase: first.phase === "failed" || reverts ? first.phase : later.phase,
    complete: reverts ? first.complete : (later.complete ?? first.complete),
    verb,
    // A call that opened without naming itself takes the name its update
    // brought: an agent that runs a command often titles it only once it has
    // one to report.
    target: first.target ?? later.target,
    result: later.result ?? first.result,
    rawText: carriesPayload(later) ? (later.rawText ?? first.rawText) : first.rawText,
    presentation: first.presentation && {
      ...first.presentation,
      text: first.presentation.text ?? later.presentation?.text,
      command: first.presentation.command ?? later.presentation?.command,
      change: later.presentation?.change ?? first.presentation.change,
      outcome: later.presentation?.outcome ?? first.presentation.outcome,
    },
  };
}

function restates(detail: string | undefined, outcome: string): boolean {
  if (detail === undefined) return false;
  const stripped = outcome.startsWith("Error: ") ? outcome.slice("Error: ".length) : outcome;
  const core = trimChars(stripped, "… ");
  const opening = Array.from(core).slice(0, 40).join("");
  return core.length >= 16 && detail.includes(opening);
}

function trimChars(value: string, chars: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && chars.includes(value[start])) start++;
  while (end > start && chars.includes(value[end - 1])) end--;
  return value.slice(start, end);
}

function isOutcomeOnly(presentation: TaskEventPresentation | undefined): boolean {
  if (!presentation) return false;
  return (
    presentation.type === "tool" &&
    presentation.path === undefined &&
    presentation.change === undefined &&
    presentation.command === undefined &&
    presentation.status === undefined &&
    presentation.exitCode === undefined &&
    presentation.text === undefined &&
    presentation.completed === undefined &&
    presentation.total === undefined &&
    presentation.costUsd === undefined &&
    presentation.tokensIn === undefined &&
    presentation.tokensOut === undefined &&
    presentation.tokensCached === undefined &&
    presentation.tokensThinking === undefined &&
    presentation.turns === undefined &&
    presentation.durationMs === undefined &&
    presentation.level === undefined
  );
}

function isSubject(presentation: TaskEventPresentation | undefined): boolean {
  if (!presentation) return false;
  if (presentation.type === "file") return presentation.path !== undefined;
  if (presentation.type === "command") return presentation.command !== undefined;
  return false;
}

function retryMessage(event: TaskEventView): TaskEventView {
  if (event.title !== "API retry" && !event.title.startsWith("Auto Retry ")) return event;
  const info = retryInfoFromRaw(event.rawText);
  const ended =
    event.title.includes("End") ||
    event.title.includes("Succeeded") ||
    event.title.includes("Failed") ||
    event.title.includes("Exhausted");
  const failed =
    event.title.includes("Failed") || event.title.includes("Exhausted") || (ended && info.error !== undefined);
  const copy: TaskEventView = { ...event };
  copy.kind = "lifecycle";
  copy.phase = failed ? "failed" : "info";
  copy.title = failed ? "Retry failed" : ended ? "Retry succeeded" : "Retrying";
  copy.detail =
    event.title === "API retry"
      ? event.detail
      : ended && !failed
        ? undefined
        : (info.error ??
          info.reason ??
          (info.attempt > 0 && info.maxAttempts > 1 ? `Attempt ${info.attempt} of ${info.maxAttempts}` : undefined));
  copy.presentation = undefined;
  return copy;
}

interface RetryInfo {
  error?: string;
  reason?: string;
  attempt: number;
  maxAttempts: number;
}

function retryInfoFromRaw(raw: string | undefined): RetryInfo {
  const empty: RetryInfo = { attempt: 0, maxAttempts: 0 };
  if (!raw) return empty;
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return empty;
  }
  if (typeof value !== "object" || value === null) return empty;
  const object = value as Record<string, unknown>;
  const rawError = object.error;
  const error =
    stringValue(rawError) ??
    (typeof rawError === "object" && rawError !== null
      ? stringValue((rawError as Record<string, unknown>).message)
      : undefined);
  const reason = stringValue(object.reason) ?? stringValue(object.message);
  const attempt = typeof object.attempt === "number" ? object.attempt : 0;
  const maxAttempts =
    typeof object.max_attempts === "number"
      ? object.max_attempts
      : typeof object.max_retries === "number"
        ? object.max_retries
        : 0;
  return { error, reason, attempt, maxAttempts };
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" && value !== "" ? value : undefined;
}

function withoutRedundantTurnFailure(events: TaskEventView[]): TaskEventView[] {
  const hasAgentError = events.some((event) => event.title === "Agent error");
  const hasTaskFailed = events.some((event) => event.title === "Task failed");
  if (hasAgentError && hasTaskFailed) {
    return events.filter((event) => event.title !== "Turn Failed");
  }
  return events;
}

function terminalOutcome(events: TaskEventView[], ending: ActivityEnding | undefined): TaskEventView[] {
  if (!ending || !(["failed", "blocked", "cancelled"] as TaskState[]).includes(ending.state)) return events;
  let brokerTitle: string;
  let title: string;
  let action: string;
  if (ending.state === "blocked") {
    brokerTitle = "Task blocked";
    title = "Run needs attention";
    action = "Resolve the issue, then resume this task.";
  } else if (ending.state === "cancelled") {
    brokerTitle = "Task cancelled";
    title = "Run cancelled";
    action = "Resume this task to continue.";
  } else {
    brokerTitle = "Task failed";
    title = "Run stopped before finishing";
    action = "Resume this task to continue.";
  }
  const isOutcome = (event: TaskEventView) => event.source === "broker" && event.title === brokerTitle;
  const found = findLast(events, isOutcome);
  const outcome: TaskEventView = found
    ? { ...found }
    : {
        id: (events.length > 0 ? Math.max(...events.map((event) => event.id)) : 0) + 1,
        taskId: ending.taskId,
        source: "broker",
        type: "broker.terminal",
        kind: ending.state === "failed" ? "error" : "lifecycle",
        phase: "failed",
        title: brokerTitle,
        detail: ending.error,
        createdAt: ending.updatedAt,
      };
  const reason = [ending.error, ending.reason, outcome.detail]
    .filter((value): value is string => value !== undefined)
    .find((value) => value.trim() !== "");
  let detail = reason ?? "";
  if (detail !== "") detail += "\n";
  detail += action;
  if (ending.code !== undefined) detail += `\nCode: ${ending.code}`;
  outcome.title = title;
  outcome.detail = detail;
  outcome.phase = "failed";
  outcome.presentation = { type: "signal", status: "terminal", text: detail, level: "error" };

  const result = events.slice();
  const index = findLastIndex(result, isOutcome);
  if (index !== -1) result[index] = outcome;
  else result.push(outcome);
  return result;
}

// ---------------------------------------------------------------------------
// Turn ids
// ---------------------------------------------------------------------------

type TurnSignal =
  | { type: "started" }
  | { type: "ended" }
  | { type: "assistant"; message: string }
  | { type: "step"; message: string; ends: boolean };

function turnSignal(event: TaskEventView): TurnSignal | undefined {
  const value = rawValue(event);
  if (typeof value !== "object" || value === null) return undefined;
  const object = value as Record<string, unknown>;
  const typeName = object.type;
  if (typeof typeName !== "string") return undefined;
  if (typeName.endsWith("turn.started") || typeName === "turn_start" || typeName.endsWith(".turn_start")) {
    return { type: "started" };
  }
  if (
    typeName.endsWith("turn.completed") ||
    typeName.endsWith("turn.failed") ||
    typeName === "turn_end" ||
    typeName.endsWith(".turn_end")
  ) {
    return { type: "ended" };
  }
  if (typeName === "assistant" || typeName.endsWith(".assistant")) {
    const message = object.message;
    const id = typeof message === "object" && message !== null ? (message as Record<string, unknown>).id : undefined;
    if (typeof id === "string" && id !== "") return { type: "assistant", message: id };
  }
  const part = object.part;
  const id = typeof part === "object" && part !== null ? (part as Record<string, unknown>).messageID : undefined;
  if (typeof id === "string" && id !== "") {
    return { type: "step", message: id, ends: typeName === "step_finish" || typeName.endsWith(".step_finish") };
  }
  return undefined;
}

export function deriveTurnIds(events: TaskEventView[]): TaskEventView[] {
  let next = -1;
  const active = new Map<string, number>();
  const assistants = new Map<string, number>();
  const steps = new Map<string, number>();
  const allocate = () => {
    const result = next;
    next -= 1;
    return result;
  };

  return events.map((original) => {
    const event = { ...original };
    const source = event.source;
    const signal = turnSignal(event);
    if (signal?.type === "started") {
      const turn = event.turnId ?? allocate();
      event.turnId = turn;
      active.set(source, turn);
    } else if (signal?.type === "ended") {
      if (event.turnId === undefined) event.turnId = active.get(source);
      active.delete(source);
    } else if (signal?.type === "assistant") {
      const key = `${source}:${signal.message}`;
      const turn = event.turnId ?? assistants.get(key) ?? allocate();
      event.turnId = turn;
      assistants.set(key, turn);
      active.set(source, turn);
    } else if (signal?.type === "step") {
      const key = `${source}:${signal.message}`;
      const turn = event.turnId ?? steps.get(key) ?? allocate();
      event.turnId = turn;
      if (signal.ends) {
        steps.delete(key);
        active.delete(source);
      } else {
        steps.set(key, turn);
        active.set(source, turn);
      }
    } else if (event.turnId !== undefined) {
      active.set(source, event.turnId);
    } else if (active.has(source)) {
      event.turnId = active.get(source);
    }
    return event;
  });
}

// ---------------------------------------------------------------------------
// Small shared helpers
// ---------------------------------------------------------------------------

function parentActionId(event: TaskEventView): string | undefined {
  if (event.parentActionId) return event.parentActionId;
  const object = rawObject(rawValue(event));
  if (object === undefined) return undefined;
  for (const key of ["parent_tool_use_id", "parentToolUseId"]) {
    const candidate = object[key];
    if (typeof candidate === "string" && candidate !== "") return candidate;
  }
  return undefined;
}

function rawValue(event: TaskEventView): unknown {
  if (!event.rawText) return undefined;
  try {
    return JSON.parse(event.rawText);
  } catch {
    return undefined;
  }
}

function rawString(event: TaskEventView, key: string): string | undefined {
  const found = rawObject(rawValue(event))?.[key];
  return typeof found === "string" && found !== "" ? found : undefined;
}

/** SAFETY: guarded on the value being a non-null, non-array object. */
function rawObject(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function eventSubject(event: TaskEventView): string {
  return event.target ?? event.detail ?? "";
}

function eventText(event: TaskEventView): string | undefined {
  const text = event.presentation?.text ?? event.detail;
  return text !== undefined && text.trim() !== "" ? text : undefined;
}

function eventTime(value: string): number | undefined {
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? undefined : parsed;
}

function eventDurationSeconds(start: string, end: string): number | undefined {
  const duration = spanMs(start, end);
  return duration === undefined ? undefined : Math.trunc(duration / 1000) || undefined;
}

function findLast<T>(items: T[], predicate: (item: T) => boolean): T | undefined {
  const index = findLastIndex(items, predicate);
  return index === -1 ? undefined : items[index];
}

function findLastIndex<T>(items: T[], predicate: (item: T) => boolean): number {
  for (let index = items.length - 1; index >= 0; index--) {
    if (predicate(items[index])) return index;
  }
  return -1;
}

function providerLabel(value: string): string {
  switch (value) {
    case "claude":
      return "Claude";
    case "codex":
      return "Codex";
    case "opencode":
      return "OpenCode";
    case "opencode-2":
      return "OpenCode 2";
    case "antigravity":
      return "Antigravity";
    case "pi":
      return "Pi";
    case "fx":
      return "fx";
    default:
      return value;
  }
}

function clip(value: string, limit: number): string {
  const chars = Array.from(value);
  if (chars.length <= limit) return value;
  return chars.slice(0, Math.max(limit - 3, 0)).join("") + "...";
}
