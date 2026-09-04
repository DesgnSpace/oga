// Pure activity composition for the task detail trace.
// Ported from rust/crates/oga-ui/src/activity/mod.rs — keep behavior identical.

import type { EventKind, TaskEventPresentation, TaskEventView, TaskState } from "@/bridge/types";
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

export type ActivityBlock =
  | { type: "chapter"; id: number; rows: ChapterRow[]; title?: string }
  | { type: "reasoning"; pulse: ReasoningPulse }
  | { type: "signal"; event: TaskEventView }
  | { type: "receipt"; event: TaskEventView; thinkingTokens: number; usageWindow?: string }
  | { type: "handoff"; boundary: HandoffBoundary };

export function blockId(block: ActivityBlock): number {
  switch (block.type) {
    case "chapter":
      return block.id;
    case "reasoning":
      return block.pulse.id;
    case "signal":
    case "receipt":
      return block.event.id;
    case "handoff":
      return 0;
  }
}

export type ChapterRow =
  | { type: "work"; event: TaskEventView }
  | { type: "reasoning"; pulse: ReasoningPulse }
  | { type: "group"; group: ActivityGroup };

export function chapterRowId(row: ChapterRow): number {
  switch (row.type) {
    case "work":
      return row.event.id;
    case "reasoning":
      return row.pulse.id;
    case "group":
      return groupId(row.group);
  }
}

export function chapterRowEvents(row: ChapterRow): TaskEventView[] {
  switch (row.type) {
    case "work":
      return [row.event];
    case "reasoning":
      return [];
    case "group":
      return row.group.kind === "turn"
        ? row.group.hidden.slice()
        : [row.group.anchor, ...row.group.hidden];
  }
}

export type ActivityGroupKind = "delegation" | "lifecycle" | "run" | "turn" | "subagent";
export type ActivityGroupStatus = "running" | "needs_input" | "failed" | "done";

export interface ActivityGroup {
  kind: ActivityGroupKind;
  anchor: TaskEventView;
  children: ChapterRow[];
  members: TaskEventView[];
  runLabel: string;
  turnTitle?: string;
  hidden: TaskEventView[];
}

function makeActivityGroup(
  kind: ActivityGroupKind,
  anchor: TaskEventView,
  children: ChapterRow[],
  members: TaskEventView[],
  runLabel: string,
  turnTitle: string | undefined,
): ActivityGroup {
  const hidden = kind === "lifecycle" ? members.slice() : children.flatMap(chapterRowEvents);
  return { kind, anchor, children, members, runLabel, turnTitle, hidden };
}

export function groupId(group: ActivityGroup): number {
  return group.anchor.id;
}

export function groupStatus(group: ActivityGroup): ActivityGroupStatus {
  const hasQuestion = (event: TaskEventView) => event.title === "Worker needs input";
  if (hasQuestion(group.anchor) || group.hidden.some(hasQuestion)) return "needs_input";
  if (group.anchor.phase === "failed" || group.hidden.some((event) => event.phase === "failed")) {
    return "failed";
  }
  if (group.kind === "turn") {
    return group.anchor.phase === "started" || group.children.some(chapterRowIsRunning) ? "running" : "done";
  }
  if (group.kind === "subagent") {
    return group.hidden.some((event) => subagentBoundary(event)?.role === "stop") ? "done" : "running";
  }
  return group.anchor.phase === "started" || group.children.some(chapterRowIsRunning) ? "running" : "done";
}

function chapterRowIsRunning(row: ChapterRow): boolean {
  if (row.type === "work") return row.event.phase === "started";
  if (row.type === "reasoning") return false;
  return groupStatus(row.group) === "running";
}

export function groupStartsExpanded(group: ActivityGroup): boolean {
  if (group.kind === "lifecycle" || group.kind === "run" || group.kind === "turn" || group.kind === "subagent") {
    return false;
  }
  return groupStatus(group) !== "done";
}

export function groupDurationMs(group: ActivityGroup): number | undefined {
  const last = group.hidden[group.hidden.length - 1];
  if (!last) return undefined;
  const from = eventTime(group.anchor.createdAt);
  const to = eventTime(last.createdAt);
  if (from === undefined || to === undefined) return undefined;
  const duration = to - from;
  return duration > 0 ? duration : undefined;
}

export function groupLabel(group: ActivityGroup): string {
  if (group.kind === "turn" && group.turnTitle !== undefined) {
    if (group.turnTitle === "Work step") return `Work step #${group.anchor.id}`;
    if (group.children.length === 0) {
      return `Message #${group.anchor.id}: ${clip(group.turnTitle, 64)}`;
    }
    return group.turnTitle;
  }
  for (const child of group.children) {
    const event = firstMeaningful(child);
    if (event) {
      const title = event.title.trim();
      if (title !== "") return title;
    }
  }
  for (const child of group.children) {
    const prose = firstProse(child);
    if (prose !== undefined) {
      const line = (prose.split("\n")[0] ?? prose).trim();
      if (line !== "") return `Message #${group.anchor.id}: ${clip(line, 64)}`;
    }
  }
  return `Work step #${group.anchor.id}`;
}

