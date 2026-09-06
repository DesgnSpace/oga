// Flat, readable rows for the task activity trace.
// Ported from rust/crates/oga-ui/src/trace/mod.rs — keep behavior identical.
// The syntax-highlighted rendering (`ReviewContent`, `CodeLanguage` highlighting) is
// out of scope here — this module only ports the pure row/expansion composition.

import type { EventKind, TaskEventView } from "@/bridge/types";
import { fileChangeFromRaw, type FileChange } from "@/domain/changes";
import {
  ActivityBlock,
  ActivityGroup,
  ActivityGroupStatus,
  ChapterRow,
  chapterCallCount,
  chapterDurationMs,
  groupDurationMs,
  groupId,
  groupStartsExpanded,
  groupStatus,
  ReasoningPulse,
  type ActivityComposition,
  type HandoffBoundary,
} from "@/domain/activity";

export type TraceStyle = "work" | "message" | "notice";
export type TraceState = "running" | "needs-input" | "failed" | "done";

/** What kind of turn a row opens or closes: a resumed run, an answer to a
 * question, a mid-run steer, a handoff to another profile, or the worker's
 * own reply to any of those. */
export type TurnMarkerKind = "resume" | "reply" | "steer" | "handoff" | "response";

const TURN_MARKER_LABELS = {
  resume: "Follow-up",
  reply: "Reply",
  steer: "Instruction",
  handoff: "Handed off",
  response: "Response",
} satisfies Record<TurnMarkerKind, string>;

export function turnMarkerLabel(kind: TurnMarkerKind): string {
  return TURN_MARKER_LABELS[kind];
}

/** The turn boundary a raw event announces, read off the same titles the
 * broker already gives these events — nothing new to track. */
export function turnMarkerForEvent(event: TaskEventView): TurnMarkerKind | undefined {
  switch (event.title) {
    case "Follow-up queued":
    case "Follow-up started":
    case "Resumed":
      return "resume";
    case "Question answered":
      return "reply";
    case "Instruction sent":
      return "steer";
    case "Handed off":
      return "handoff";
    default:
      return undefined;
  }
}

export type TodoItemStatus = "pending" | "in_progress" | "completed";

export interface TodoItem {
  text: string;
  status: TodoItemStatus;
}

/** A minimal language tag for the code an expansion carries — enough to pick a
 * class name; syntax highlighting itself is rendering, not domain logic. */
export type CodeLanguage =
  | "plain"
  | "rust"
  | "typescript"
  | "tsx"
  | "javascript"
  | "json"
  | "markdown"
  | "yaml"
  | "toml"
  | "shell"
  | "python"
  | "css"
  | "html"
  | "sql"
  | "swift"
  | "go"
  | "ruby"
  | "java"
  | "kotlin"
  | "php"
  | "c"
  | "cpp"
  | "csharp";

export function codeLanguageFromPath(path: string | undefined): CodeLanguage {
  const name = path?.split("/").pop();
  const extension = name?.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : undefined;
  switch (extension) {
    case "rs":
      return "rust";
    case "ts":
      return "typescript";
    case "tsx":
      return "tsx";
    case "js":
    case "mjs":
    case "cjs":
      return "javascript";
    case "jsx":
      return "tsx";
    case "json":
    case "jsonl":
      return "json";
    case "md":
    case "markdown":
      return "markdown";
    case "yml":
    case "yaml":
      return "yaml";
    case "toml":
      return "toml";
    case "sh":
    case "bash":
    case "zsh":
    case "fish":
      return "shell";
    case "py":
      return "python";
    case "css":
    case "scss":
      return "css";
    case "html":
    case "htm":
      return "html";
    case "sql":
      return "sql";
    case "swift":
      return "swift";
    case "go":
      return "go";
    case "rb":
      return "ruby";
    case "java":
      return "java";
    case "kt":
    case "kts":
      return "kotlin";
    case "php":
      return "php";
    case "c":
    case "h":
      return "c";
    case "cc":
    case "cpp":
    case "cxx":
    case "hh":
    case "hpp":
    case "hxx":
      return "cpp";
    case "cs":
      return "csharp";
    default:
      return "plain";
  }
}

/** A file preview needs more than syntax highlighting: an image renders as
 * an image, a markdown file renders as prose. Everything else keeps the
 * plain-text treatment `content` already had. */
export type ContentPreview = { kind: "image"; dataUrl?: string } | { kind: "markdown" };

const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico"]);
const MARKDOWN_EXTENSIONS = new Set(["md", "markdown", "mdx"]);

