// The activity trace panel: flat rows, expansion, and windowing.
// Ported from rust/crates/oga-ui/src/trace/mod.rs's view layer — the pure
// row/expansion composition already lives in @/domain/trace.

import * as React from "react";
import type { TaskEventView } from "@/bridge/types";
import { readImagePreview } from "@/bridge";
import type { DiffKind, FileChange } from "@/domain/changes";
import { MarkdownContent } from "@/domain/markdown";
import { parseBlocks, parseInline } from "@/domain/markdown/parse";
import {
  expansionLabel,
  stripTransportMarkup,
  traceRowOffersExpansion,
  turnMarkerLabel,
  type EventExpansion,
  type TodoItem,
  type TraceRow,
  type TurnMarkerKind,
} from "@/domain/trace";
import { readStorage, writeStorage } from "@/state/storage";
import { Modal } from "@/components/primitives/Modal";
import { EmptyState } from "@/components/atoms/ListState";
import {
  DiffMarkIcon,
  DisclosureIcon,
  FollowUpIcon,
  HandoffIcon,
  CheckIcon,
  CloseIcon,
  ReplyIcon,
  ResponseIcon,
  SteerIcon,
} from "@/ui/icons";
import { SyntaxCode } from "@/components/SyntaxCode";
import { ReviewContent } from "./CodeReview";
import { DiffHeader } from "@/components/DiffHeader";
import { buildInlineDiffRanges, splitDiffBlock } from "@/lib/inline-diff";
import { useDiffView } from "@/state/diff-preferences";

type ContentExpansion = Extract<EventExpansion, { type: "content" }>;

interface OpenFilePreview {
  path?: string;
  cwd?: string;
  expansion: ContentExpansion;
  imageDataUrl?: string;
}

export function resolvePreviewPath(path: string, cwd?: string): string {
  if (path.startsWith("/") || !cwd) return path;
  return `${cwd.replace(/\/+$/, "")}/${path.replace(/^\/+/, "")}`;
}

function diffMark(kind: Exclude<DiffKind, "skipped">): React.ReactNode {
  return kind === "context" ? null : <DiffMarkIcon kind={kind} />;
}

/** An edit reads as one diff, not as a before block above an after block. */
function FileChangeBody({ change }: { change: FileChange }) {
  const [view] = useDiffView();
  const ranges = React.useMemo(() => buildInlineDiffRanges(change.blocks), [change.blocks]);
  return (
    <div className="trace-diff">
      <DiffHeader />
      {change.blocks.map((block, blockIndex) => (
        <React.Fragment key={blockIndex}>
          {view === "split" ? splitDiffBlock(block).map((row, rowIndex) => (
            <div className="diff-split-row" key={rowIndex}>
              {[row.before, row.after].map((line, side) => {
                const lineIndex = line ? block.indexOf(line) : -1;
                return <div className={`trace-diff-line trace-diff-${line?.kind ?? "empty"}`} key={side}>
                  {line && <SyntaxCode source={line.text} path={change.path} diffKind={line.kind === "added" || line.kind === "removed" ? line.kind : undefined} changedRange={lineIndex >= 0 && ranges[blockIndex][lineIndex] ? side === 0 ? ranges[blockIndex][lineIndex]?.before : ranges[blockIndex][lineIndex]?.after : undefined} />}
                </div>;
              })}
            </div>
          )) : block.map((line, lineIndex) =>
            line.kind === "skipped" ? (
              <div className="trace-diff-line trace-diff-skipped" key={lineIndex}>
                {line.text}
              </div>
            ) : (
              <div className={`trace-diff-line trace-diff-${line.kind}`} key={lineIndex}>
                <span className="trace-diff-mark" aria-hidden="true">
                  {diffMark(line.kind)}
                </span>
                <SyntaxCode
                  source={line.text}
                   path={change.path}
                   diffKind={line.kind === "context" ? undefined : line.kind}
                   changedRange={ranges[blockIndex][lineIndex] && line.kind === "removed" ? ranges[blockIndex][lineIndex]?.before : ranges[blockIndex][lineIndex]?.after}
                />
              </div>
            ),
          )}
        </React.Fragment>
      ))}
    </div>
  );
}