export interface HandoffBoundary {
  chain: string;
  earlierRuns: HandoffRun[];
  hiddenEventCount: number;
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

function composeWithState(
  rawEvents: TaskEventView[],
  settled: boolean,
  ending: ActivityEnding | undefined,
  showReceipt = settled,
): ActivityComposition {
  rawEvents = normalizeAntigravityEvents(rawEvents);
  const folded = foldActions(rawEvents);
  const events = deriveTurnIds(folded.rows);
  const boundaries = events
    .map((event, index) => [index, event] as const)
    .filter(([, event]) => event.type === "handed_off" || event.title === "Handed off to another profile");
  if (boundaries.length === 0) {
    return composeFlat(events, folded.members, settled, ending, showReceipt);
  }

  const hops = boundaries.map(([, event]) => hopFromDetail(event.detail));
  const segments: TaskEventView[][] = [];
  let start = 0;
  for (const [index] of boundaries) {
    segments.push(events.slice(start, index));
    start = index + 1;
  }
  segments.push(events.slice(start));
  const composed = segments.map((segment, index) =>
    composeFlat(
      segment,
      folded.members,
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
    { type: "handoff", boundary: { chain: hopChain(hops), earlierRuns, hiddenEventCount } },
  ];
  const current = composed[composed.length - 1];
  if (current) blocks.push(...current.blocks);
  return {
    blocks,
    technical: composed.flatMap((composition) => composition.technical),
  };
}

/** Converts Antigravity's raw step protocol into the same rows other providers use. */
export function normalizeAntigravityEvents(events: TaskEventView[]): TaskEventView[] {
  const normalized: TaskEventView[] = [];
  for (const event of events) {
    if (event.source !== "antigravity") {
      normalized.push(event);
      continue;
    }
    const payload = parseAntigravityPayload(event.rawText);
    const step = payload?.step_update;
    const stepType = stringValue(step?.step_type);
    if (payload?.event === "step_update" && step) {
      if (stepType === "user_input" || stepType === "system_message" || stepType === "agent_response") continue;
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
        const label = name === "call_mcp_tool"
          ? [stringValue(parameters.ServerName), stringValue(parameters.ToolName)].filter(Boolean).join("/") || name
          : name;
        const output = stringValue(info?.output);
        const error = info?.error ?? step.error;
        const state = stringValue(step.state)?.toLowerCase();
        const mapped = antigravityToolPresentation(name, parameters, output);
        normalized.push({
          ...event,
          kind: mapped?.kind ?? event.kind,
          title: label,
          detail: mapped?.detail ?? (Object.keys(parameters).length > 0 ? JSON.stringify(parameters) : event.detail),
          result: stringValue(error?.message) ?? output ?? event.result,
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
  Arguments?: unknown;
}

interface AntigravityStep {
  step_type?: string;
  step_index?: number;
  state?: string;
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

const SCHEDULING_TITLES = new Set(["Waiting", "Continuing", "Queued"]);
const HOLD_OUTCOME_TITLES = new Set(["Waiting ended", "Wait timed out"]);

function isTechnical(event: TaskEventView): boolean {
  if (event.minor === true) return true;
  if (event.kind === "raw") {
    return [
      "Step Start",
      "Step Finish",
      "Tool result",
      "Hook Started",
      "Hook Response",
      // An unnamed agent hook: a raw payload with no label a reader can use.
      "Event",
      "Status",
      "Commands Changed",
    ].includes(event.title);
  }
  if (event.title === "Heartbeat") return true;
  if (SCHEDULING_TITLES.has(event.title)) return true;
  // A usage-window notice earns its place only when it says what was limited
  // and for how long; the bare marker repeats without telling a reader
  // anything.
  if (event.title === USAGE_WINDOW_TITLE && (event.detail === undefined || event.detail === event.title)) {
    return true;
  }
  // A hold that came and went says nothing a reader can act on. It stays only
  // when it carries the reason a run never started.
  if (HOLD_OUTCOME_TITLES.has(event.title) && !event.detail) return true;
  if (event.kind === "tool" && event.detail === undefined && event.presentation === undefined) {
    return true;
  }
  return [
    "Task queued",
    "Worker started",
    "Worker spawned",
    "Task completed",
    "Session started",
    "Session Reused",
    "Session Captured",
    "Archived",
    "Handoff brief built",
  ].includes(event.title);
}

export const ActivityGrouping = {
  group(
    rows: ChapterRow[],
    members: Map<number, TaskEventView[]>,
    turns: Map<number, number>,
    splitTurns = true,
  ): ChapterRow[] {
    const nested = nestDelegations(nestSubagents(rows, members, turns)).map((row) => foldLifecycle(row, members));
    const folded = foldRepeats(foldRuns(nested));
    return splitTurns ? groupTurns(folded, turns) : folded;
  },
};

export const SUBAGENT_STARTED_TITLE = "Subagent started";
export const SUBAGENT_FINISHED_TITLE = "Subagent finished";

/**
 * The words an agent says before it starts working — "Checking types, lint,
 * and Rust command compilation" — are the only section titles the trace has,
 * and every provider streams them as an ordinary message. One short line is a
 * heading; a paragraph, a list, or a long line is the worker's actual answer
 * and stays a message of its own.
 */
const NARRATION_WORD_LIMIT = 16;

export function narrationTitle(event: TaskEventView): string | undefined {
  if (event.kind !== "message" || isTechnical(event)) return undefined;
  const text = eventText(event);
  if (text === undefined) return undefined;
  const lines = text.split("\n").map((line) => line.trim()).filter((line) => line !== "");
  if (lines.length !== 1) return undefined;
  const line = lines[0];
  if (/^[#>*\-|`]/.test(line)) return undefined;
  return line.split(/\s+/).length <= NARRATION_WORD_LIMIT ? line : undefined;
}

/** How much work a chapter holds: the tool calls a reader would have counted. */
export function chapterCallCount(rows: ChapterRow[]): number {
  return rows.flatMap(chapterRowEvents).filter(meaningful).length;
}

export function chapterDurationMs(rows: ChapterRow[]): number | undefined {
  const events = rows.flatMap(chapterRowEvents);
  const first = events[0];
  const last = events[events.length - 1];
  if (!first || !last) return undefined;
  const from = eventTime(first.createdAt);
  const to = eventTime(last.createdAt);
  if (from === undefined || to === undefined) return undefined;
  const duration = to - from;
  return duration > 0 ? duration : undefined;
}

/**
 * Claude Code brackets a subagent run two ways. Hooks give a start/stop pair
 * sharing an `agent_id` (normalized to the same started/finished titles as
 * the streamed pair, with wire names kept as a fallback), and the stream
 * gives a `task_started` / `task_notification` pair sharing a `task_id`; no
 * other provider streams a subagent's own events into the parent's trace at
 * all, so there is nothing else to nest. Everything a matched pair owns moves
 * under it, reprocessed through the same grouping pipeline so its runs and
 * turns fold the same way the top-level trace does. A start with no matching
 * stop is left exactly where it fell rather than guessing where the subagent
 * ended.
 */
function nestSubagents(
  rows: ChapterRow[],
  members: Map<number, TaskEventView[]>,
  turns: Map<number, number>,
): ChapterRow[] {
  const starts = new Map<string, { index: number; event: TaskEventView }>();
  rows.forEach((row, index) => {
    if (row.type !== "work") return;
    const boundary = subagentBoundary(row.event);
    if (boundary?.role !== "start") return;
    if (!starts.has(boundary.id)) starts.set(boundary.id, { index, event: row.event });
  });
  if (starts.size === 0) return rows;

  const pairs: { owns: (event: TaskEventView) => boolean; start: number; stop: number }[] = [];
  for (const [id, { index: start, event }] of starts) {
    let stop: number | undefined;
    for (let index = start + 1; index < rows.length; index++) {
      const row = rows[index];
      if (row.type !== "work") continue;
      const boundary = subagentBoundary(row.event);
      if (boundary?.role === "stop" && boundary.id === id) {
        stop = index;
        break;
      }
    }
    if (stop !== undefined) pairs.push({ owns: subagentMembership(event, id), start, stop });
  }
  if (pairs.length === 0) return rows;

  // Two subagents can run at once, and their rows arrive interleaved — real
  // runs have shown five at a time — so membership between a pair's start and
  // stop is decided by what each row says it belongs to, never by index range:
  // a range can span rows that belong to a different subagent entirely.
  const childrenOf = new Map<number, number[]>();
  const consumed = new Set<number>();
  for (const { owns, start, stop } of pairs) {
    consumed.add(start);
    const owned: number[] = [];
    for (let index = start + 1; index <= stop; index++) {
      const row = rows[index];
      if (row.type === "work" && (index === stop || owns(row.event))) {
        owned.push(index);
        consumed.add(index);
      }
    }
    childrenOf.set(start, owned);
  }

  const result: ChapterRow[] = [];
  rows.forEach((row, index) => {
    const owned = childrenOf.get(index);
    if (owned) {
      if (row.type !== "work") return;
      const rawChildren = owned.map((childIndex) => rows[childIndex]);
      const children = ActivityGrouping.group(rawChildren, members, turns);
      result.push({
        type: "group",
        group: makeActivityGroup("subagent", row.event, children, [], subagentLabel(row.event), undefined),
      });
      return;
    }
    if (!consumed.has(index)) result.push(row);
  });
  return result;
}

function subagentBoundary(event: TaskEventView): { id: string; role: "start" | "stop" } | undefined {
  if (event.title === "SubagentStart" || event.title === "SubagentStop" || event.title === "Spawned subagent") {
    const id = rawString(event, "agent_id");
    if (id === undefined) return undefined;
    const role = event.title === "SubagentStop" ? "stop" : "start";
    return { id, role };
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

function composeFlat(
  rawEvents: TaskEventView[],
  members: Map<number, TaskEventView[]>,
  settled: boolean,
  ending: ActivityEnding | undefined,
  showReceipt: boolean,
): ActivityComposition {
  const events = terminalOutcome(
    withoutRedundantTurnFailure(rawEvents.map(retryMessage)),
    ending,
  );
  const usageWindow = usageWindowSummary(events);
  const turns = new Map<number, number>();
  for (const event of events) if (event.turnId !== undefined) turns.set(event.id, event.turnId);
  const receiptId = showReceipt ? [...events].reverse().find((event) => event.kind === "usage")?.id : undefined;
  const responseId = settled ? closingMessageId(events) : undefined;

  const blocks: ActivityBlock[] = [];
  const technical: TaskEventView[] = [];
  let chapter: ChapterRow[] = [];
  let chapterTitle: { id: number; text: string } | undefined;
  let pulse: TaskEventView[] = [];
  let thinkingTokens = 0;
  // Counters tick out between blocks, so they belong to the stretch of
  // thinking they land in rather than to any one event.
  let pulseTokens = 0;
  let receiptIndex: number | undefined;

  const flushPulse = () => {
    const first = pulse[0];
    if (!first) return;
    const last = pulse[pulse.length - 1] ?? first;
    const pulseValue: ReasoningPulse = {
      id: first.id,
      text: mergeReasoning(pulse),
      tokens: pulseTokens > 0 ? pulseTokens : undefined,
      seconds: eventDurationSeconds(first.createdAt, last.createdAt),
    };
    pulseTokens = 0;
    if (chapter.length === 0) blocks.push({ type: "reasoning", pulse: pulseValue });
    else chapter.push({ type: "reasoning", pulse: pulseValue });
    pulse = [];
  };
  const flushChapter = () => {
    const title = chapterTitle;
    chapterTitle = undefined;
    if (chapter.length === 0 && title === undefined) return;
    const id = title?.id ?? chapterRowId(chapter[0]);
    // A titled chapter already marks where one stretch of work ends and the
    // next begins, so nesting its turns underneath would only add a level.
    const rows = ActivityGrouping.group(chapter, members, turns, title === undefined);
    chapter = [];
    blocks.push({ type: "chapter", id, rows, title: title?.text });
  };

  for (const event of events) {
    const tokens = event.presentation?.tokensThinking ?? 0;
    thinkingTokens += tokens;
    pulseTokens += tokens;
    if (isTechnical(event) && event.id !== receiptId) {
      technical.push(event);
      continue;
    }
    if (isThinkingPulse(event)) {
      pulse.push(event);
      continue;
    }
    flushPulse();
    if (event.id === responseId) continue;
    const narration = narrationTitle(event);
    if (narration !== undefined) {
      flushChapter();
      chapterTitle = { id: event.id, text: narration };
      continue;
    }
    if (event.kind === "usage") {
      if (event.id === receiptId) {
        flushChapter();
        receiptIndex = blocks.length;
        blocks.push({ type: "receipt", event, thinkingTokens, usageWindow });
      } else {
        technical.push(event);
      }
      continue;
    }
    if (isSignal(event)) {
      flushChapter();
      blocks.push({ type: "signal", event });
      continue;
    }
    const previousRow = chapter[chapter.length - 1];
    if (previousRow?.type === "work" && sameShape(previousRow.event, event)) {
      chapter.pop();
      chapter.push({ type: "work", event: { ...event, id: previousRow.event.id } });
    } else {
      chapter.push({ type: "work", event });
    }
  }
  flushPulse();
  flushChapter();

  if (receiptIndex !== undefined) {
    const block = blocks[receiptIndex];
    if (block.type === "receipt" && thinkingTokens > block.thinkingTokens) {
      blocks[receiptIndex] = { ...block, thinkingTokens };
    }
  }

  return { blocks: mergeSkipNotices(collapseWaits(blocks)), technical };
}

export const ACTIVITY_SKIPPED_TITLE = "Some activity was not recorded";
/** Every provider's reasoning blocks share this title; it is how they fold. */
export const REASONING_TITLE = "Thinking";
const USAGE_WINDOW_TITLE = "Usage window";

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
  const inRows = (rows: ChapterRow[]): boolean =>
    rows.some((row) => (row.type === "group" ? inRows(row.group.children) : row.type === "reasoning"));
  return composition.blocks.some((block) =>
    block.type === "reasoning" ? true : block.type === "chapter" && inRows(block.rows),
  );
}

/**
 * Output goes missing a line at a time, so a lossy run can raise the notice
 * dozens of times. A reader needs to know once, with the total.
 */
function mergeSkipNotices(blocks: ActivityBlock[]): ActivityBlock[] {
  const counts = new Map<string, number>();
  let first: number | undefined;
  blocks.forEach((block, index) => {
    if (block.type !== "signal" || block.event.title !== ACTIVITY_SKIPPED_TITLE) return;
    if (first === undefined) first = index;
    const skipped = parseSkipped(block.event.detail);
    if (skipped) counts.set(skipped.noun, (counts.get(skipped.noun) ?? 0) + skipped.count);
  });
  if (first === undefined) return blocks;

  const totals = [...counts]
    .map(([noun, count]) => `${count} ${noun}${count === 1 ? "" : "s"} skipped`)
    .join(" · ");
  const text = totals === "" ? ACTIVITY_SKIPPED_TITLE : `${ACTIVITY_SKIPPED_TITLE} — ${totals}`;
  return blocks.flatMap((block, index) => {
    if (block.type !== "signal" || block.event.title !== ACTIVITY_SKIPPED_TITLE) return [block];
    if (index !== first) return [];
    return [
      {
        type: "signal" as const,
        event: {
          ...block.event,
          detail: totals === "" ? undefined : totals,
          presentation: { type: "signal" as const, text, level: "warning" as const },
        },
      },
    ];
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
  const index = findLastIndex(events, (event) => event.title === USAGE_WINDOW_TITLE);
  if (index === -1) return undefined;
  const windows = rawObject(rawObject(rawObject(rawValue(events[index]))?.rate_limit_info)?.unifiedWindows);
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

const WAIT_TITLES = new Set(["Waiting", "Waiting ended", "Continuing", "Wait timed out", "Task blocked"]);

function collapseWaits(blocks: ActivityBlock[]): ActivityBlock[] {
  const output: ActivityBlock[] = [];
  const seen = new Map<string, number>();
  const counts = new Map<string, number>();
  for (const block of blocks) {
    if (block.type !== "signal") {
      output.push(block);
      continue;
    }
    const event = block.event;
    if (!WAIT_TITLES.has(event.title)) {
      output.push(block);
      continue;
    }
    const base = stripWaitCount(event.detail ?? event.title);
    const key = `${event.title}|${base}`;
    const index = seen.get(key);
    if (index === undefined) {
      seen.set(key, output.length);
      counts.set(key, 1);
      output.push(block);
      continue;
    }
    const count = (counts.get(key) ?? 1) + 1;
    counts.set(key, count);
    const previous = output[index];
    if (previous.type === "signal") {
      const text = `${base} · ×${count}`;
      output[index] = {
        type: "signal",
        event: {
          ...previous.event,
          detail: text,
          presentation: { type: "signal", text, level: "info" },
        },
      };
    }
  }
  return output;
}

function stripWaitCount(value: string): string {
  const marker = " · ×";
  const index = value.lastIndexOf(marker);
  if (index === -1) return value;
  const rest = value.slice(index + marker.length);
  return /^[0-9]+$/.test(rest) ? value.slice(0, index) : value;
}

function nestDelegations(rows: ChapterRow[]): ChapterRow[] {
  const anchorAt = new Map<string, number>();
  rows.forEach((row, index) => {
    if (row.type !== "work") return;
    const action = row.event.actionId;
    if (!action) return;
    if (!anchorAt.has(action)) anchorAt.set(action, index);
  });
  const childrenOf = new Map<number, number[]>();
  const nested = new Set<number>();
  rows.forEach((row, index) => {
    if (row.type !== "work") return;
    const parent = parentActionId(row.event);
    if (parent === undefined) return;
    const anchor = anchorAt.get(parent);
    if (anchor === undefined || anchor >= index) return;
    const list = childrenOf.get(anchor) ?? [];
    list.push(index);
    childrenOf.set(anchor, list);
    nested.add(index);
  });
  if (nested.size === 0) return rows;
  const build = (index: number): ChapterRow => {
    const row = rows[index];
    if (row.type !== "work") return row;
    const children = childrenOf.get(index);
    if (!children) return row;
    return {
      type: "group",
      group: makeActivityGroup("delegation", row.event, children.map(build), [], "", undefined),
    };
  };
  return rows.map((_, index) => index).filter((index) => !nested.has(index)).map(build);
}

function foldLifecycle(row: ChapterRow, members: Map<number, TaskEventView[]>): ChapterRow {
  if (row.type === "work") {
    const events = members.get(row.event.id);
    if (!events) return row;
    return { type: "group", group: makeActivityGroup("lifecycle", row.event, [], events, "", undefined) };
  }
  if (row.type === "reasoning") return row;
  return {
    type: "group",
    group: makeActivityGroup(
      row.group.kind,
      row.group.anchor,
      row.group.children.map((child) => foldLifecycle(child, members)),
      row.group.members,
      row.group.runLabel,
      row.group.turnTitle,
    ),
  };
}

const RUN_FLOOR = 3;
const NAMED_RUN_FLOOR = 2;
const RUN_GAP_SECONDS = 5 * 60;
const REPEAT_BLOCK_MAX = 4;

/**
 * Every way a run has of going to look something up, whatever tool or shell
 * command it reached for. They share one run key, so a stretch of looking
 * around folds into one row even when it mixed reading, grepping and listing.
 */
const LOOKUP_NOUNS = new Map<string, string>([
  ["read file", "files"],
  ["search code", "searches"],
  ["find files", "file searches"],
  ["list directory", "directory listings"],
  ["inspect changes", "change checks"],
  ["web search", "web searches"],
  ["fetch page", "page fetches"],
  ["task status", "task checks"],
  ["check permissions", "permission checks"],
  ["schedule check", "schedule checks"],
]);

/** What each verification step proves, for the row that names a whole batch. */
const CHECK_NAMES = new Map<string, string>([
  ["check lint", "lint"],
  ["check types", "types"],
  ["check tests", "tests"],
  ["check build", "build"],
]);

/** A verification whose command does not say what it proves. */
const UNNAMED_CHECK = "run checks";

const EDIT_TITLES = new Set([
  "edit file",
  "edit files",
  "write file",
  "write files",
  "delete file",
  "create file",
  "apply patch",
]);

function foldRuns(rows: ChapterRow[]): ChapterRow[] {
  const mapped = rows.map((row): ChapterRow => {
    if (row.type === "group" && row.group.kind === "delegation") {
      return {
        type: "group",
        group: makeActivityGroup(
          row.group.kind,
          row.group.anchor,
          foldRuns(row.group.children),
          row.group.members,
          row.group.runLabel,
          row.group.turnTitle,
        ),
      };
    }
    return row;
  });

  const folded: ChapterRow[] = [];
  let run: ChapterRow[] = [];
  let calls = 0;
  let lastCall: number | undefined;

  const flush = () => {
    const anchor = run[0] ? callEvent(run[0]) : undefined;
    if (!anchor || calls < runFloor(anchor)) {
      folded.push(...run);
      run = [];
      calls = 0;
      lastCall = undefined;
      return;
    }
    const split = lastCall ?? 0;
    const children = run.slice(0, split + 1);
    const trailing = run.slice(split + 1);
    const label = runLabel(children.map(callEvent).filter((event): event is TaskEventView => event !== undefined));
    folded.push({
      type: "group",
      group: makeActivityGroup("run", anchor, children, [], label, undefined),
    });
    folded.push(...trailing);
    run = [];
    calls = 0;
    lastCall = undefined;
  };

  for (const row of mapped) {
    const event = callEvent(row);
    if (event) {
      const previous = [...run].reverse().map(callEvent).find((candidate) => candidate !== undefined);
      const open = run[0] ? callEvent(run[0]) : undefined;
      if (
        open &&
        (runKey(open) !== runKey(event) ||
          open.turnId !== event.turnId ||
          exceedsGap(previous, event, RUN_GAP_SECONDS))
      ) {
        flush();
      }
      run.push(row);
      calls += 1;
      lastCall = run.length - 1;
    } else if (isThought(row) && run.length > 0) {
      run.push(row);
    } else {
      flush();
      folded.push(row);
    }
  }
  flush();
  return folded;
}

/**
 * A run that is stuck in a loop — edit, lint, test, edit, lint, test — spends
 * most of the trace saying the same thing again. The repeated stretch collapses
 * to one row carrying how many times it came round; the passes themselves stay
 * inside it.
 */
function foldRepeats(rows: ChapterRow[]): ChapterRow[] {
  const signatures = rows.map(rowSignature);
  const folded: ChapterRow[] = [];
  let index = 0;
  while (index < rows.length) {
    const repeat = repeatAt(signatures, index);
    const anchor = repeat && rowAnchor(rows[index]);
    if (!repeat || !anchor) {
      folded.push(rows[index]);
      index += 1;
      continue;
    }
    const members = rows.slice(index, index + repeat.size * repeat.count);
    folded.push({
      type: "group",
      group: makeActivityGroup("run", anchor, members, [], repeatLabel(anchor, members[0], repeat), undefined),
    });
    index += members.length;
  }
  return folded;
}

function rowAnchor(row: ChapterRow): TaskEventView | undefined {
  if (row.type === "work") return row.event;
  if (row.type === "group") return row.group.anchor;
  return undefined;
}

interface Repeat {
  size: number;
  count: number;
}

function repeatAt(signatures: (string | undefined)[], start: number): Repeat | undefined {
  for (let size = 1; size <= REPEAT_BLOCK_MAX; size++) {
    if (start + size * 2 > signatures.length) return undefined;
    if (signatures.slice(start, start + size).some((signature) => signature === undefined)) {
      return undefined;
    }
    let count = 1;
    while (blockRepeats(signatures, start, start + count * size, size)) count += 1;
    if (count > 1) return { size, count };
  }
  return undefined;
}

function blockRepeats(
  signatures: (string | undefined)[],
  first: number,
  next: number,
  size: number,
): boolean {
  if (next + size > signatures.length) return false;
  for (let offset = 0; offset < size; offset++) {
    if (signatures[first + offset] !== signatures[next + offset]) return false;
  }
  return true;
}

function repeatLabel(anchor: TaskEventView, first: ChapterRow, repeat: Repeat): string {
  if (repeat.size > 1) return `Repeated ${repeat.size} steps ×${repeat.count}`;
  if (first.type === "group") return `${first.group.runLabel} ×${repeat.count}`;
  const subject = anchor.kind === "file" ? fileName(eventSubject(anchor)) : eventSubject(anchor);
  return `${anchor.verb ?? "Ran"} ${clip(subject, 48)} ×${repeat.count}`;
}

const REPEATABLE_KINDS: EventKind[] = ["tool", "file", "command"];

/**
 * Two rows repeat when they say the same thing in the same turn. Prose and
 * lifecycle rows have no signature: a second "Rate limit" notice is not the run
 * doing the same work twice, and folding it would say it was.
 */
function rowSignature(row: ChapterRow): string | undefined {
  if (row.type === "group") {
    return row.group.kind === "run" ? `run|${row.group.runLabel}` : undefined;
  }
  if (row.type !== "work") return undefined;
  const event = row.event;
  if (!REPEATABLE_KINDS.includes(event.kind)) return undefined;
  const subject = eventSubject(event);
  if (subject === "") return undefined;
  return `${event.turnId ?? ""}|${eventTitleKey(event)}|${subject}`;
}

function groupTurns(rows: ChapterRow[], turns: Map<number, number>): ChapterRow[] {
  const distinct = new Set(
    rows.map((row) => chapterRowTurnId(row, turns)).filter((value): value is number => value !== undefined),
  );
  if (distinct.size <= 1) return rows;

  const grouped: ChapterRow[] = [];
  let active: { turn: number; rows: ChapterRow[] } | undefined;
  const flush = () => {
    if (!active) return;
    const rows = active.rows;
    active = undefined;
    const anchor = rows.flatMap(chapterRowEvents)[0];
    if (!anchor) return;
    const [title, children] = turnTitleAndRows(rows);
    grouped.push({ type: "group", group: makeActivityGroup("turn", anchor, children, [], "", title) });
  };
  for (const row of rows) {
    const turn = chapterRowTurnId(row, turns);
    if (turn === undefined) {
      flush();
      grouped.push(row);
      continue;
    }
    if (active && active.turn === turn) active.rows.push(row);
    else {
      flush();
      active = { turn, rows: [row] };
    }
  }
  flush();
  return grouped;
}

function turnTitleAndRows(rows: ChapterRow[]): [string, ChapterRow[]] {
  for (let index = 0; index < rows.length; index++) {
    const row = rows[index];
    if (row.type !== "work") continue;
    const event = row.event;
    if (event.kind !== "message" || isTechnical(event)) continue;
    const text = eventText(event);
    if (text === undefined) continue;
    const line = (text.split("\n")[0] ?? text).trim();
    if (line === "") continue;
    const filtered = [...rows.slice(0, index), ...rows.slice(index + 1)];
    return [clip(line, 80), filtered];
  }
  for (const row of rows) {
    const event = meaningfulRowEvent(row);
    if (event) {
      const title = event.title.trim();
      if (title !== "") return [title, rows];
    }
  }
  // A turn led by a nested message still has words to show; "Work step" is
  // what is left when the turn genuinely said nothing.
  for (const row of rows) {
    const prose = firstProse(row);
    if (prose === undefined) continue;
    const line = (prose.split("\n")[0] ?? prose).trim();
    if (line !== "") return [clip(line, 80), rows];
  }
  return ["Work step", rows];
}

function meaningfulRowEvent(row: ChapterRow): TaskEventView | undefined {
  if (row.type === "work" && meaningful(row.event)) return row.event;
  if (row.type === "group" && meaningful(row.group.anchor)) return row.group.anchor;
  return undefined;
}

function firstMeaningful(row: ChapterRow): TaskEventView | undefined {
  if (row.type === "work") return meaningful(row.event) ? row.event : undefined;
  if (row.type === "group") {
    if (meaningful(row.group.anchor)) return row.group.anchor;
    for (const child of row.group.children) {
      const found = firstMeaningful(child);
      if (found) return found;
    }
  }
  return undefined;
}

function firstProse(row: ChapterRow): string | undefined {
  if (row.type === "work" && row.event.kind === "message" && !isTechnical(row.event)) {
    return eventText(row.event);
  }
  if (row.type === "group") {
    for (const child of row.group.children) {
      const found = firstProse(child);
      if (found !== undefined) return found;
    }
  }
  return undefined;
}

function meaningful(event: TaskEventView): boolean {
  return !isTechnical(event) && (["tool", "file", "command", "retry"] as EventKind[]).includes(event.kind);
}

function callEvent(row: ChapterRow): TaskEventView | undefined {
  if (row.type === "work" && row.event.kind !== "reasoning") return row.event;
  if (row.type === "group" && row.group.kind === "lifecycle") return row.group.anchor;
  return undefined;
}

function isThought(row: ChapterRow): boolean {
  if (row.type === "reasoning") return true;
  if (row.type === "work") return row.event.kind === "reasoning";
  return false;
}

function exceedsGap(previous: TaskEventView | undefined, current: TaskEventView, gapSeconds: number): boolean {
  if (!previous) return false;
  const from = eventTime(previous.createdAt);
  const to = eventTime(current.createdAt);
  if (from === undefined || to === undefined) return false;
  return Math.trunc((to - from) / 1000) > gapSeconds;
}

function chapterRowTurnId(row: ChapterRow, turns: Map<number, number>): number | undefined {
  switch (row.type) {
    case "work":
      return row.event.turnId;
    case "reasoning":
      return turns.get(row.pulse.id);
    case "group":
      return row.group.anchor.turnId;
  }
}

function eventTitleKey(event: TaskEventView): string {
  return event.title.trim().toLowerCase();
}

function eventSubject(event: TaskEventView): string {
  return event.target ?? event.detail ?? "";
}

/**
 * What makes two neighbouring rows one run. A run has to be homogeneous —
 * folding an edit into "Read 3 files" hides the edit — so looking things up,
 * verifying, and changing a file each key differently, and edits key on the
 * file they touch so six passes over one stylesheet read as one row.
 */
function runKey(event: TaskEventView): string {
  const title = eventTitleKey(event);
  if (CHECK_NAMES.has(title) || title === UNNAMED_CHECK) return "check";
  if (LOOKUP_NOUNS.has(title)) return "lookup";
  if (EDIT_TITLES.has(title)) return `edit:${eventSubject(event)}`;
  return event.kind;
}

/**
 * A stretch of the same named work is worth folding at two rows; a stretch of
 * rows that share only their event kind needs three before the fold says
 * anything.
 */
function runFloor(event: TaskEventView): number {
  return runKey(event) === event.kind ? RUN_FLOOR : NAMED_RUN_FLOOR;
}

/** A run's row says what the run did; it carries no verb of its own. */
function runLabel(events: TaskEventView[]): string {
  const anchor = events[0];
  const count = events.length;
  const key = runKey(anchor);
  if (key === "check") {
    const names = [
      ...new Set(events.map((event) => CHECK_NAMES.get(eventTitleKey(event))).filter((name) => name !== undefined)),
    ];
    return names.length > 0 ? `Checked ${names.join(", ")}` : `Ran ${count} checks`;
  }
  if (key.startsWith("edit:")) {
    return `Edited ${fileName(eventSubject(anchor))} ×${count}`;
  }
  if (key === "lookup") {
    const titles = new Set(events.map(eventTitleKey));
    if (titles.size > 1) return `Ran ${count} lookups`;
    const title = eventTitleKey(anchor);
    if (title === "read file") return `Read ${count} files`;
    return `Ran ${count} ${LOOKUP_NOUNS.get(title)}`;
  }
  if (anchor.kind === "command") return `Ran ${count} commands`;
  if (anchor.kind === "file") return `Changed ${count} files`;
  return `Ran ${count} calls`;
}

function fileName(path: string): string {
  const name = path.split("/").at(-1);
  return name === undefined || name === "" ? path : name;
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
  const from = eventTime(start);
  const to = eventTime(end);
  if (from === undefined || to === undefined) return undefined;
  const duration = Math.trunc((to - from) / 1000);
  return duration > 0 ? duration : undefined;
}

function isThinkingPulse(event: TaskEventView): boolean {
  return event.kind === "reasoning" && event.title === REASONING_TITLE;
}

function isSignal(event: TaskEventView): boolean {
  return (
    event.presentation?.type === "signal" ||
    event.kind === "error" ||
    event.title === "Worker needs input" ||
    event.title === ACTIVITY_SKIPPED_TITLE
  );
}

const SUBAGENT_BOUNDARY_TITLES = new Set([
  "SubagentStart",
  "SubagentStop",
  "Spawned subagent",
  SUBAGENT_STARTED_TITLE,
  SUBAGENT_FINISHED_TITLE,
]);

function sameShape(left: TaskEventView, right: TaskEventView): boolean {
  // A subagent boundary's identity lives in its payload, not its title/detail —
  // two subagents of the same type starting back to back would otherwise
  // collapse into one marker and the second subagent would vanish.
  if (SUBAGENT_BOUNDARY_TITLES.has(left.title)) return false;
  return (
    left.actionId === undefined &&
    right.actionId === undefined &&
    left.kind === right.kind &&
    left.title === right.title &&
    left.detail === right.detail
  );
}

function closingMessageId(events: TaskEventView[]): number | undefined {
  const index = findLastIndex(
    events,
    (event) => event.kind === "message" && event.minor !== true && eventText(event) !== undefined,
  );
  if (index === -1) return undefined;
  const rest = events.slice(index + 1);
  return rest.every((event) => isTechnical(event) || event.kind === "usage") ? events[index].id : undefined;
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
  const found = [...events].reverse().find((event) => event.source === "broker" && event.title === brokerTitle);
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
  const index = findLastIndex(result, (event) => event.source === "broker" && event.title === brokerTitle);
  if (index !== -1) result[index] = outcome;
  else result.push(outcome);
  return result;
}

function findLastIndex<T>(items: T[], predicate: (item: T) => boolean): number {
  for (let index = items.length - 1; index >= 0; index--) {
    if (predicate(items[index])) return index;
  }
  return -1;
}

interface FoldedActions {
  rows: TaskEventView[];
  members: Map<number, TaskEventView[]>;
}

function foldActions(events: TaskEventView[]): FoldedActions {
  const slots = new Map<string, number>();
  const rows: TaskEventView[] = [];
  const members = new Map<number, TaskEventView[]>();
  for (const event of events) {
    const action = event.actionId && event.actionId !== "" ? event.actionId : undefined;
    if (action === undefined) {
      rows.push(event);
      continue;
    }
    const index = slots.get(action);
    if (index !== undefined && replays(rows[index], event)) {
      const list = members.get(rows[index].id) ?? [];
      list.push(event);
      members.set(rows[index].id, list);
      continue;
    }
    if (index !== undefined && !reopens(rows[index], event)) {
      const list = members.get(rows[index].id) ?? [];
      list.push(event);
      members.set(rows[index].id, list);
      rows[index] = settleAction(rows[index], event);
      continue;
    }
    if (event.title === "Tool progress") continue;
    if (isTechnical(event)) {
      rows.push(event);
      continue;
    }
    slots.set(action, rows.length);
    members.set(event.id, [event]);
    rows.push(event);
  }
  const filteredMembers = new Map<number, TaskEventView[]>();
  for (const [id, memberEvents] of members) {
    if (memberEvents.length > 1 && memberEvents.some(isLifecycleUpdate)) filteredMembers.set(id, memberEvents);
  }
  return { rows, members: filteredMembers };
}

function reopens(row: TaskEventView, event: TaskEventView): boolean {
  return (row.phase === "completed" || row.phase === "failed") && event.phase === "started" && event.minor !== true;
}

/**
 * Resuming a session replays the whole prior transcript, so every tool call
 * that already finished arrives a second time as a fresh start under its
 * original id. It is the same call, and opening a second row for it doubles the
 * trace.
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
      later.title === "Tool progress" &&
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
    if (later.phase === "failed") merged.phase = "failed";
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

function isLifecycleUpdate(event: TaskEventView): boolean {
  const value = rawValue(event);
  if (typeof value !== "object" || value === null) return false;
  const type = (value as Record<string, unknown>).type;
  return type === "tool_execution_update" || type === "item.updated";
}

function parentActionId(event: TaskEventView): string | undefined {
  if (event.parentActionId) return event.parentActionId;
  const value = rawValue(event);
  if (typeof value !== "object" || value === null) return undefined;
  const object = value as Record<string, unknown>;
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
    default:
      return value;
  }
}

function clip(value: string, limit: number): string {
  const chars = Array.from(value);
  if (chars.length <= limit) return value;
  return chars.slice(0, Math.max(limit - 3, 0)).join("") + "...";
}