function previewKindFromPath(path: string | undefined): "image" | "markdown" | undefined {
  const name = path?.split("/").pop();
  const extension = name?.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : undefined;
  if (extension && IMAGE_EXTENSIONS.has(extension)) return "image";
  if (extension && MARKDOWN_EXTENSIONS.has(extension)) return "markdown";
  return undefined;
}

export type EventExpansion =
  | { type: "changes"; change: FileChange }
  | { type: "command"; command?: string; output?: string }
  | { type: "skill"; text: string }
  | { type: "prose"; text: string }
  | { type: "thinking"; text: string }
  | { type: "detail"; text: string }
  | { type: "content"; text: string; hiddenLines: number; language: CodeLanguage; preview?: ContentPreview }
  | { type: "todo"; items: TodoItem[] }
  | { type: "payload"; text: string };

export function expansionFromEvent(event: TaskEventView): EventExpansion | undefined {
  const raw = event.rawText;
  if (isProse(event)) {
    const text = eventText(event);
    return text !== undefined ? { type: "prose", text } : undefined;
  }

  if (event.presentation?.type === "todo" && raw) {
    const items = todoItems(raw);
    if (items) return { type: "todo", items };
  }

  if (raw) {
    if (event.kind === "tool" && event.title === "Load skill") {
      const instructions = findText(raw, ["output"]);
      if (instructions !== undefined) return { type: "skill", text: instructions };
    }

    const change = fileChangeFromRaw(raw);
    if (change) return { type: "changes", change };
  }

  const command = event.presentation?.type === "command" ? event.presentation.command : undefined;
  if (event.kind === "command" && raw) {
    const output = commandOutput(raw);
    if (command !== undefined || output !== undefined) {
      return { type: "command", command, output };
    }
  } else if (event.kind === "command" && command !== undefined) {
    return { type: "command", command, output: undefined };
  }

  if (
    (event.kind === "error" || event.kind === "lifecycle") &&
    event.detail !== undefined &&
    (event.title === "Resumed" || isLong(event.detail))
  ) {
    return { type: "detail", text: event.detail };
  }

  if (raw) {
    const keys =
      event.kind === "file"
        ? ["content", "text", "file_text", "fileText", "output", "result"]
        : ["tool_response", "result", "output", "matches", "content"];
    const text = findText(raw, keys);
    const path = event.presentation?.path;
    const previewKind = event.kind === "file" ? previewKindFromPath(path) : undefined;
    const imageDataUrl = previewKind === "image" ? imageDataUrlFromRaw(raw) : undefined;
    const imageReadFailed = previewKind === "image" && imageDataUrl === undefined && /"status"\s*:\s*"error"/.test(raw);
    if (text !== undefined || imageDataUrl !== undefined || (previewKind === "image" && !imageReadFailed)) {
      const preview: ContentPreview | undefined = previewKind === "image"
        ? { kind: "image", dataUrl: imageDataUrl }
        : previewKind === "markdown" && text !== undefined
          ? { kind: "markdown" }
          : undefined;
      return {
        type: "content",
        hiddenLines: text !== undefined ? hiddenLines(text) : 0,
        language: codeLanguageFromPath(path),
        text: text !== undefined ? capLines(text, 200) : "",
        preview,
      };
    }
  }

  const failure = failureText(event);
  if (failure !== undefined && isLong(failure)) return { type: "detail", text: failure };

  if (raw && raw !== "" && !designedKind(event.kind)) {
    return { type: "payload", text: stripTransportMarkup(raw) };
  }
  return undefined;
}

export function expansionOffers(expansion: EventExpansion, event: TaskEventView): boolean {
  if (event.kind === "lifecycle" && (event.title === "Retrying" || event.title.startsWith("Retry "))) {
    return false;
  }
  switch (expansion.type) {
    case "changes":
    case "content":
    case "todo":
      return true;
    case "command": {
      const failure = failureText(event);
      return (
        event.presentation?.text !== undefined ||
        expansion.output !== undefined ||
        (expansion.command !== undefined && expansion.command.includes("\n")) ||
        (failure !== undefined && isLong(failure))
      );
    }
    case "skill":
      return true;
    // Prose is the message itself; the row shows it whole rather than hiding it
    // behind a truncated copy of its own first line.
    case "prose":
      return false;
    case "thinking":
      return true;
    case "detail":
      return event.title === "Resumed" || isLong(expansion.text);
    case "payload":
      return true;
  }
}

