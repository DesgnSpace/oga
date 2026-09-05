// The conversation transcript: request/follow-up bubbles, collapsible work
// blocks, and the worker's replies.
// Ported from rust/crates/oga-ui/src/task_detail/mod.rs's view layer.

import * as React from "react";
import { MarkdownContent } from "@/domain/markdown";
import { compositionHasThinking, type ActivityComposition } from "@/domain/activity";
import { TraceVisibility, stripTransportMarkup, turnMarkerLabel, withoutThinking, type TraceRow, type TurnMarkerKind } from "@/domain/trace";
import { ChevronIcon, FollowUpIcon, ReplyIcon, ResponseIcon, SteerIcon } from "@/ui/icons";
import { ReviewContent } from "./CodeReview";
import { TraceRows, useShowThinking } from "./Trace";
import type { Bubble, ResponseBlock, TranscriptItem, WorkSegment } from "./transcriptModel";

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

function TranscriptBubble({ bubble }: { bubble: Bubble }) {
  const full = bubble.text;
  const previewRef = React.useRef<HTMLDivElement>(null);
  const [hasMore, setHasMore] = React.useState<boolean>();
  const [open, setOpen] = React.useState(false);

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

  const collapsed = !open && hasMore !== false;
  const expandFromPreview = (event: React.MouseEvent) => {
    if (!collapsed) return;
    if (event.target instanceof HTMLElement && event.target.closest("a, button")) return;
    setOpen(true);
  };

  return (
    <div className="transcript-row transcript-row-user">
      {bubble.kind !== undefined && <TurnMarker kind={bubble.kind} icon={BUBBLE_MARKER_ICONS[bubble.kind]} />}
      <div className="transcript-bubble">
        <div
          ref={previewRef}
          className={collapsed ? "transcript-bubble-preview is-clamped" : "transcript-bubble-preview"}
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
        {bubble.rawText !== undefined && (
          <details className="trace-raw-event">
            <summary>Show raw event</summary>
            <ReviewContent source={stripTransportMarkup(bubble.rawText)} language="json" />
          </details>
        )}
      </div>
    </div>
  );
}

function TranscriptResponse({ block, question }: { block: ResponseBlock; question: boolean }) {
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
      <div className={className}>
        <p className="detail-response-label detail-response-label-question">Waiting for your answer</p>
        <MarkdownContent source={block.text} />
      </div>
    );
  }
  if (block.error) {
    return (
      <div className={className}>
        {marker}
        <p className="detail-response-label">{block.awaitingDecision ? "Waiting on your decision" : "The task failed"}</p>
        <ReviewContent source={block.text} language="plain" />
      </div>
    );
  }
  return (
    <div className={className}>
      {marker}
      <MarkdownContent source={block.text} />
    </div>
  );
}

function workLabel(segment: WorkSegment): string {
  const verb = segment.live ? "Working" : "Worked";
  return segment.durationMs !== undefined ? `${verb} for ${durationWords(segment.durationMs)}` : verb;
}

function durationWords(ms: number): string {
  const seconds = Math.max(Math.trunc(ms / 1_000), 1);
  if (seconds < 60) return `${seconds} second${seconds === 1 ? "" : "s"}`;
  if (seconds < 3_600) {
    const minutes = Math.trunc(seconds / 60);
    return `${minutes} minute${minutes === 1 ? "" : "s"}`;
  }
  const hours = Math.trunc(seconds / 3_600);
  return `${hours} hour${hours === 1 ? "" : "s"}`;
}

function workSummary(segment: WorkSegment, rows: TraceRow[]): string {
  const narration = segmentNarration(segment.composition);
  if (narration) return narration;
  const count = rows.reduce((total, row) => total + traceRowCount(row), 0);
  return `${count} action${count === 1 ? "" : "s"}`;
}

function segmentNarration(composition: ActivityComposition): string | undefined {
  for (const block of composition.blocks) {
    if (block.type === "chapter" && block.title !== undefined) return shortSummary(block.title);
  }
  return undefined;
}

function shortSummary(value: string): string {
  const line = value.replace(/\s+/g, " ").trim();
  return line.length > 48 ? `${line.slice(0, 47)}…` : line;
}

function traceRowCount(row: TraceRow): number {
  return Math.max(1, row.children.reduce((total, child) => total + traceRowCount(child), 0));
}

function TranscriptWork({
  segment,
  open,
  showThinking,
  onToggle,
}: {
  segment: WorkSegment;
  open: boolean;
  showThinking: boolean;
  onToggle: () => void;
}) {
  const rows = React.useMemo(() => {
    const all = TraceVisibility.interleavedRows(segment.composition, segment.cwd, segment.live).filter(
      (row: TraceRow) => !row.isTechnical,
    );
    return showThinking ? all : withoutThinking(all);
  }, [segment.composition, segment.cwd, segment.live, showThinking]);
  if (rows.length === 0) return null;
  const summary = workSummary(segment, rows);

  return (
    <div className="transcript-work">
      <button className="transcript-work-toggle" type="button" aria-expanded={open} onClick={onToggle}>
        <span className="transcript-work-label">{workLabel(segment)}{summary ? ` · ${summary}` : ""}</span>
        <span className={`transcript-work-chevron${open ? " transcript-work-chevron-open" : ""}`} aria-hidden="true">
          <ChevronIcon size={14} />
        </span>
      </button>
      {open && (
        <div className="transcript-work-body">
          <TraceRows rows={rows} cwd={segment.cwd} />
        </div>
      )}
    </div>
  );
}

type NonWorkTranscriptItem = Exclude<TranscriptItem, { type: "work" }>;

function TranscriptRow({ item }: { item: NonWorkTranscriptItem }) {
  switch (item.type) {
    case "bubble":
      return <TranscriptBubble bubble={item.bubble} />;
    case "response":
      return <TranscriptResponse block={item.block} question={false} />;
    case "question":
      return <TranscriptResponse block={item.block} question={true} />;
  }
}

export function Transcript({ items }: { items: TranscriptItem[] }) {
  const [manualExpansion, setManualExpansion] = React.useState<Map<number, boolean>>(new Map());
  const [showThinking, toggleThinking] = useShowThinking();
  const hasThinking = items.some((item) => item.type === "work" && compositionHasThinking(item.segment.composition));

  return (
    <div className="transcript">
      {hasThinking && (
        <div className="transcript-options">
          <button className="text-button" type="button" aria-pressed={showThinking} onClick={toggleThinking}>
            {showThinking ? "Hide thinking" : "Show thinking"}
          </button>
        </div>
      )}
      {items.map((item) => (
        item.type === "work" ? (
          <TranscriptWork
            segment={item.segment}
            showThinking={showThinking}
            open={manualExpansion.get(item.segment.id) ?? item.segment.startsExpanded}
            onToggle={() =>
              setManualExpansion((current) => {
                const next = new Map(current);
                next.set(item.segment.id, !(current.get(item.segment.id) ?? item.segment.startsExpanded));
                return next;
              })
            }
            key={itemKey(item)}
          />
        ) : (
          <TranscriptRow item={item} key={itemKey(item)} />
        )
      ))}
    </div>
  );
}