function ContentPreviewBody({
  expansion,
  path,
  cwd,
  onOpen,
}: {
  expansion: ContentExpansion;
  path: string | undefined;
  cwd?: string;
  onOpen: (path: string | undefined, imageDataUrl?: string) => void;
}) {
  const [diskImage, setDiskImage] = React.useState<string | null>(null);
  const [imageError, setImageError] = React.useState<string | null>(null);
  const payloadImage = expansion.preview?.kind === "image" ? expansion.preview.dataUrl : undefined;

  React.useEffect(() => {
    if (payloadImage || expansion.preview?.kind !== "image" || !path) return;
    let active = true;
    void readImagePreview(resolvePreviewPath(path, cwd)).then((result) => {
      if (!active) return;
      if (!result.ok) {
        setImageError(result.error.message);
        return;
      }
      const bytes = new Uint8Array(result.value.bytes);
      let binary = "";
      for (let offset = 0; offset < bytes.length; offset += 0x8000) {
        binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
      }
      setDiskImage(`data:${result.value.mime};base64,${btoa(binary)}`);
    });
    return () => {
      active = false;
    };
  }, [cwd, path, payloadImage, expansion.preview?.kind]);

  switch (expansion.preview?.kind) {
    case "image":
      if (imageError) return <p className="trace-file-preview-error" role="status">{imageError}</p>;
      if (!payloadImage && !diskImage) return <p className="trace-file-preview-status" role="status">Loading image…</p>;
      return (
        <button type="button" className="trace-file-preview trace-file-preview-image" onClick={() => onOpen(path, payloadImage ?? diskImage ?? undefined)}>
          <img src={payloadImage ?? diskImage ?? ""} alt={path ?? "Image preview"} />
        </button>
      );
    case "markdown":
      return (
        <button type="button" className="trace-file-preview trace-file-preview-markdown" onClick={() => onOpen(path)}>
          <MarkdownContent source={expansion.text} />
        </button>
      );
    default:
      return <ReviewContent source={expansion.text} language={expansion.language} path={path} />;
  }
}

function CommandTerminal({
  command,
  output,
  state,
}: {
  command?: string;
  output?: string;
  state: TraceRow["state"];
}) {
  const failed = state === "failed";
  const outputRef = React.useRef<HTMLPreElement>(null);
  const followOutput = React.useRef(true);

  React.useLayoutEffect(() => {
    const outputElement = outputRef.current;
    if (outputElement && followOutput.current) {
      outputElement.scrollTop = outputElement.scrollHeight;
    }
  }, [output]);

  return (
    <div className={`trace-terminal${failed ? " trace-terminal-failed" : ""}`}>
      <div className="trace-terminal-label">shell</div>
      {command !== undefined && (
        <div className="trace-terminal-command">
          <span className="trace-terminal-prompt" aria-hidden="true">
            $
          </span>
          <SyntaxCode source={command} language="shell" />
        </div>
      )}
      {output !== undefined && (
        <pre
          ref={outputRef}
          className="trace-terminal-output"
          onScroll={(event) => {
            const element = event.currentTarget;
            followOutput.current = element.scrollHeight - element.scrollTop - element.clientHeight <= 4;
          }}
        >
          {output}
        </pre>
      )}
      <div className="trace-terminal-outcome" role="status">
        {failed ? <CloseIcon size={14} /> : <CheckIcon size={14} />}
        {failed ? "Failed" : "Succeeded"}
      </div>
    </div>
  );
}

