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
      {failed && (
        <div className="trace-terminal-outcome" role="status">
          <CloseIcon size={14} />
          Failed
        </div>
      )}
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
  insideGroup?: boolean;
  rowRef?: (node: HTMLElement | null) => void;
  traceIndex?: number;
  traceSetSize?: number;
}

const TraceRowView = React.memo(function TraceRowView({
  row,
  path,
  expanded,
  onToggle,
  onOpenPreview,
  cwd,
  insideGroup = false,
  rowRef,
  traceIndex,
  traceSetSize,
}: TraceRowViewProps) {
  const isOpen = expanded.get(path) ?? row.startsExpanded;
  const hasControl = traceRowOffersExpansion(row);
  const controlLabel = row.expansion
    ? expansionLabel(row.expansion, isOpen)
    : isOpen
      ? "Hide details"
      : "Show details";
  const running = row.state === "running";
  const ownsRunningAnimation = running && (!insideGroup || row.children.length > 0);
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
    <article
      ref={rowRef}
      className={rowClass}
      data-state={row.state}
      data-running={ownsRunningAnimation ? "true" : undefined}
      data-trace-index={traceIndex}
      role="listitem"
      tabIndex={-1}
      aria-posinset={traceIndex === undefined ? undefined : traceIndex + 1}
      aria-setsize={traceSetSize}
    >
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
        <div className="trace-children" role="list">
          {row.children.map((child) => (
            <TraceRowView
              row={child}
              path={`${path}/${child.id}`}
              expanded={expanded}
              onToggle={onToggle}
              onOpenPreview={onOpenPreview}
              cwd={cwd}
              insideGroup
              key={`${path}/${child.id}`}
            />
          ))}
        </div>
      )}
    </article>
  );
});

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

const ESTIMATED_ROW_HEIGHT = 28;
const DEFAULT_VIEWPORT_HEIGHT = 640;
const OVERSCAN_PX = 560;

interface TraceLayout {
  keys: string[];
  offsets: number[];
  total: number;
}

interface TraceViewport {
  top: number;
  height: number;
}

interface TraceRange {
  start: number;
  end: number;
}

interface TraceVirtualization {
  range: TraceRange;
  layout: TraceLayout;
  getRowRef: (key: string) => (node: HTMLElement | null) => void;
  onKeyDown: (event: React.KeyboardEvent<HTMLElement>) => void;
  onFocusCapture: (event: React.FocusEvent<HTMLElement>) => void;
}

function traceKeys(rows: TraceRow[]): string[] {
  const occurrences = new Map<number, number>();
  return rows.map((row) => {
    const occurrence = occurrences.get(row.id) ?? 0;
    occurrences.set(row.id, occurrence + 1);
    return `${row.id}:${occurrence}`;
  });
}

function findScrollParent(node: HTMLElement | null): HTMLElement | null {
  let current = node?.parentElement;
  while (current) {
    const overflow = getComputedStyle(current).overflowY;
    if (overflow === "auto" || overflow === "scroll" || overflow === "overlay") return current;
    current = current.parentElement;
  }
  return null;
}

function findOffsetIndex(offsets: number[], target: number): number {
  let low = 0;
  let high = offsets.length - 1;
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    if (offsets[middle] <= target) low = middle;
    else high = middle - 1;
  }
  return Math.max(0, Math.min(low, offsets.length - 2));
}

function focusTraceRow(element: HTMLElement): void {
  (element.querySelector("button") as HTMLElement | null ?? element).focus();
}