export function expansionLabel(expansion: EventExpansion, expanded: boolean): string {
  const verb = expanded ? "Hide" : "Show";
  const noun = (() => {
    switch (expansion.type) {
      case "changes":
        return "changes";
      case "command":
        return expansion.output !== undefined ? "output" : "full command";
      case "skill":
        return "instructions";
      case "prose":
      case "thinking":
        return "full text";
      case "detail":
        return "full details";
      case "content":
        return "preview";
      case "todo":
        return "plan";
      case "payload":
        return "raw details";
    }
  })();
  return `${verb} ${noun}`;
}

export interface TraceRow {
  id: number;
  style: TraceStyle;
  verb?: string;
  target?: string;
  result?: string;
  state: TraceState;
  event?: TaskEventView;
  expansion?: EventExpansion;
  preview?: string;
  children: TraceRow[];
  startsExpanded: boolean;
  isStepStart: boolean;
  isTechnical: boolean;
  /** Set on the rows "Show thinking" governs. */
  isThinking?: boolean;
  /** Set on rows that open or close a turn boundary — a follow-up, a reply,
   * a steer, a handoff, or the worker's response to one. */
  marker?: TurnMarkerKind;
}

export function traceRowWeight(row: TraceRow): number {
  return 1 + row.children.reduce((sum, child) => sum + traceRowWeight(child), 0);
}

export function traceRowOffersExpansion(row: TraceRow): boolean {
  if (row.children.length > 0) return true;
  if (!row.expansion) return false;
  // A stretch of thinking is folded from many events, so it has no single one
  // of its own to judge.
  if (row.expansion.type === "thinking") return true;
  if (!row.event) return false;
  return expansionOffers(row.expansion, row.event);
}

/** Drops the thinking rows, including any nested under a group or chapter. */
export function withoutThinking(rows: TraceRow[]): TraceRow[] {
  return rows
    .filter((row) => row.isThinking !== true)
    .map((row) => (row.children.length > 0 ? { ...row, children: withoutThinking(row.children) } : row));
}

export function traceRowIsEmpty(row: TraceRow): boolean {
  return (
    !row.target &&
    !row.verb &&
    !row.result &&
    !row.preview &&
    row.children.length === 0 &&
    !traceRowOffersExpansion(row)
  );
}

export const TraceRowBuilder = {
  rows(blocks: ActivityBlock[], cwd: string, live: boolean): TraceRow[] {
    return blocks.flatMap((block, index) => {
      const rows = blockRows(block, cwd, live)
        .map((row) => dropEmpty(row))
        .filter((row): row is TraceRow => row !== undefined);
      if (index > 0 && rows[0]) rows[0] = { ...rows[0], isStepStart: true };
      return rows;
    });
  },
  chapterRows(row: ChapterRow, cwd: string, live: boolean): TraceRow[] {
    return chapterRows(row, cwd, live);
  },
};

function blockRows(block: ActivityBlock, cwd: string, live: boolean): TraceRow[] {
  switch (block.type) {
    case "chapter":
      return chapterBlockRows(block, cwd, live);
    case "reasoning":
      return [thinkingRow(block.pulse)];
    case "signal":
      return [signalRow(block.event, cwd)];
    case "receipt":
      return [receiptRow(block.event, block.thinkingTokens, block.usageWindow)];
    case "handoff":
      return [handoffRow(block.boundary)];
  }
}

type ChapterBlock = Extract<ActivityBlock, { type: "chapter" }>;

/**
 * A chapter the worker announced becomes one collapsed heading over the work
 * that followed it. A single call needs no heading of its own: the words the
 * worker said label that one row instead of nesting it.
 */
function chapterBlockRows(block: ChapterBlock, cwd: string, live: boolean): TraceRow[] {
  const rows = block.rows.flatMap((row) => chapterRows(row, cwd, live));
  const title = block.title;
  if (title === undefined) return rows;
  if (rows.length === 0) return [narrationRow(block.id, title)];
  const single = rows.length === 1 && block.rows[0]?.type === "work" ? rows[0] : undefined;
  if (single) {
    return [{ ...single, id: block.id, target: title, preview: single.target ?? single.preview }];
  }
  return [
    {
      ...narrationRow(block.id, title),
      result: chapterSummary(block.rows),
      state: chapterState(rows, live),
      children: rows,
    },
  ];
}

function chapterSummary(rows: ChapterRow[]): string | undefined {
  const calls = chapterCallCount(rows);
  const duration = formatDuration(chapterDurationMs(rows));
  const parts = [calls > 0 ? `${calls} call${calls === 1 ? "" : "s"}` : undefined, duration];
  const summary = parts.filter((part): part is string => part !== undefined).join(" · ");
  return summary !== "" ? summary : undefined;
}