function ExpansionBody({
  expansion,
  path,
  cwd,
  failed,
  onOpenPreview,
}: {
  expansion: EventExpansion;
  path: string | undefined;
  cwd?: string;
  failed: boolean;
  onOpenPreview: (expansion: ContentExpansion, path: string | undefined, imageDataUrl?: string) => void;
}) {
  switch (expansion.type) {
    case "changes":
      return <FileChangeBody change={expansion.change} />;
    case "command":
      return (
        <div className="trace-expansion-command">
          <CommandTerminal command={expansion.command} output={expansion.output} state={failed ? "failed" : "done"} />
        </div>
      );
    case "skill":
      return <MarkdownContent source={expansion.text} />;
    case "prose":
    case "thinking":
      return <MarkdownContent source={expansion.text} />;
    case "detail":
      return <ReviewContent source={expansion.text} language="markdown" />;
    case "content":
      return (
        <div className="trace-expansion-content">
          <ContentPreviewBody expansion={expansion} path={path} cwd={cwd} onOpen={(p, imageDataUrl) => onOpenPreview(expansion, p, imageDataUrl)} />
          {expansion.hiddenLines > 0 && <small>{`${expansion.hiddenLines} more lines`}</small>}
        </div>
      );
      case "todo":
        return (
          <ul className="trace-todo-list">
            {expansion.items.map((item: TodoItem, index: number) => {
              const marker = item.status === "completed" ? "done" : item.status === "in_progress" ? "current" : "pending";
              const markerText = marker === "done" ? "[x]" : marker === "current" ? "[>]" : "[ ]";
              return (
                <li
                  className={`trace-todo-item trace-todo-${marker}`}
                  aria-label={`${item.status}: ${item.text}`}
                  key={index}
                >
                  <span className="trace-todo-marker" aria-hidden="true">
                    {markerText}
                  </span>
                  <span>{item.text}</span>
                </li>
              );
            })}
          </ul>
        );
    case "payload":
      return <ReviewContent source={expansion.text} language="json" />;
  }
}

function containsMarkdown(source: string): boolean {
  const parsed = parseBlocks(source);
  return parsed.truncated || parsed.blocks.some((block) => {
    if (block.type !== "paragraph") return true;
    return parseInline(block.text).some((node) => node.type !== "text");
  });
}

const TURN_MARKER_ICONS = {
  resume: FollowUpIcon,
  reply: ReplyIcon,
  steer: SteerIcon,
  handoff: HandoffIcon,
  response: ResponseIcon,
} satisfies Record<TurnMarkerKind, React.ComponentType<{ size?: number }>>;

function TraceTurnMarker({ kind }: { kind: TurnMarkerKind }) {
  const Icon = TURN_MARKER_ICONS[kind];
  return (
    <span className={`trace-turn-marker trace-turn-marker-${kind}`}>
      <Icon size={14} />
      {turnMarkerLabel(kind)}
    </span>
  );
}

function TraceTarget({ row }: { row: TraceRow }) {
  const prose = row.expansion?.type === "prose" ? row.expansion.text : undefined;
  const source = prose ?? row.target;
  if (source === undefined) return null;
  const presentationType = row.event?.presentation?.type;
  const technical = presentationType === "file" || presentationType === "command" || row.event?.verb === "Searched";
  const className = `trace-target${technical ? " trace-target-technical" : ""}`;
  if (prose === undefined || !containsMarkdown(source)) {
    return <span className={className}>{source}</span>;
  }
  return (
    <div className={className}>
      <MarkdownContent source={source} />
    </div>
  );
}

export function EventExpansionView({
  event,
  expansion,
  cwd,
  onOpenPreview,
}: {
  event: TaskEventView;
  expansion: EventExpansion;
  cwd?: string;
  onOpenPreview: (expansion: ContentExpansion, path: string | undefined, imageDataUrl?: string) => void;
}) {
  const hasPrimaryExpansion = expansion.type !== "payload";
  return (
    <div className={`trace-expansion trace-expansion-${event.kind}`}>
      <ExpansionBody
        expansion={expansion}
        path={event.presentation?.path}
        cwd={cwd}
        failed={event.phase === "failed"}
        onOpenPreview={onOpenPreview}
      />
      {hasPrimaryExpansion && event.rawText !== undefined && (
        <details className="trace-raw-event">
          <summary>Show raw event</summary>
          <ReviewContent source={stripTransportMarkup(event.rawText)} language="json" />
        </details>
      )}
    </div>
  );
}