function useTraceVirtualization(
  rows: TraceRow[],
  panelRef: React.RefObject<HTMLElement | null>,
  scrollRoot?: React.RefObject<HTMLElement | null>,
): TraceVirtualization {
  const [heights, setHeights] = React.useState<Map<string, number>>(new Map());
  const [viewport, setViewport] = React.useState<TraceViewport>({ top: Number.POSITIVE_INFINITY, height: DEFAULT_VIEWPORT_HEIGHT });
  const heightsRef = React.useRef(heights);
  const viewportRef = React.useRef(viewport);
  const layoutRef = React.useRef<TraceLayout>({ keys: [], offsets: [0], total: 0 });
  const rootRef = React.useRef<HTMLElement | null>(null);
  const rowElements = React.useRef(new Map<string, HTMLElement>());
  const rowRefCallbacks = React.useRef(new Map<string, (node: HTMLElement | null) => void>());
  const resizeObserver = React.useRef<ResizeObserver | null>(null);
  const pendingFocus = React.useRef<number | undefined>(undefined);
  const activeIndex = React.useRef<number | undefined>(undefined);
  const frame = React.useRef<number | undefined>(undefined);

  heightsRef.current = heights;
  viewportRef.current = viewport;

  const layout = React.useMemo<TraceLayout>(() => {
    const keys = traceKeys(rows);
    const offsets = [0];
    let total = 0;
    rows.forEach((row, index) => {
      const key = keys[index];
      total += heights.get(key) ?? ESTIMATED_ROW_HEIGHT;
      offsets.push(total);
    });
    return { keys, offsets, total };
  }, [heights, rows]);
  layoutRef.current = layout;

  const range = React.useMemo<TraceRange>(() => {
    if (rows.length === 0) return { start: 0, end: 0 };
    const top = Number.isFinite(viewport.top) ? viewport.top : Math.max(layout.total - viewport.height, 0);
    const start = findOffsetIndex(layout.offsets, Math.max(0, top - OVERSCAN_PX));
    const end = Math.min(
      rows.length,
      findOffsetIndex(layout.offsets, Math.min(layout.total, top + viewport.height + OVERSCAN_PX)) + 1,
    );
    return { start, end: Math.max(start + 1, end) };
  }, [layout, rows.length, viewport]);

  const panelOffset = React.useCallback((): number => {
    const root = rootRef.current;
    const panel = panelRef.current;
    if (!root || !panel) return 0;
    const rootRect = root.getBoundingClientRect();
    const panelRect = panel.getBoundingClientRect();
    return panelRect.top - rootRect.top + root.scrollTop;
  }, [panelRef]);

  const readViewport = React.useCallback((): TraceViewport => {
    const root = rootRef.current;
    if (!root) return { top: Number.POSITIVE_INFINITY, height: DEFAULT_VIEWPORT_HEIGHT };
    const height = root.clientHeight || DEFAULT_VIEWPORT_HEIGHT;
    return {
      top: Math.max(0, root.scrollTop - panelOffset()),
      height,
    };
  }, [panelOffset]);

  const updateViewport = React.useCallback(() => {
    const next = readViewport();
    viewportRef.current = next;
    setViewport(next);
  }, [readViewport]);

  const scheduleViewport = React.useCallback(() => {
    if (frame.current !== undefined) return;
    const run = () => {
      frame.current = undefined;
      updateViewport();
    };
    if (typeof requestAnimationFrame === "function") frame.current = requestAnimationFrame(run);
    else frame.current = setTimeout(run, 16) as unknown as number;
  }, [updateViewport]);

  React.useLayoutEffect(() => {
    const root = scrollRoot?.current ?? findScrollParent(panelRef.current);
    rootRef.current = root;
    updateViewport();
    if (!root) return;
    root.addEventListener("scroll", scheduleViewport, { passive: true });
    window.addEventListener("resize", scheduleViewport);
    return () => {
      root.removeEventListener("scroll", scheduleViewport);
      window.removeEventListener("resize", scheduleViewport);
      if (frame.current !== undefined) {
        if (typeof cancelAnimationFrame === "function") cancelAnimationFrame(frame.current);
        else clearTimeout(frame.current);
        frame.current = undefined;
      }
    };
  }, [panelRef, scheduleViewport, scrollRoot, updateViewport]);

  const applyHeight = React.useCallback((key: string, height: number) => {
    if (height <= 0) return;
    const oldHeight = heightsRef.current.get(key) ?? ESTIMATED_ROW_HEIGHT;
    if (Math.abs(oldHeight - height) < 1) return;
    const index = layoutRef.current.keys.indexOf(key);
    const currentTop = Number.isFinite(viewportRef.current.top)
      ? viewportRef.current.top
      : Math.max(layoutRef.current.total - viewportRef.current.height, 0);
    if (index >= 0 && layoutRef.current.offsets[index] < currentTop && rootRef.current) {
      rootRef.current.scrollTop += height - oldHeight;
    }
    const next = new Map(heightsRef.current);
    next.set(key, height);
    heightsRef.current = next;
    setHeights(next);
  }, []);

  const measureElement = React.useCallback((key: string, element: HTMLElement) => {
    const height = element.getBoundingClientRect().height || element.offsetHeight;
    applyHeight(key, height);
  }, [applyHeight]);

  const registerRow = React.useCallback((key: string, element: HTMLElement | null) => {
    const previous = rowElements.current.get(key);
    if (element === null) {
      if (previous && resizeObserver.current) resizeObserver.current.unobserve(previous);
      rowElements.current.delete(key);
      return;
    }
    rowElements.current.set(key, element);
    element.dataset.traceKey = key;
    resizeObserver.current?.observe(element);
    measureElement(key, element);
  }, [measureElement]);

  React.useLayoutEffect(() => {
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver((entries) => {
      entries.forEach((entry) => {
        const key = (entry.target as HTMLElement).dataset.traceKey;
        if (key) measureElement(key, entry.target as HTMLElement);
      });
    });
    resizeObserver.current = observer;
    rowElements.current.forEach((element) => observer.observe(element));
    return () => {
      observer.disconnect();
      resizeObserver.current = null;
    };
  }, [measureElement]);

  const getRowRef = React.useCallback((key: string) => {
    const existing = rowRefCallbacks.current.get(key);
    if (existing) return existing;
    const callback = (element: HTMLElement | null) => {
      registerRow(key, element);
      if (element === null) rowRefCallbacks.current.delete(key);
    };
    rowRefCallbacks.current.set(key, callback);
    return callback;
  }, [registerRow]);

  const scrollToIndex = React.useCallback((index: number) => {
    const root = rootRef.current;
    const top = layoutRef.current.offsets[index] ?? 0;
    const bottom = layoutRef.current.offsets[index + 1] ?? top + ESTIMATED_ROW_HEIGHT;
    const current = viewportRef.current;
    const localTop = Number.isFinite(current.top) ? current.top : Math.max(layoutRef.current.total - current.height, 0);
    if (top >= localTop && bottom <= localTop + current.height) return;
    if (root) {
      const next = top < localTop
        ? panelOffset() + top
        : panelOffset() + bottom - current.height;
      root.scrollTop = Math.max(0, next);
      updateViewport();
    } else {
      const nextTop = top < localTop ? top : bottom - current.height;
      const next = { ...current, top: Math.max(0, nextTop) };
      viewportRef.current = next;
      setViewport(next);
    }
  }, [panelOffset, updateViewport]);

  const focusIndex = React.useCallback((index: number) => {
    if (rows.length === 0) return;
    const nextIndex = Math.max(0, Math.min(index, rows.length - 1));
    activeIndex.current = nextIndex;
    pendingFocus.current = nextIndex;
    scrollToIndex(nextIndex);
    const key = layoutRef.current.keys[nextIndex];
    const element = key === undefined ? undefined : rowElements.current.get(key);
    if (element) {
      pendingFocus.current = undefined;
      focusTraceRow(element);
    }
  }, [rows.length, scrollToIndex]);

  React.useLayoutEffect(() => {
    const index = pendingFocus.current;
    if (index === undefined) return;
    const key = layout.keys[index];
    const element = key === undefined ? undefined : rowElements.current.get(key);
    if (!element) return;
    pendingFocus.current = undefined;
    focusTraceRow(element);
  }, [layout, range]);

  const onFocusCapture = React.useCallback((event: React.FocusEvent<HTMLElement>) => {
    const target = event.target instanceof HTMLElement ? event.target.closest<HTMLElement>("[data-trace-index]") : null;
    const index = target?.dataset.traceIndex;
    if (index !== undefined) activeIndex.current = Number(index);
  }, []);

  const onKeyDown = React.useCallback((event: React.KeyboardEvent<HTMLElement>) => {
    const target = event.target instanceof HTMLElement ? event.target.closest<HTMLElement>("[data-trace-index]") : null;
    const current = target?.dataset.traceIndex === undefined
      ? (activeIndex.current ?? range.start)
      : Number(target.dataset.traceIndex);
    let next: number | undefined;
    if (event.key === "ArrowDown") next = current + 1;
    else if (event.key === "ArrowUp") next = current - 1;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = rows.length - 1;
    else if (event.key === "PageDown") next = current + Math.max(1, Math.floor(viewport.height / ESTIMATED_ROW_HEIGHT));
    else if (event.key === "PageUp") next = current - Math.max(1, Math.floor(viewport.height / ESTIMATED_ROW_HEIGHT));
    if (next === undefined) return;
    event.preventDefault();
    focusIndex(next);
  }, [focusIndex, range.start, rows.length, viewport.height]);

  return { range, layout, getRowRef, onKeyDown, onFocusCapture };
}