function chapterState(rows: TraceRow[], live: boolean): TraceState {
  if (rows.some((row) => row.state === "needs-input")) return "needs-input";
  if (rows.some((row) => row.state === "failed")) return "failed";
  return live && rows.some((row) => row.state === "running") ? "running" : "done";
}

function narrationRow(id: number, title: string): TraceRow {
  return {
    id,
    style: "notice",
    verb: undefined,
    target: title,
    result: undefined,
    state: "done",
    event: undefined,
    expansion: undefined,
    preview: undefined,
    children: [],
    startsExpanded: false,
    isStepStart: false,
    isTechnical: false,
  };
}

function chapterRows(row: ChapterRow, cwd: string, live: boolean): TraceRow[] {
  switch (row.type) {
    case "work":
      return [workRow(row.event, cwd, live)];
    case "reasoning":
      return [thinkingRow(row.pulse)];
    case "group":
      return [groupRow(row.group, cwd, live)];
  }
}

export const TraceVisibility = {
  interleavedRows,
};

function interleavedRows(composition: ActivityComposition, cwd: string, live: boolean): TraceRow[] {
  const main = TraceRowBuilder.rows(composition.blocks, cwd, live);
  const technicalBlocks: ActivityBlock[] = composition.technical.map((event) => ({
    type: "chapter",
    id: event.id,
    rows: [{ type: "work", event }],
  }));
  const technical = TraceRowBuilder.rows(technicalBlocks, cwd, false).map((row) => ({
    ...row,
    isTechnical: true,
    isStepStart: false,
  }));
  return [...main, ...technical].sort((left, right) => left.id - right.id);
}


export const ActivityFormat = {
  count(value: number): string {
    if (value >= 999_500) return `${(value / 1_000_000).toFixed(1)}M`;
    if (value >= 1_000) return `${Math.round(value / 1_000)}k`;
    return String(value);
  },
  cost(value: number): string {
    if (value === 0) return "$0.00";
    if (value < 0.0001) return "~$0";
    if (value >= 0.01) return `$${value.toFixed(2)}`;
    return `$${value.toFixed(4)}`;
  },
  /** For durations already measured in whole seconds — no false decimal. */
  seconds(value: number): string {
    return value < 10 ? `${value}s` : ActivityFormat.duration(value * 1_000);
  },
  duration(milliseconds: number): string {
    if (milliseconds < 1_000) return `${milliseconds}ms`;
    const seconds = milliseconds / 1_000;
    if (seconds < 60) {
      return seconds < 10 ? `${seconds.toFixed(1)}s` : `${Math.round(seconds)}s`;
    }
    const minutes = Math.trunc(seconds / 60);
    if (minutes < 60) return `${minutes}m ${Math.trunc(seconds) % 60}s`;
    return `${Math.trunc(minutes / 60)}h ${minutes % 60}m`;
  },
};

export type ExpansionScope = "row" | "group";

export interface ExpansionKey {
  scope: ExpansionScope;
  id: number;
}

function groupRow(group: ActivityGroup, cwd: string, live: boolean): TraceRow {
  let style: TraceStyle;
  let verb: string | undefined;
  let target: string | undefined;
  let result: string | undefined;
  let event: TaskEventView | undefined;
  let expansion: EventExpansion | undefined;

  if (group.kind === "run") {
    style = "work";
    // The label is already a sentence — "Read 8 files", "Checked lint, tests".
    verb = undefined;
    target = group.runLabel;
    result = formatDuration(groupDurationMs(group));
    event = undefined;
    expansion = undefined;
  } else if (group.kind === "turn") {
    style = "notice";
    verb = undefined;
    target = group.turnTitle ?? "Work step";
    result = formatDuration(groupDurationMs(group));
    event = undefined;
    expansion = undefined;
  } else if (group.kind === "subagent") {
    style = "notice";
    verb = undefined;
    target = group.runLabel;
    result = formatDuration(groupDurationMs(group));
    event = undefined;
    expansion = undefined;
  } else {
    const row = workRow(group.anchor, cwd, live);
    style = row.style;
    verb = row.verb;
    target = row.target;
    result = row.result;
    event = row.event;
    expansion = row.expansion;
  }

  const children =
    group.kind === "lifecycle"
      ? group.members
          .filter((member) => member.id !== groupId(group))
          .map((member) => workRow(member, cwd, live))
          .map((row) => dropEmpty(row))
          .filter((row): row is TraceRow => row !== undefined)
      : group.children
          .flatMap((row) => chapterRows(row, cwd, live))
          .map((row) => dropEmpty(row))
          .filter((row): row is TraceRow => row !== undefined);

  return {
    id: groupId(group),
    style,
    verb,
    target,
    result,
    state: topState(groupStatus(group), live),
    event,
    expansion,
    preview: undefined,
    children,
    startsExpanded: groupStartsExpanded(group),
    isStepStart: false,
    isTechnical: false,
  };
}

