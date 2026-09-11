// The conversation transcript: request/follow-up bubbles, collapsible work
// blocks, and the worker's replies.
// Ported from rust/crates/oga-ui/src/task_detail/mod.rs's view layer.

import * as React from "react";
import { MarkdownContent } from "@/domain/markdown";
import { compositionHasThinking, type ActivityComposition } from "@/domain/activity";
import { TraceVisibility, stripTransportMarkup, turnMarkerLabel, withoutThinking, type TraceRow, type TurnMarkerKind } from "@/domain/trace";
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

const TranscriptBubble = React.memo(function TranscriptBubble({ bubble, cwd }: { bubble: Bubble; cwd?: string }) {
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

  const collapsed = !open && hasMore === true;
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
        {bubble.rawText !== undefined && <RawEventDetails source={stripTransportMarkup(bubble.rawText)} />}
        <AttachmentsRow paths={bubble.attachments} cwd={cwd} />
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
});

function workLabel(segment: WorkSegment): string {
  const verb = segment.live ? "Working" : "Worked";
  return segment.durationMs !== undefined ? `${verb} for ${durationWords(segment.durationMs)}` : verb;
}

function durationWords(ms: number): string {
  const seconds = Math.max(Math.round(ms / 1_000), 1);
  if (seconds < 60) return `${seconds} second${seconds === 1 ? "" : "s"}`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? "" : "s"}`;
  const hours = Math.round(minutes / 60);
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
    if (block.type === "chapter" && block.title !== undefined) return block.title.replace(/\s+/g, " ").trim();
  }
  return undefined;
}

function traceRowCount(row: TraceRow): number {
  return Math.max(1, row.children.reduce((total, child) => total + traceRowCount(child), 0));
}

const TranscriptWork = React.memo(function TranscriptWork({
  segment,
  open,
  showThinking,
  onToggle,
  scrollRoot,
}: {
  segment: WorkSegment;
  open: boolean;
  showThinking: boolean;
  onToggle: (id: number, startsExpanded: boolean) => void;
  scrollRoot?: React.RefObject<HTMLElement | null>;
}) {
  const rows = React.useMemo(() => {
    const all = TraceVisibility.interleavedRows(segment.composition, segment.cwd, segment.live).filter(
      (row: TraceRow) => !row.isTechnical,
    );
    return showThinking ? all : withoutThinking(all);
  }, [segment.composition, segment.cwd, segment.live, showThinking]);
  if (rows.length === 0) return null;
  const summary = workSummary(segment, rows);
  const fullTitle = `${workLabel(segment)}${summary ? ` · ${summary}` : ""}`;

  return (
    <div className="transcript-work">
      <button
        className="transcript-work-toggle"
        type="button"
        aria-expanded={open}
        aria-label={open ? "Hide worker steps" : "Show worker steps"}
        onClick={() => onToggle(segment.id, segment.startsExpanded)}
      >
        <span className="transcript-work-label" title={fullTitle}>{fullTitle}</span>
        <span className={`transcript-work-chevron${open ? " transcript-work-chevron-open" : ""}`} aria-hidden="true">
          <ChevronIcon size={14} />
        </span>
      </button>
      {open && (
        <div className="transcript-work-body">
          <TraceRows rows={rows} cwd={segment.cwd} scrollRoot={scrollRoot} live={segment.live} />
        </div>
      )}
    </div>
  );
});

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
      a.attachments?.join("\0") === b.attachments?.join("\0")
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

const TranscriptRow = React.memo(function TranscriptRow({ item, cwd }: { item: NonWorkTranscriptItem; cwd?: string }) {
  switch (item.type) {
    case "bubble":
      return <TranscriptBubble bubble={item.bubble} cwd={cwd} />;
    case "response":
      return <TranscriptResponse block={item.block} question={false} />;
    case "question":
      return <TranscriptResponse block={item.block} question={true} />;
  }
}, (left, right) => left.cwd === right.cwd && sameNonWorkItem(left.item, right.item));

export function Transcript({
  items,
  cwd,
  showThinking = false,
  scrollRoot,
  expansionState,
  onExpansionChange,
}: {
  items: TranscriptItem[];
  cwd?: string;
  showThinking?: boolean;
  scrollRoot?: React.RefObject<HTMLElement | null>;
  expansionState?: ReadonlyMap<number, boolean>;
  onExpansionChange?: (id: number, expanded: boolean) => void;
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
            key={itemKey(item)}
          />
        ) : (
          <TranscriptRow item={item} cwd={cwd} key={itemKey(item)} />
        )
      ))}
    </div>
  );
}