interface TraceRowViewProps {
  row: TraceRow;
  /**
   * Where this row sits in the tree. A run group shares its id with the first
   * member it contains, so the path is what tells the two of them apart.
   */
  path: string;
  expanded: Map<string, boolean>;
  onToggle: (path: string, startsExpanded: boolean) => void;
  onOpenPreview: (expansion: ContentExpansion, filePath: string | undefined, imageDataUrl?: string) => void;
  cwd?: string;
}

function TraceRowView({ row, path, expanded, onToggle, onOpenPreview, cwd }: TraceRowViewProps) {
  const isOpen = expanded.get(path) ?? row.startsExpanded;
  const hasControl = traceRowOffersExpansion(row);
  const controlLabel = row.expansion
    ? expansionLabel(row.expansion, isOpen)
    : isOpen
      ? "Hide details"
      : "Show details";
  const running = row.state === "running";
  const rowClass = `trace-row trace-row-${row.style} trace-state-${row.state}${
    row.isStepStart ? " trace-row-step-start" : ""
  }${row.marker !== undefined ? " trace-row-turn-boundary" : ""}`;

  const main = (
    <>
      {row.marker !== undefined && <TraceTurnMarker kind={row.marker} />}
      {row.verb !== undefined && <span className="trace-verb">{row.verb}</span>}
      <TraceTarget row={row} />
      {row.preview !== undefined && <span className="trace-preview">{row.preview}</span>}
      {row.result !== undefined && <span className="trace-result">{row.result}</span>}
      {hasControl && (
        <span className="trace-disclosure" aria-hidden="true">
          <DisclosureIcon open={isOpen} size={14} />
        </span>
      )}
    </>
  );

  return (
    <article className={rowClass} data-state={row.state} data-running={running ? "true" : undefined}>
      {hasControl ? (
        <button
          className="trace-row-main trace-row-main-toggle"
          type="button"
          aria-expanded={isOpen}
          aria-label={controlLabel}
          onClick={() => onToggle(path, row.startsExpanded)}
        >
          {main}
        </button>
      ) : (
        <div className="trace-row-main">{main}</div>
      )}
      {isOpen && row.expansion && (
        row.event ? (
          <EventExpansionView event={row.event} expansion={row.expansion} cwd={cwd} onOpenPreview={onOpenPreview} />
        ) : (
          <div className="trace-expansion trace-expansion-reasoning">
            <ExpansionBody expansion={row.expansion} path={undefined} cwd={cwd} failed={false} onOpenPreview={onOpenPreview} />
          </div>
        )
      )}
      {isOpen && row.children.length > 0 && (
        <div className="trace-children">
          {row.children.map((child) => (
            <TraceRowView
              row={child}
              path={`${path}/${child.id}`}
              expanded={expanded}
              onToggle={onToggle}
              onOpenPreview={onOpenPreview}
              cwd={cwd}
              key={child.id}
            />
          ))}
        </div>
      )}
    </article>
  );
}

function FilePreviewModalBody({ preview }: { preview: OpenFilePreview }) {
  const [diskImage, setDiskImage] = React.useState<string | null>(null);
  const [imageError, setImageError] = React.useState<string | null>(null);
  const content = preview.expansion.preview;
  const image = content?.kind === "image";
  const source = preview.imageDataUrl ?? (content?.kind === "image" ? content.dataUrl : undefined) ?? diskImage;

  React.useEffect(() => {
    if (!image || source || !preview.path) return;
    let active = true;
    void readImagePreview(resolvePreviewPath(preview.path, preview.cwd)).then((result) => {
      if (!active) return;
      if (!result.ok) {
        setImageError(result.error.message);
        return;
      }
      const bytes = new Uint8Array(result.value.bytes);
      let binary = "";
      for (let offset = 0; offset < bytes.length; offset += 0x8000) {
        binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
      }
      setDiskImage(`data:${result.value.mime};base64,${btoa(binary)}`);
    });
    return () => {
      active = false;
    };
  }, [image, preview.cwd, preview.imageDataUrl, preview.path, source]);

  return (
    <div className="trace-file-preview-modal">
      <h2 id="file-preview-modal-title" className="trace-file-preview-modal-title">
        {preview.path ?? "File preview"}
      </h2>
      {image ? (
        imageError ? (
          <p className="trace-file-preview-error" role="status">{imageError}</p>
        ) : source ? (
          <img
            className="trace-file-preview-modal-image"
            src={source}
            alt={preview.path ?? "Image preview"}
          />
        ) : (
          <p className="trace-file-preview-status" role="status">Loading image…</p>
        )
      ) : (
        <MarkdownContent source={preview.expansion.text} />
      )}
    </div>
  );
}