function workRow(event: TaskEventView, cwd: string, live: boolean): TraceRow {
  const expansion = expansionFromEvent(event);
  const verb = inferVerb(event);
  const state = eventState(event, live);
  let style: TraceStyle;
  let target: string | undefined;
  let result: string | undefined;
  let preview: string | undefined;

  if (event.kind === "message") {
    style = "message";
    target = eventText(event);
    result = undefined;
    preview = undefined;
  } else if (event.kind === "command") {
    style = "work";
    const presentation = event.presentation?.type === "command" ? event.presentation : undefined;
    const output = expansion?.type === "command" ? expansion.output : undefined;
    preview = state !== "failed" ? (output !== undefined ? singleLine(output) : undefined) : undefined;
    // The command the run actually issued, not the shell around it — the whole
    // line is one click away in the terminal below.
    const subject =
      presentation?.text ??
      presentation?.command ??
      event.target ??
      event.detail ??
      (event.title.trim() !== "" ? event.title : undefined);
    target = subject !== undefined ? relativePaths(subject, cwd) : undefined;
    result = workResult(event, cwd);
  } else if (event.kind === "file" || event.kind === "retry") {
    style = "work";
    const path = event.presentation?.path ?? event.target ?? event.detail;
    target = path !== undefined ? relative(path, cwd) : undefined;
    result = workResult(event, cwd);
    preview = undefined;
  } else {
    style = "work";
    const text = event.presentation?.text ?? event.target ?? event.detail ?? (event.title.trim() !== "" ? event.title : undefined);
    target = text !== undefined ? relativePaths(text, cwd) : undefined;
    result = workResult(event, cwd);
    preview = undefined;
  }

  return {
    id: event.id,
    style,
    verb,
    target,
    result,
    state,
    event,
    expansion,
    preview,
    children: [],
    startsExpanded: false,
    isStepStart: false,
    isTechnical: false,
    marker: turnMarkerForEvent(event),
  };
}

function thinkingRow(pulse: ReasoningPulse): TraceRow {
  const label = pulse.seconds !== undefined ? `Thought for ${ActivityFormat.seconds(pulse.seconds)}` : "Thinking";
  const first = pulse.text?.split("\n").find((line) => line.trim() !== "")?.trim();
  return {
    id: pulse.id,
    style: "message",
    verb: undefined,
    target: label,
    result: pulse.tokens !== undefined ? `~${ActivityFormat.count(pulse.tokens)} tokens` : undefined,
    state: "done",
    event: undefined,
    expansion: pulse.text !== undefined ? { type: "thinking", text: pulse.text } : undefined,
    preview: first,
    children: [],
    startsExpanded: false,
    isStepStart: false,
    isTechnical: false,
    isThinking: true,
  };
}

function signalRow(event: TaskEventView, cwd: string): TraceRow {
  const failed = event.phase === "failed" || event.presentation?.level === "error";
  const text = eventText(event);
  return {
    id: event.id,
    style: "notice",
    verb: undefined,
    target: text !== undefined ? relativePaths(text, cwd) : undefined,
    result: undefined,
    state: failed ? "failed" : "done",
    event,
    expansion: expansionFromEvent(event),
    preview: undefined,
    children: [],
    startsExpanded: false,
    isStepStart: false,
    isTechnical: false,
    marker: turnMarkerForEvent(event),
  };
}

function receiptRow(event: TaskEventView, thinkingTokens: number, usageWindow: string | undefined): TraceRow {
  let text = event.phase === "failed" ? "Run failed" : "Run complete";
  const presentation = event.presentation;
  const stats: string[] = [];
  if (presentation) {
    if (presentation.costUsd !== undefined) stats.push(ActivityFormat.cost(presentation.costUsd));
    if (presentation.turns !== undefined) {
      stats.push(`${presentation.turns} turn${presentation.turns === 1 ? "" : "s"}`);
    }
    if (presentation.durationMs !== undefined) stats.push(ActivityFormat.duration(presentation.durationMs));
    if (presentation.tokensOut !== undefined) stats.push(`${presentation.tokensOut.toLocaleString()} tokens out`);
    if (thinkingTokens > 0) stats.push(`~${thinkingTokens.toLocaleString()} thinking`);
  }
  if (usageWindow !== undefined) stats.push(usageWindow);
  if (stats.length > 0) text += ` · ${stats.join(" · ")}`;
  return {
    id: event.id,
    style: "notice",
    verb: undefined,
    target: text,
    result: undefined,
    state: event.phase === "failed" ? "failed" : "done",
    event: undefined,
    expansion: undefined,
    preview: undefined,
    children: [],
    startsExpanded: false,
    isStepStart: false,
    isTechnical: false,
  };
}

