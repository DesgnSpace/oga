// The conversation transcript: request bubbles, work blocks, and replies.

import * as React from "react";
import { useShowTechnicalDetails } from "@/appearance";
import { CopyButton } from "@/components/atoms/CopyButton";
import { MarkdownContent } from "@/domain/markdown";
import { compositionContainsNode, compositionHasThinking, type ActivityComposition } from "@/domain/activity";
import { TraceVisibility, findRevealTarget, stripTransportMarkup, turnMarkerLabel, withoutThinking, type RevealRequest, type TraceRow, type TurnMarkerKind } from "@/domain/trace";
import { ChevronIcon, FollowUpIcon, ReplyIcon, ResponseIcon, SteerIcon } from "@/ui/icons";
import { AttachmentsRow } from "./Attachments";
import { ReviewContent } from "./CodeReview";
import { RawEventDetails, TraceRows } from "./Trace";
import type { Bubble, ResponseBlock, TranscriptItem, WorkSegment } from "./transcriptModel";

/** Whether anything in this transcript is worth turning "Show thinking" on for. */
export function transcriptHasThinking(items: TranscriptItem[]): boolean {
  return items.some((item) => item.type === "work" && compositionHasThinking(item.segment.composition));
}

const BUBBLE_MARKER_ICONS = {
  resume: FollowUpIcon,
  reply: ReplyIcon,
  steer: SteerIcon,
} satisfies Record<Exclude<Bubble["kind"], undefined>, React.ComponentType<{ size?: number }>>;

function TurnMarker({ kind, icon: Icon }: { kind: TurnMarkerKind; icon: React.ComponentType<{ size?: number }> }) {
  return (
    <span className={`transcript-turn-marker transcript-turn-marker-${kind}`}>
      <Icon size={14} />
      {turnMarkerLabel(kind)}
    </span>
  );
}

function itemKey(item: TranscriptItem): string {
  switch (item.type) {
    case "bubble":
      return `bubble:${item.bubble.id}`;
    case "work":
      return `work:${item.segment.id}`;
    case "response":
      return `response:${item.block.id}`;
    case "question":
      return `question:${item.block.id}`;
  }
}