const SHOW_THINKING_KEY = "traceShowThinking";

/**
 * Whether to show what the worker was thinking. Off by default: most readers
 * want the work, and the thinking is longer than all of it put together.
 */
export function useShowThinking(): [boolean, () => void] {
  const [showing, setShowing] = React.useState(() => readStorage(SHOW_THINKING_KEY) === "true");
  const toggle = React.useCallback(() => {
    setShowing((value) => {
      writeStorage(SHOW_THINKING_KEY, String(!value));
      return !value;
    });
  }, []);
  return [showing, toggle];
}

/** How many rows are mounted at once, and how many more each reveal adds. */
const WINDOW_SIZE = 60;

interface RowWindow {
  /** Index of the first mounted row. */
  from: number;
  /** Attach to a node above the window; reaching it mounts another window's worth. */
  sentinelRef: (node: HTMLDivElement | null) => void;
}

/**
 * Mounts the newest rows and reaches back only as the reader scrolls into the
 * top of what is mounted. A long run is thousands of rows and the reader
 * arrives at the end of it, so the ones above cost nothing until asked for.
 */
function useRowWindow(total: number): RowWindow {
  const [shown, setShown] = React.useState(WINDOW_SIZE);
  const observer = React.useRef<IntersectionObserver | null>(null);

  React.useEffect(() => () => observer.current?.disconnect(), []);

  const sentinelRef = React.useCallback((node: HTMLDivElement | null) => {
    observer.current?.disconnect();
    observer.current = null;
    if (!node) return;
    observer.current = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) setShown((count) => count + WINDOW_SIZE);
    });
    observer.current.observe(node);
  }, []);

  return { from: Math.max(total - shown, 0), sentinelRef };
}

/** The flat activity trace: every row the run produced, with expand/collapse. */
export function TraceRows({ rows, cwd }: { rows: TraceRow[]; cwd?: string }) {
  const [expanded, setExpanded] = React.useState<Map<string, boolean>>(new Map());
  const [openPreview, setOpenPreview] = React.useState<OpenFilePreview | null>(null);
  const toggle = React.useCallback((path: string, startsExpanded: boolean) => {
    setExpanded((current) => {
      const next = new Map(current);
      const value = current.get(path) ?? startsExpanded;
      next.set(path, !value);
      return next;
    });
  }, []);
  const { from, sentinelRef } = useRowWindow(rows.length);

  return (
    <section className="trace-panel" aria-label="What the worker did">
      {rows.length === 0 ? (
        <EmptyState title="No activity yet" className="detail-message" />
      ) : (
        <div className="trace-list-static">
          {from > 0 && <div className="trace-list-reach" ref={sentinelRef} aria-hidden="true" />}
          {rows.slice(from).map((row) => (
            <TraceRowView
              row={row}
              path={String(row.id)}
              expanded={expanded}
               onToggle={toggle}
                onOpenPreview={(expansion, filePath, imageDataUrl) => setOpenPreview({ cwd, path: filePath, expansion, imageDataUrl })}
                cwd={cwd}
              key={row.id}
            />
          ))}
        </div>
      )}
      <Modal
        open={openPreview !== null}
        onClose={() => setOpenPreview(null)}
        labelledBy="file-preview-modal-title"
        className="modal-dialog-file-preview"
      >
        {openPreview && <FilePreviewModalBody preview={openPreview} />}
      </Modal>
    </section>
  );
}