function handoffRow(boundary: HandoffBoundary): TraceRow {
  const runs = boundary.earlierRuns.length === 1 ? "run" : "runs";
  const events = boundary.hiddenEventCount === 1 ? "event" : "events";
  return {
    id: 0,
    style: "notice",
    verb: undefined,
    target: `${boundary.chain} · ${boundary.earlierRuns.length} earlier ${runs} · ${boundary.hiddenEventCount} ${events}`,
    result: undefined,
    state: "done",
    event: undefined,
    expansion: undefined,
    preview: undefined,
    children: [],
    startsExpanded: false,
    isStepStart: true,
    isTechnical: false,
    marker: "handoff",
  };
}

function dropEmpty(row: TraceRow): TraceRow | undefined {
  const children = row.children.map(dropEmpty).filter((child): child is TraceRow => child !== undefined);
  const next = { ...row, children };
  return traceRowIsEmpty(next) ? undefined : next;
}

function topState(status: ActivityGroupStatus, live: boolean): TraceState {
  if (status === "running" && live) return "running";
  if (status === "needs_input") return "needs-input";
  if (status === "failed") return "failed";
  return "done";
}

function eventState(event: TaskEventView, live: boolean): TraceState {
  if (event.phase === "failed") return "failed";
  if (event.phase === "started" && live) return "running";
  return "done";
}

function formatDuration(durationMs: number | undefined): string | undefined {
  return durationMs !== undefined ? ActivityFormat.duration(durationMs) : undefined;
}

function inferVerb(event: TaskEventView): string | undefined {
  if (event.verb === "Ran" && event.kind !== "command" && event.presentation?.type !== "command") return undefined;
  return event.verb !== undefined && event.verb !== "" ? event.verb : undefined;
}

function eventText(event: TaskEventView): string | undefined {
  const text = event.presentation?.text ?? event.detail;
  return text !== undefined && text.trim() !== "" ? text : undefined;
}

function singleLine(value: string): string | undefined {
  const lines = value
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
  return lines.length === 1 ? lines[0] : undefined;
}

function workResult(event: TaskEventView, cwd: string): string | undefined {
  if (event.phase === "failed") {
    const text = event.result ?? event.presentation?.outcome ?? event.detail;
    return text !== undefined ? relativePaths(text, cwd) : undefined;
  }
  if (event.kind === "command") return undefined;
  const outcome = event.result ?? event.presentation?.outcome;
  if (outcome === undefined || defaultSuccess(outcome)) return undefined;
  return relativePaths(outcome, cwd);
}

function defaultSuccess(value: string): boolean {
  const normalized = value.trim().replace(/\.+$/, "").toLowerCase();
  return [
    "",
    "completed",
    "success",
    "succeeded",
    "ok",
    "edit applied successfully",
    "successfully edited",
    "done",
    "no result reported",
  ].includes(normalized);
}

function isProse(event: TaskEventView): boolean {
  return event.kind === "message";
}

function failureText(event: TaskEventView): string | undefined {
  if (event.phase !== "failed") return undefined;
  const text = event.result ?? event.presentation?.outcome ?? event.detail;
  return text !== undefined && text !== "" ? text : undefined;
}

function isLong(text: string): boolean {
  const lines = text.split("\n");
  return lines.length > 1 || Array.from(lines[0] ?? text).length > 140;
}

function designedKind(kind: EventKind): boolean {
  return (["file", "command", "tool", "error", "lifecycle", "message", "reasoning"] as EventKind[]).includes(kind);
}

function stringValue(value: unknown): string | undefined {
  if (typeof value === "string") return value !== "" ? value : undefined;
  if (Array.isArray(value)) {
    const parts = value
      .map((entry) => {
        if (typeof entry === "string") return entry;
        if (typeof entry === "object" && entry !== null) return stringValue((entry as Record<string, unknown>).text);
        return undefined;
      })
      .filter((entry): entry is string => entry !== undefined);
    return parts.length > 0 ? parts.join("\n") : undefined;
  }
  return undefined;
}

function findValue(value: unknown, keys: string[]): unknown {
  if (Array.isArray(value)) {
    for (const entry of value) {
      const found = findValue(entry, keys);
      if (found !== undefined) return found;
    }
    return undefined;
  }
  if (typeof value === "object" && value !== null) {
    const object = value as Record<string, unknown>;
    for (const key of keys) {
      if (key in object) return object[key];
    }
    for (const child of Object.values(object)) {
      const found = findValue(child, keys);
      if (found !== undefined) return found;
    }
  }
  return undefined;
}