/** The flat activity trace: every row the run produced, with expand/collapse. */
export function TraceRows({
  rows,
  cwd,
  scrollRoot,
}: {
  rows: TraceRow[];
  cwd?: string;
  scrollRoot?: React.RefObject<HTMLElement | null>;
}) {
  const [expanded, setExpanded] = React.useState<Map<string, boolean>>(new Map());
  const [openPreview, setOpenPreview] = React.useState<OpenFilePreview | null>(null);
  const panelRef = React.useRef<HTMLElement>(null);
  const toggle = React.useCallback((path: string, startsExpanded: boolean) => {
    setExpanded((current) => {
      const next = new Map(current);
      const value = current.get(path) ?? startsExpanded;
      next.set(path, !value);
      return next;
    });
  }, []);
  const { range, layout, getRowRef, onKeyDown, onFocusCapture } = useTraceVirtualization(rows, panelRef, scrollRoot);
  const openPreviewFile = React.useCallback(
    (expansion: ContentExpansion, filePath: string | undefined, imageDataUrl?: string) => {
      setOpenPreview({ cwd, path: filePath, expansion, imageDataUrl });
    },
    [cwd],
  );

  return (
    <section
      ref={panelRef}
      className="trace-panel"
      aria-label="What the worker did"
      tabIndex={0}
      onKeyDown={onKeyDown}
      onFocusCapture={onFocusCapture}
    >
      {rows.length === 0 ? (
        <EmptyState title="No activity yet" className="detail-message" />
      ) : (
        <div className="trace-list-static" role="list">
          {range.start > 0 && (
            <div className="trace-list-spacer" style={{ height: layout.offsets[range.start] }} aria-hidden="true" />
          )}
          {rows.slice(range.start, range.end).map((row, offset) => {
            const index = range.start + offset;
            const key = layout.keys[index];
            return (
              <TraceRowView
                row={row}
                path={String(row.id)}
                expanded={expanded}
                onToggle={toggle}
                onOpenPreview={openPreviewFile}
                cwd={cwd}
                rowRef={getRowRef(key)}
                traceIndex={index}
                traceSetSize={rows.length}
                key={key}
              />
            );
          })}
          {range.end < rows.length && (
            <div className="trace-list-spacer" style={{ height: layout.total - layout.offsets[range.end] }} aria-hidden="true" />
          )}
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