const TranscriptBubble = React.memo(function TranscriptBubble({ bubble }: { bubble: Bubble }) {
  const full = bubble.text;
  const previewRef = React.useRef<HTMLDivElement>(null);
  const [hasMore, setHasMore] = React.useState<boolean>();
  const [open, setOpen] = React.useState(false);
  const showTechnicalDetails = useShowTechnicalDetails();

  React.useLayoutEffect(() => {
    if (open) return;
    const element = previewRef.current;
    if (element === null) return;
    const measure = () => setHasMore(element.scrollHeight > element.clientHeight + 1);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [open, full]);

  // The preview stays capped while closed even when it fits, so a bubble that
  // grows mid-run is still measured against the cap rather than against itself.
  const collapsed = !open;
  const overflowing = collapsed && hasMore === true;
  const expandFromPreview = (event: React.MouseEvent) => {
    if (!overflowing) return;
    if (event.target instanceof HTMLElement && event.target.closest("a, button")) return;
    setOpen(true);
  };

  return (
    <div className="transcript-row transcript-row-user">
      <div className="transcript-bubble transcript-card copy-host">
        <CopyButton text={full} label="instruction" />
        {bubble.kind !== undefined && <TurnMarker kind={bubble.kind} icon={BUBBLE_MARKER_ICONS[bubble.kind]} />}
        <div
          ref={previewRef}
          className={`transcript-bubble-preview${collapsed ? " is-clamped" : ""}${overflowing ? " is-overflowing" : ""}`}
          onClick={expandFromPreview}
        >
          <MarkdownContent source={full} />
        </div>
        {hasMore === true && (
          <button
            className="transcript-bubble-toggle"
            type="button"
            aria-expanded={open}
            onClick={() => setOpen((value) => !value)}
          >
            {open ? "Show less" : "Show more"}
          </button>
        )}
        {showTechnicalDetails && bubble.rawText !== undefined && <RawEventDetails source={stripTransportMarkup(bubble.rawText)} />}
        {bubble.attachments && <AttachmentsRow taskId={bubble.attachments.taskId} paths={bubble.attachments.paths} />}
      </div>
    </div>
  );
});

const TranscriptResponse = React.memo(function TranscriptResponse({ block, question }: { block: ResponseBlock; question: boolean }) {
  const className = question
    ? "detail-response detail-response-question"
    : block.error
      ? "detail-response detail-response-error"
      : "detail-response";
  const marker = !question && block.marker !== undefined ? <TurnMarker kind={block.marker} icon={ResponseIcon} /> : null;

  if (block.text === undefined) {
    return (
      <div className="detail-empty-response">
        {marker}
        <h2>No response yet</h2>
        <p>The response will appear here.</p>
      </div>
    );
  }
  if (question) {
    return (
      <div className={`${className} copy-host`}>
        <CopyButton text={block.text} label="question" />
        <p className="detail-response-label detail-response-label-question">Waiting for your answer</p>
        <MarkdownContent source={block.text} />
      </div>
    );
  }
  if (block.error) {
    return (
      <div className={`${className} copy-host`}>
        <CopyButton text={block.text} label="error" />
        {marker}
        <p className="detail-response-label">{block.awaitingDecision ? "Waiting on your decision" : "The task failed"}</p>
        <ReviewContent source={block.text} language="plain" />
      </div>
    );
  }
  return (
    <div className={`${className} copy-host`}>
      <CopyButton text={block.text} label="response" />
      {marker}
      <MarkdownContent source={block.text} />
    </div>
  );
});

/**
 * The turn's header: a short status such as "Working · 24 min · step 1 of 5".
 * What the worker said stays in the feed below. A closed turn adds the gist
 * of what it did, since its feed is hidden.
 */
function workStatus(segment: WorkSegment, entries: WorkEntry[], open: boolean): string {
  const parts = [segment.live ? "Working" : "Worked"];
  if (segment.durationMs !== undefined) parts.push(durationWords(segment.durationMs));
  const step = stepProgress(segment.composition);
  if (step !== undefined) parts.push(step);
  const calls = entries.flatMap((entry) => (entry.type === "calls" ? entry.rows : []));
  if (!open && calls.length > 0) {
    const summary = callGroupSummary(calls);
    parts.push(summary.charAt(0).toLowerCase() + summary.slice(1));
  }
  return parts.join(" · ");
}

function durationWords(ms: number): string {
  const seconds = Math.max(Math.round(ms / 1_000), 1);
  if (seconds < 60) return `${seconds} sec`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  return `${Math.round(minutes / 60)} hr`;
}

/** The newest "Step 2 of 5" the worker wrote, as "step 2 of 5". */
function stepProgress(composition: ActivityComposition): string | undefined {
  for (let at = composition.blocks.length - 1; at >= 0; at -= 1) {
    const block = composition.blocks[at];
    if (block?.type !== "turn" || block.turn.title === undefined) continue;
    const match = /\bstep (\d+) of (\d+)\b/i.exec(block.turn.title);
    if (match) return `step ${match[1]} of ${match[2]}`;
  }
  return undefined;
}

type WorkEntry = { type: "message"; row: TraceRow } | { type: "calls"; rows: TraceRow[]; key: string };

function workEntries(rows: TraceRow[]): WorkEntry[] {
  const entries: WorkEntry[] = [];
  for (const row of rows) {
    if (row.style === "message") {
      entries.push({
        type: "message",
        row: row.children.length > 0
          ? { ...row, children: [], result: undefined, expansion: undefined, preview: undefined }
          : row,
      });
      for (const child of row.children) appendCallEntry(entries, child);
    } else {
      appendCallEntry(entries, row);
    }
  }
  return entries;
}

function appendCallEntry(entries: WorkEntry[], row: TraceRow): void {
  const previous = entries[entries.length - 1];
  if (previous?.type === "calls") previous.rows.push(row);
  else entries.push({ type: "calls", rows: [row], key: row.nodeId ?? String(row.id) });
}

type CallKind = "command" | "file-read" | "file-edit" | "search" | "tool";

function callKind(row: TraceRow): CallKind {
  if (row.state === "running") {
    if (row.event?.kind === "command") return "command";
    return "tool";
  }
  if (row.event?.kind === "command") return "command";
  if (row.event?.kind === "file") {
    const verb = row.verb?.toLowerCase();
    if (verb?.includes("read")) return "file-read";
    if (verb?.includes("search") || verb?.includes("found")) return "search";
    return "file-edit";
  }
  if (row.event?.kind === "tool") {
    const verb = row.verb?.toLowerCase();
    if (verb?.includes("read")) return "file-read";
    if (verb?.includes("search") || verb?.includes("found")) return "search";
    if (verb?.includes("edit") || verb?.includes("write")) return "file-edit";
  }
  return "tool";
}

/** Most telling first: what changed, then what ran, then what was looked at. */
const CALL_KIND_ORDER: CallKind[] = ["file-edit", "command", "file-read", "search", "tool"];

const CALL_KIND_WORDS: Record<CallKind, { done: string; running: string; one: string; many: (count: number) => string }> = {
  "file-edit": { done: "Edited", running: "Editing", one: "a file", many: (count) => `${count} files` },
  command: { done: "Ran", running: "Running", one: "a command", many: (count) => `${count} commands` },
  "file-read": { done: "Read", running: "Reading", one: "a file", many: (count) => `${count} files` },
  search: { done: "Searched", running: "Searching", one: "the code", many: (count) => `the code ${count} times` },
  tool: { done: "Used", running: "Using", one: "a tool", many: (count) => `${count} tools` },
};

/**
 * One short line for a run of tool calls: what is running now, or at most
 * the two most telling counts, such as "Edited 4 files, ran 11 commands".
 */
function callGroupSummary(rows: TraceRow[]): string {
  const running = [...rows].reverse().find((row) => row.state === "running");
  if (running !== undefined) {
    const words = CALL_KIND_WORDS[callKind(running)];
    return `${words.running} ${words.one}…`;
  }
  const counts = new Map<CallKind, number>();
  for (const row of rows) {
    const kind = callKind(row);
    counts.set(kind, (counts.get(kind) ?? 0) + 1);
  }
  const phrases = CALL_KIND_ORDER.filter((kind) => counts.has(kind)).slice(0, 2).map((kind, index) => {
    const words = CALL_KIND_WORDS[kind];
    const count = counts.get(kind) ?? 0;
    const verb = index === 0 ? words.done : words.done.toLowerCase();
    return `${verb} ${count === 1 ? words.one : words.many(count)}`;
  });
  return phrases.join(", ");
}

const TranscriptWork = React.memo(function TranscriptWork({
  segment,
  open,
  showThinking,
  onToggle,
  scrollRoot,
  rowExpansion,
  onRowExpansionChange,
  reveal,
}: {
  segment: WorkSegment;
  open: boolean;
  showThinking: boolean;
  onToggle: (id: number, startsExpanded: boolean) => void;
  scrollRoot?: React.RefObject<HTMLElement | null>;
  rowExpansion?: ReadonlyMap<string, boolean>;
  onRowExpansionChange?: (key: string, expanded: boolean) => void;
  reveal?: RevealRequest;
}) {
  const rows = React.useMemo(() => {
    const all = TraceVisibility.interleavedRows(segment.composition, segment.cwd, segment.live).filter(
      (row: TraceRow) => !row.isTechnical,
    );
    return showThinking ? all : withoutThinking(all);
  }, [segment.composition, segment.cwd, segment.live, showThinking]);
  // A reveal from the composer tray opens the run holding its row.
  React.useEffect(() => {
    if (reveal === undefined || open) return;
    if (!compositionContainsNode(segment.composition, reveal.nodeId)) return;
    onToggle(segment.id, segment.startsExpanded);
  }, [reveal, open, segment, onToggle]);
  if (rows.length === 0) return null;
  const entries = workEntries(rows);
  const status = workStatus(segment, entries, open);

  return (
    <div className="transcript-work">
      <button
        className="transcript-work-toggle"
        type="button"
        aria-expanded={open}
        aria-label={open ? "Hide worker steps" : "Show worker steps"}
        onClick={() => onToggle(segment.id, segment.startsExpanded)}
      >
        <span className="transcript-work-label" title={status}>{status}</span>
        <span className={`transcript-work-chevron${open ? " transcript-work-chevron-open" : ""}`} aria-hidden="true">
          <ChevronIcon size={14} />
        </span>
      </button>
      {open && (
        <div className="transcript-work-body">
          {entries.map((entry) => entry.type === "message" ? (
            <TraceRows rows={[entry.row]} cwd={segment.cwd} scrollRoot={scrollRoot} live={segment.live} reveal={reveal} key={`message:${entry.row.nodeId ?? entry.row.id}`} />
          ) : (
            <TranscriptCallGroup
              key={`calls:${entry.key}`}
              entry={entry}
              segment={segment}
              scrollRoot={scrollRoot}
              rowExpansion={rowExpansion}
              onRowExpansionChange={onRowExpansionChange}
              reveal={reveal}
            />
          ))}
        </div>
      )}
    </div>
  );
});

function TranscriptCallGroup({
  entry,
  segment,
  scrollRoot,
  rowExpansion,
  onRowExpansionChange,
  reveal,
}: {
  entry: Extract<WorkEntry, { type: "calls" }>;
  segment: WorkSegment;
  scrollRoot?: React.RefObject<HTMLElement | null>;
  rowExpansion?: ReadonlyMap<string, boolean>;
  onRowExpansionChange?: (key: string, expanded: boolean) => void;
  reveal?: RevealRequest;
}) {
  const key = `transcript-call-group:${segment.id}:${entry.key}`;
  // The saved map only seeds the first render: the controller does not
  // re-render on each toggle, so reading it afterwards would go stale.
  const [expanded, setExpanded] = React.useState(() => rowExpansion?.get(key) ?? false);
  const toggle = () => {
    setExpanded(!expanded);
    onRowExpansionChange?.(key, !expanded);
  };
  // A reveal from the composer tray opens the group holding its row.
  React.useEffect(() => {
    if (reveal === undefined || expanded) return;
    if (findRevealTarget(entry.rows, reveal.nodeId) === undefined) return;
    setExpanded(true);
    onRowExpansionChange?.(key, true);
  }, [reveal, expanded, entry.rows, key, onRowExpansionChange]);
  return (
    <div className="transcript-call-group">
      <button
        className="transcript-call-group-toggle"
        type="button"
        aria-expanded={expanded}
        onClick={toggle}
      >
        {callGroupSummary(entry.rows)}
        <span className={`transcript-call-group-chevron${expanded ? " is-open" : ""}`} aria-hidden="true"><ChevronIcon size={14} /></span>
      </button>
      {expanded && <TraceRows
        rows={entry.rows}
        cwd={segment.cwd}
        scrollRoot={scrollRoot}
        live={segment.live}
        expansionState={rowExpansion}
        onExpansionChange={onRowExpansionChange}
        reveal={reveal}
      />}
    </div>
  );
}

type NonWorkTranscriptItem = Exclude<TranscriptItem, { type: "work" }>;

function sameNonWorkItem(left: NonWorkTranscriptItem, right: NonWorkTranscriptItem): boolean {
  if (left.type === "bubble") {
    if (right.type !== "bubble") return false;
    const a = left.bubble;
    const b = right.bubble;
    return (
      a.id === b.id &&
      a.text === b.text &&
      a.at === b.at &&
      a.kind === b.kind &&
      a.rawText === b.rawText &&
      a.attachments?.taskId === b.attachments?.taskId &&
      a.attachments?.paths.join("\0") === b.attachments?.paths.join("\0")
    );
  }
  if (right.type === "bubble" || left.type !== right.type) return false;
  return (
    left.block.id === right.block.id &&
    left.block.text === right.block.text &&
    left.block.error === right.block.error &&
    left.block.awaitingDecision === right.block.awaitingDecision &&
    left.block.marker === right.block.marker
  );
}

const TranscriptRow = React.memo(function TranscriptRow({ item }: { item: NonWorkTranscriptItem }) {
  switch (item.type) {
    case "bubble":
      return <TranscriptBubble bubble={item.bubble} />;
    case "response":
      return <TranscriptResponse block={item.block} question={false} />;
    case "question":
      return <TranscriptResponse block={item.block} question={true} />;
  }
}, (left, right) => sameNonWorkItem(left.item, right.item));

export function Transcript({
  items,
  showThinking = false,
  scrollRoot,
  expansionState,
  onExpansionChange,
  rowExpansionState,
  onRowExpansionChange,
  reveal,
}: {
  items: TranscriptItem[];
  showThinking?: boolean;
  scrollRoot?: React.RefObject<HTMLElement | null>;
  expansionState?: ReadonlyMap<number, boolean>;
  onExpansionChange?: (id: number, expanded: boolean) => void;
  rowExpansionState?: ReadonlyMap<string, boolean>;
  onRowExpansionChange?: (key: string, expanded: boolean) => void;
  reveal?: RevealRequest;
}) {
  const [manualExpansion, setManualExpansion] = React.useState<Map<number, boolean>>(
    () => new Map(expansionState),
  );
  const toggleWork = React.useCallback((id: number, startsExpanded: boolean) => {
    setManualExpansion((current) => {
      const next = new Map(current);
      const expanded = !(current.get(id) ?? startsExpanded);
      next.set(id, expanded);
      onExpansionChange?.(id, expanded);
      return next;
    });
  }, [onExpansionChange]);

  return (
    <div className="transcript">
      {items.map((item) => (
        item.type === "work" ? (
          <TranscriptWork
            segment={item.segment}
            showThinking={showThinking}
            open={manualExpansion.get(item.segment.id) ?? item.segment.startsExpanded}
            onToggle={toggleWork}
            scrollRoot={scrollRoot}
            rowExpansion={rowExpansionState}
            onRowExpansionChange={onRowExpansionChange}
            reveal={reveal}
            key={itemKey(item)}
          />
        ) : (
          <TranscriptRow item={item} key={itemKey(item)} />
        )
      ))}
    </div>
  );
}