function findText(raw: string, keys: string[]): string | undefined {
  const value = parseJson(raw);
  if (value === undefined) return undefined;
  const text = stringValue(findValue(value, keys));
  return text !== undefined ? stripTransportWrappers(text) : undefined;
}

interface TaggedSection {
  attributes: boolean;
  start: number;
  contentStart: number;
  contentEnd: number;
  end: number;
}

interface TagToken {
  name: string;
  closing: boolean;
  selfClosing: boolean;
  attributes: boolean;
  start: number;
  end: number;
}

function stripTransportWrappers(text: string): string {
  const sections = topLevelSections(text);
  if (sections.length === 0) return text;

  if (sections.length > 1) {
    const section = sections[sections.length - 1];
    return trimWrapperBoundary(text.slice(section.contentStart, section.contentEnd));
  }

  const section = sections[0];
  const spansText = text.slice(0, section.start).trim() === "" && text.slice(section.end).trim() === "";
  if (!spansText || (!section.attributes && hasTag(text.slice(section.contentStart, section.contentEnd)))) return text;
  return trimWrapperBoundary(text.slice(section.contentStart, section.contentEnd));
}

function topLevelSections(text: string): TaggedSection[] {
  const sections: TaggedSection[] = [];
  let cursor = text.search(/\S/);
  while (cursor !== -1 && cursor < text.length) {
    const opening = tagAt(text, cursor);
    if (!opening || opening.closing) break;
    const closing = opening.selfClosing ? undefined : matchingTag(text, opening);
    const contentEnd = opening.selfClosing ? opening.end : closing?.start ?? text.length;
    const end = opening.selfClosing ? opening.end : closing?.end ?? text.length;
    sections.push({
      attributes: opening.attributes,
      start: opening.start,
      contentStart: opening.end,
      contentEnd,
      end,
    });
    if (opening.selfClosing || !closing) break;
    const next = text.slice(end).search(/\S/);
    cursor = next === -1 ? -1 : end + next;
  }
  return sections;
}

function tagAt(text: string, start: number): TagToken | undefined {
  if (text.charAt(start) !== "<") return undefined;
  let end = start + 1;
  let quote: string | undefined;
  while (end < text.length) {
    const character = text.charAt(end);
    if (quote !== undefined) {
      if (character === quote) quote = undefined;
    } else if (character === '"' || character === "'") {
      quote = character;
    } else if (character === ">") {
      break;
    }
    end++;
  }
  if (end >= text.length) return undefined;
  const body = text.slice(start + 1, end).trim();
  if (body === "" || body.startsWith("!") || body.startsWith("?")) return undefined;
  const closing = body.startsWith("/");
  const source = closing ? body.slice(1).trim() : body;
  const selfClosing = !closing && source.endsWith("/");
  const withoutSlash = selfClosing ? source.slice(0, -1).trim() : source;
  const match = /^[A-Za-z][A-Za-z0-9_.:-]*/.exec(withoutSlash);
  if (!match) return undefined;
  return {
    name: match[0],
    closing,
    selfClosing,
    attributes: !closing && withoutSlash.slice(match[0].length).trim() !== "",
    start,
    end: end + 1,
  };
}

function matchingTag(text: string, opening: TagToken): TagToken | undefined {
  const stack = [opening.name];
  let cursor = opening.end;
  while (cursor < text.length) {
    const start = text.indexOf("<", cursor);
    if (start === -1) return undefined;
    const token = tagAt(text, start);
    if (!token) {
      cursor = start + 1;
      continue;
    }
    if (token.closing) {
      if (stack[stack.length - 1] === token.name) stack.pop();
      if (stack.length === 0) return token;
    } else if (!token.selfClosing) {
      stack.push(token.name);
    }
    cursor = token.end;
  }
  return undefined;
}

function hasTag(text: string): boolean {
  for (let cursor = text.indexOf("<"); cursor !== -1; cursor = text.indexOf("<", cursor + 1)) {
    if (tagAt(text, cursor)) return true;
  }
  return false;
}

function trimWrapperBoundary(text: string): string {
  return text.replace(/^(?:\r\n|\n)/, "").replace(/(?:\r\n|\n)$/, "");
}

export function stripTransportMarkup(text: string): string {
  let result = "";
  let cursor = 0;
  while (cursor < text.length) {
    const start = text.indexOf("<", cursor);
    if (start === -1) return result + text.slice(cursor);
    const token = tagAt(text, start);
    if (!token) {
      result += text.slice(cursor, start + 1);
      cursor = start + 1;
      continue;
    }
    result += text.slice(cursor, start);
    cursor = token.end;
  }
  return result;
}

/** A provider's tool result for reading an image embeds the bytes right in
 * the event payload as a base64 content block — the same payload the row's
 * raw-event viewer already renders. That is the only place image bytes are
 * available; nothing here reaches back out to disk. */
function findImageDataUrl(value: unknown): string | undefined {
  if (Array.isArray(value)) {
    for (const entry of value) {
      const found = findImageDataUrl(entry);
      if (found !== undefined) return found;
    }
    return undefined;
  }
  if (typeof value === "object" && value !== null) {
    const object = value as Record<string, unknown>;
    const source = object.source as Record<string, unknown> | undefined;
    if (
      object.type === "image" &&
      source?.type === "base64" &&
      isBase64(source.data) &&
      typeof source.media_type === "string"
    ) {
      return `data:${source.media_type};base64,${source.data}`;
    }
    // Claude Code's hook payload carries the same bytes in its own shape: the
    // tool response holds a file with its own base64 and mime type rather than
    // an API content block.
    const file = object.file as Record<string, unknown> | undefined;
    if (object.type === "image" && isBase64(file?.base64) && typeof file?.type === "string") {
      return `data:${file.type};base64,${file.base64}`;
    }
    for (const child of Object.values(object)) {
      const found = findImageDataUrl(child);
      if (found !== undefined) return found;
    }
  }
  return undefined;
}

/**
 * Stored payloads cap long strings with an ellipsis, so a large image arrives
 * as a cut base64 body that no browser can decode. Only a whole body is
 * usable; anything else falls back to reading the file from disk.
 */
function isBase64(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9+/]*={0,2}$/.test(value);
}

function imageDataUrlFromRaw(raw: string): string | undefined {
  const value = parseJson(raw);
  if (value === undefined) return undefined;
  return findImageDataUrl(value);
}

function parseJson(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

function commandOutput(raw: string): string | undefined {
  return findText(raw, ["stdout", "stderr", "output"]) ?? findText(raw, ["tool_response"]);
}

function todoItems(raw: string): TodoItem[] | undefined {
  const find = (value: unknown): TodoItem[] | undefined => {
    if (Array.isArray(value)) {
      const items: TodoItem[] = [];
      let allMatched = true;
      for (const entry of value) {
        if (typeof entry !== "object" || entry === null) {
          allMatched = false;
          break;
        }
        const object = entry as Record<string, unknown>;
        const text = ["content", "text", "description", "activeForm"]
          .map((key) => stringValue(object[key]))
          .find((candidate) => candidate !== undefined);
        if (text === undefined) {
          allMatched = false;
          break;
        }
        const status: TodoItemStatus =
          object.status === "completed"
            ? "completed"
            : object.status === "in_progress"
              ? "in_progress"
              : object.completed === true
                ? "completed"
                : "pending";
        items.push({ text, status });
      }
      if (allMatched && items.length > 0) return items;
      for (const entry of value) {
        const found = find(entry);
        if (found) return found;
      }
      return undefined;
    }
    if (typeof value === "object" && value !== null) {
      for (const child of Object.values(value as Record<string, unknown>)) {
        const found = find(child);
        if (found) return found;
      }
    }
    return undefined;
  };
  const value = parseJson(raw);
  return value === undefined ? undefined : find(value);
}

function hiddenLines(value: string): number {
  return Math.max(value.split("\n").length - 200, 0);
}

function capLines(value: string, limit: number): string {
  return value.split("\n").slice(0, limit).join("\n");
}

function relative(path: string, cwd: string): string {
  const root = cwd.replace(/\/+$/, "");
  if (path === root) return ".";
  const prefix = `${root}/`;
  return path.startsWith(prefix) ? path.slice(prefix.length) : path;
}

export function relativePaths(text: string, cwd: string): string {
  const root = cwd.replace(/\/+$/, "");
  if (root === "" || root === "/") return text;
  if (text === root) return ".";
  return text.split(`${root}/`).join("");
}

export function middleTruncated(text: string, maxChars: number): string {
  const chars = Array.from(text);
  if (maxChars <= 2 || chars.length <= maxChars) return text;
  const usable = maxChars - 1;
  const head = Math.ceil(usable / 2);
  const tail = Math.floor(usable / 2);
  const prefix = chars.slice(0, head).join("");
  const suffix = chars.slice(chars.length - tail).join("");
  return `${prefix}…${suffix}`;
}
