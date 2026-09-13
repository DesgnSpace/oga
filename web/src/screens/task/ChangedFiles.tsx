// The changed-files view: files a run touched, with bounded per-file diffs.
// Two chromes over one body — a side panel beside the transcript, and a
// full-screen review with the file tree kept open beside the diffs.
// Ported from rust/crates/oga-ui/src/changes/mod.rs's view layer — the pure
// derivation (`collectRunChanges`) already lives in @/domain/changes.

import * as React from "react";
import type { TaskDiffFileStatus } from "@/bridge/types";
import type { ChangedFileSet, ChangedFileView } from "@/domain/changes";
import { runChangeSetAdded, runChangeSetRemoved } from "@/domain/changes";
import type { ChangeTurn, ChangeTurnSet } from "@/domain/changes/grouped";
import { buildFileTree, type TreeNode } from "@/domain/changes/tree";
import { absoluteTime, relativeTime } from "@/ui/time";
import { CloseIcon, CollapseIcon, DisclosureIcon, ExpandIcon, RefreshIcon } from "@/ui/icons";
import { EmptyState, LoadingState } from "@/components/atoms/ListState";
import { CodeDiff } from "@/components/CodeDiff";
import { Modal } from "@/components/primitives/Modal";
import { CHANGED_FILES_MAX_WIDTH, CHANGED_FILES_MIN_WIDTH } from "@/state/changed-files-preferences";
import { patchFromBlocks } from "@/lib/unified-patch";
import { DiffHeader } from "@/components/DiffHeader";

/** Arrow-key resize increment, in pixels. */
const RESIZE_KEYBOARD_STEP = 16;

/** Names the full-screen dialog for assistive technology. */
const FULL_SCREEN_TITLE_ID = "changed-files-full-screen-title";

/** Turn id for grouped turns, or a stable key for the pre-turn bucket. */
function turnKey(turn: ChangeTurn): string {
  return turn.turnId !== undefined ? `t${turn.turnId}` : `e${turn.ordinal}`;
}

/** Where the panel's diffs come from. */
const CHANGES_SOURCES = ["reported", "git"] as const;

export type ChangesSource = (typeof CHANGES_SOURCES)[number];

const CHANGES_SOURCE_LABELS = {
  reported: "Reported",
  git: "Git",
} satisfies Record<ChangesSource, string>;

const CHANGES_SOURCE_HINTS = {
  reported: "What the worker said it changed",
  git: "What git shows in this task's checkout",
} satisfies Record<ChangesSource, string>;

const STATUS_LABELS = {
  added: "New",
  modified: "Changed",
  deleted: "Deleted",
  renamed: "Renamed",
  untracked: "Not in git",
} satisfies Record<TaskDiffFileStatus, string>;

/** Single-letter glyphs for the file tree, in the shorthand git status readers already know. */
const STATUS_GLYPHS = {
  added: "A",
  modified: "M",
  deleted: "D",
  renamed: "R",
  untracked: "U",
} satisfies Record<TaskDiffFileStatus, string>;

function fileCount(files: number): string {
  return `${files} file${files === 1 ? "" : "s"}`;
}

function ChangedFileRow({
  file,
  expanded,
  onToggle,
  active,
  registerRow,
  showDiffHeader,
}: {
  file: ChangedFileView;
  expanded: boolean;
  onToggle: () => void;
  active: boolean;
  registerRow: (path: string, element: HTMLElement | null) => void;
  showDiffHeader: boolean;
}) {
  const patch = React.useMemo(
    () => file.patch ?? patchFromBlocks(file.path, file.change.blocks),
    [file.patch, file.path, file.change.blocks],
  );
  const status = file.status === undefined || file.status === "modified" ? undefined : STATUS_LABELS[file.status];
  return (
    <section
      className={`changed-file-row${active ? " changed-file-row-active" : ""}`}
      ref={(element) => registerRow(file.path, element)}
    >
      <button
        className="changed-file-heading"
        type="button"
        aria-expanded={expanded}
        onClick={onToggle}
      >
        <span className="changed-file-disclosure" aria-hidden="true">
          <DisclosureIcon open={expanded} />
        </span>
        <span className="changed-file-path" title={file.path}>{file.path}</span>
        <span className="changed-file-count">
          {status !== undefined && <span className="changed-file-status">{status}</span>}
          <span className="diff-stat-added">{`+${file.added}`}</span>{" "}
          <span className="diff-stat-removed">{`-${file.removed}`}</span>
        </span>
      </button>
      {expanded && (
        <div className="changed-file-diff">
          {showDiffHeader && <DiffHeader />}
          {patch !== undefined && <CodeDiff patch={patch} numbered={file.patch !== undefined} wrap />}
          {file.tooLarge ? (
            <p className="changed-file-note">This file's diff is too big to show here. Open it in your editor.</p>
          ) : (
            patch === undefined && <p className="changed-file-note">No text changes to show.</p>
          )}
          {file.hiddenLines > 0 && (
            <p className="changed-file-note">{`${file.hiddenLines} more line${file.hiddenLines === 1 ? "" : "s"} not shown`}</p>
          )}
          {file.shortened && (
            <p className="changed-file-note">Some lines are missing because the stored change was shortened.</p>
          )}
        </div>
      )}
    </section>
  );
}

interface FileRowProps {
  expandedPaths: Set<string>;
  onToggleFile: (path: string) => void;
  activePath?: string;
  registerRow: (path: string, element: HTMLElement | null) => void;
  showDiffHeader: boolean;
}

function ChangedFileList({ files, expandedPaths, onToggleFile, activePath, registerRow, showDiffHeader }: FileRowProps & { files: ChangedFileView[] }) {
  return (
    <div className="changed-files-list">
      {files.map((file) => (
        <ChangedFileRow
          file={file}
          key={file.path}
          expanded={expandedPaths.has(file.path)}
          onToggle={() => onToggleFile(file.path)}
          active={activePath === file.path}
          registerRow={registerRow}
          showDiffHeader={showDiffHeader}
        />
      ))}
    </div>
  );
}

function ChangeTurnGroup({
  turn,
  expanded,
  onToggleTurn,
  ...fileRowProps
}: FileRowProps & { turn: ChangeTurn; expanded: boolean; onToggleTurn: () => void }) {
  return (
    <section className="changed-files-turn">
      <button className="changed-files-turn-heading" type="button" aria-expanded={expanded} onClick={onToggleTurn}>
        <span className="changed-file-disclosure" aria-hidden="true">
          <DisclosureIcon open={expanded} />
        </span>
        <span className="changed-files-turn-label">{turn.label}</span>
        <span className="changed-files-turn-meta">
          {turn.at !== undefined ? <><span title={absoluteTime(turn.at)}>{relativeTime(turn.at)}</span> · </> : null}
          {fileCount(turn.files.length)}
        </span>
      </button>
      {expanded && <ChangedFileList files={turn.files} {...fileRowProps} />}
    </section>
  );
}

function FileTreeNodes({
  nodes,
  activePath,
  collapsedDirs,
  onToggleDir,
  onSelectFile,
}: {
  nodes: TreeNode[];
  activePath?: string;
  collapsedDirs: Set<string>;
  onToggleDir: (path: string) => void;
  onSelectFile: (path: string) => void;
}) {
  return (
    <ul className="changed-files-tree-list">
      {nodes.map((node) =>
        node.kind === "dir" ? (
          <li key={node.path}>
            <button
              type="button"
              className="changed-files-tree-dir"
              aria-expanded={!collapsedDirs.has(node.path)}
              onClick={() => onToggleDir(node.path)}
            >
              <span className="changed-file-disclosure" aria-hidden="true">
                <DisclosureIcon open={!collapsedDirs.has(node.path)} />
              </span>
              <span className="changed-files-tree-name" title={node.path}>{node.name}</span>
            </button>
            {!collapsedDirs.has(node.path) && (
              <FileTreeNodes
                nodes={node.children}
                activePath={activePath}
                collapsedDirs={collapsedDirs}
                onToggleDir={onToggleDir}
                onSelectFile={onSelectFile}
              />
            )}
          </li>
        ) : (
          <li key={node.path}>
            <button
              type="button"
              className={`changed-files-tree-file${activePath === node.path ? " changed-files-tree-file-active" : ""}`}
              onClick={() => onSelectFile(node.path)}
            >
              {node.file.status !== undefined && (
                <span
                  className={`changed-files-tree-glyph changed-files-tree-glyph-${node.file.status}`}
                  title={STATUS_LABELS[node.file.status]}
                >
                  {STATUS_GLYPHS[node.file.status]}
                </span>
              )}
              <span className="changed-files-tree-name" title={node.path}>{node.name}</span>
              <span className="changed-files-tree-count">
                <span className="diff-stat-added">{`+${node.file.added}`}</span>{" "}
                <span className="diff-stat-removed">{`-${node.file.removed}`}</span>
              </span>
            </button>
          </li>
        ),
      )}
    </ul>
  );
}

function SourcePicker({
  source,
  onSourceChange,
}: {
  source: ChangesSource;
  onSourceChange: (source: ChangesSource) => void;
}) {
  return (
    <div className="changed-files-sources" role="group" aria-label="Show changes from">
      {CHANGES_SOURCES.map((option) => (
        <button
          key={option}
          className={`changed-files-source${source === option ? " changed-files-source-active" : ""}`}
          type="button"
          aria-pressed={source === option}
          title={CHANGES_SOURCE_HINTS[option]}
          onClick={() => onSourceChange(option)}
        >
          {CHANGES_SOURCE_LABELS[option]}
        </button>
      ))}
    </div>
  );
}

/** The files and their state, shared by both chromes. */
export interface ChangedFilesProps {
  source: ChangesSource;
  onSourceChange: (source: ChangesSource) => void;
  groupByTurn: boolean;
  onGroupByTurn: (grouped: boolean) => void;
  /** Reads the checkout again. Only offered for the git source. */
  onReload: () => void;
  /** The flat view of whichever source is showing. */
  changes: ChangedFileSet;
  /** The same files split by turn, present only for the reported source. */
  turns?: ChangeTurnSet;
  loading: boolean;
  /** Why reading the checkout failed, when it did. */
  error?: string;
  /** Set when the checkout held more than one read carries. */
  truncated?: boolean;
  live: boolean;
  hasEarlier: boolean;
  loadingEarlier: boolean;
  onLoadEarlier: () => void;
}

type ChangedFilesLayout = "docked" | "full";

interface ChangedFilesViewProps extends ChangedFilesProps {
  layout: ChangedFilesLayout;
  onClose: () => void;
  /** Opens the full-screen review. Offered by the docked panel only. */
  onExpand?: () => void;
}

function ChangedFilesView({
  layout,
  source,
  onSourceChange,
  groupByTurn,
  onGroupByTurn,
  onReload,
  changes,
  turns,
  loading,
  error,
  truncated,
  live,
  hasEarlier,
  loadingEarlier,
  onLoadEarlier,
  onClose,
  onExpand,
}: ChangedFilesViewProps) {
  const full = layout === "full";
  const added = runChangeSetAdded(changes);
  const removed = runChangeSetRemoved(changes);
  const grouped = turns !== undefined && groupByTurn;
  const empty = grouped ? turns.turns.length === 0 : changes.files.length === 0;
  const tree = React.useMemo(() => buildFileTree(changes.files), [changes.files]);

  const [filePopoverOpen, setFilePopoverOpen] = React.useState(false);
  const filePopoverRef = React.useRef<HTMLDivElement>(null);
  const [collapsedDirs, setCollapsedDirs] = React.useState<Set<string>>(new Set());
  const [expandedPaths, setExpandedPaths] = React.useState<Set<string>>(new Set());
  const [expandedTurns, setExpandedTurns] = React.useState<Set<string>>(new Set());
  const [activePath, setActivePath] = React.useState<string | undefined>(undefined);
  const [scrollRequest, setScrollRequest] = React.useState<{ path: string } | undefined>(undefined);
  const seenTurnsRef = React.useRef<Set<string>>(new Set());
  const seenPathsRef = React.useRef<Set<string>>(new Set());
  const rowElementsRef = React.useRef<Map<string, HTMLElement>>(new Map());
  const scrollFrameRef = React.useRef<number | undefined>(undefined);

  // Docked, the newest turn opens on its own, same as the reader would expect
  // from an accordion; turns they have already seen keep whatever they set.
  // Full screen there is room for everything, so every turn opens.
  React.useEffect(() => {
    const candidates = full ? (turns?.turns ?? []) : (turns?.turns.slice(0, 1) ?? []);
    const fresh = candidates.map(turnKey).filter((key) => !seenTurnsRef.current.has(key));
    if (fresh.length === 0) return;
    for (const key of fresh) seenTurnsRef.current.add(key);
    setExpandedTurns((prev) => {
      const next = new Set(prev);
      for (const key of fresh) next.add(key);
      return next;
    });
  }, [turns, full]);

  // Full screen reads as one continuous diff, so a file opens the first time
  // it appears and stays however the reader leaves it.
  React.useEffect(() => {
    if (!full) return;
    const fresh = changes.files.map((file) => file.path).filter((path) => !seenPathsRef.current.has(path));
    if (fresh.length === 0) return;
    for (const path of fresh) seenPathsRef.current.add(path);
    setExpandedPaths((prev) => {
      const next = new Set(prev);
      for (const path of fresh) next.add(path);
      return next;
    });
  }, [full, changes.files]);

  React.useEffect(() => {
    if (!filePopoverOpen) return;
    const closeOnPointer = (event: PointerEvent) => {
      // SAFETY: pointer events always target a Node in the DOM tree.
      const target = event.target as Node | null;
      if (target && filePopoverRef.current?.contains(target)) return;
      setFilePopoverOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setFilePopoverOpen(false);
    };
    document.addEventListener("pointerdown", closeOnPointer);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointer);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [filePopoverOpen]);

  React.useEffect(() => () => {
    if (scrollFrameRef.current !== undefined) cancelAnimationFrame(scrollFrameRef.current);
  }, []);

  // A row's disclosure only mounts once its turn is open, so the row this
  // click targets may not exist yet: keep polling a few frames until it does.
  React.useEffect(() => {
    if (!scrollRequest) return;
    let frame: number;
    let attempts = 0;
    const tryScroll = () => {
      const element = rowElementsRef.current.get(scrollRequest.path);
      if (element) {
        element.scrollIntoView({ block: "center", behavior: "smooth" });
        setActivePath(scrollRequest.path);
        return;
      }
      attempts += 1;
      if (attempts < 12) frame = requestAnimationFrame(tryScroll);
    };
    frame = requestAnimationFrame(tryScroll);
    return () => cancelAnimationFrame(frame);
  }, [scrollRequest]);

  // Rows never unregister: a row that unmounts (its turn collapsed) leaves a
  // stale entry until another row for the same path replaces it, which is
  // harmless — scrollIntoView on a detached element is a silent no-op.
  const registerRow = React.useCallback((path: string, element: HTMLElement | null) => {
    if (element) rowElementsRef.current.set(path, element);
  }, []);

  const toggleDir = (path: string) => {
    setCollapsedDirs((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const toggleFile = (path: string) => {
    setExpandedPaths((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const toggleTurn = (key: string) => {
    setExpandedTurns((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const selectFile = (path: string) => {
    if (turns) {
      setExpandedTurns((prev) => {
        const next = new Set(prev);
        for (const turn of turns.turns) {
          if (turn.files.some((file) => file.path === path)) next.add(turnKey(turn));
        }
        return next;
      });
    }
    setExpandedPaths((prev) => new Set(prev).add(path));
    setScrollRequest({ path });
  };

  const handleDiffsScroll = (event: React.UIEvent<HTMLDivElement>) => {
    if (scrollFrameRef.current !== undefined) return;
    const container = event.currentTarget;
    scrollFrameRef.current = requestAnimationFrame(() => {
      scrollFrameRef.current = undefined;
      const top = container.getBoundingClientRect().top;
      let above: string | undefined;
      let aboveOffset = -Infinity;
      let below: string | undefined;
      let belowOffset = Infinity;
      for (const [path, element] of rowElementsRef.current) {
        if (!container.contains(element)) continue;
        const offset = element.getBoundingClientRect().top - top;
        if (offset <= 8 && offset > aboveOffset) {
          aboveOffset = offset;
          above = path;
        } else if (offset > 8 && offset < belowOffset) {
          belowOffset = offset;
          below = path;
        }
      }
      const next = above ?? below;
      if (next !== undefined) setActivePath(next);
    });
  };

  const fileRowProps: FileRowProps = {
    expandedPaths,
    onToggleFile: toggleFile,
    activePath,
    registerRow,
    showDiffHeader: !full,
  };

  const fileTree = (
    <FileTreeNodes
      nodes={tree}
      activePath={activePath}
      collapsedDirs={collapsedDirs}
      onToggleDir={toggleDir}
      onSelectFile={(path) => {
        selectFile(path);
        setFilePopoverOpen(false);
      }}
    />
  );

  const diffs = (
    <div className="changed-files-diffs" onScroll={handleDiffsScroll}>
      {grouped ? (
        <div className="changed-files-turns">
          {turns.turns.map((turn) => (
            <ChangeTurnGroup
              key={turnKey(turn)}
              turn={turn}
              expanded={expandedTurns.has(turnKey(turn))}
              onToggleTurn={() => toggleTurn(turnKey(turn))}
              {...fileRowProps}
            />
          ))}
        </div>
      ) : (
        <ChangedFileList files={changes.files} {...fileRowProps} />
      )}
    </div>
  );

  return (
    <>
      <header className={`changed-files-header${full ? " changed-files-header-full" : ""}`}>
        <div className="changed-files-header-row">
          <div className="changed-files-header-title">
            <h2 id={full ? FULL_SCREEN_TITLE_ID : undefined}>
              {`Changed files (${fileCount(grouped ? turns.turns.reduce((count, turn) => count + turn.files.length, 0) : changes.files.length)})`}
            </h2>
            {activePath !== undefined ? (
              <p className="changed-files-active-file" title={activePath}>
                {activePath}
              </p>
            ) : (
              changes.files.length > 0 && (
                <p className="changed-files-summary">
                  <span className="diff-stat-added">{`+${added}`}</span>
                  {" · "}
                  <span className="diff-stat-removed">{`-${removed}`}</span>
                </p>
              )
            )}
          </div>
          <div className="changed-files-header-actions">
            {full && <DiffHeader />}
            <SourcePicker source={source} onSourceChange={onSourceChange} />
            {source === "reported" ? (
              <label className="changed-files-group">
                <input type="checkbox" checked={groupByTurn} onChange={(event) => onGroupByTurn(event.target.checked)} />
                Group by turn
              </label>
            ) : null}
            <button
              className="icon-button"
              type="button"
              disabled={loading || source === "reported"}
              aria-label="Refresh"
              title={source === "reported" ? "Refresh applies to changes read from git" : "Refresh"}
              onClick={onReload}
            >
              <RefreshIcon />
            </button>
            {!full && !empty && !loading && error === undefined && (
              <div className="changed-files-tree-popover" ref={filePopoverRef}>
                <button
                  className="text-button"
                  type="button"
                  aria-haspopup="true"
                  aria-expanded={filePopoverOpen}
                  onClick={() => setFilePopoverOpen((value) => !value)}
                >
                  Files
                </button>
                {filePopoverOpen && (
                  <div className="changed-files-tree-popover-panel">
                    <nav aria-label="File list">{fileTree}</nav>
                  </div>
                )}
              </div>
            )}
            {onExpand !== undefined && (
              <button className="icon-button" type="button" aria-label="Full screen" title="Full screen" onClick={onExpand}>
                <ExpandIcon />
              </button>
            )}
            <button
              className="icon-button"
              type="button"
              aria-label={full ? "Exit full screen" : "Hide changed files"}
              title={full ? "Exit full screen" : "Hide changed files"}
              onClick={onClose}
            >
              {full ? <CollapseIcon /> : <CloseIcon />}
            </button>
          </div>
        </div>
      </header>
      <div className={`changed-files-content${full ? " changed-files-content-full" : ""}`}>
        {loading ? (
          <LoadingState label="Loading changed files" className="changed-files-message" />
        ) : error !== undefined ? (
          <div className="changed-files-message" role="alert">
            <p>We couldn't read this task's checkout. Refresh to try again.</p>
            <small>{error}</small>
          </div>
        ) : empty ? (
          <EmptyState
            title="No files changed yet"
            hint={live && source === "reported" ? "Changes appear here as the run continues." : "Changes appear here when this run edits files."}
            className="changed-files-message"
          />
        ) : full ? (
          <div className="changed-files-split">
            <nav className="changed-files-rail" aria-label="File list">{fileTree}</nav>
            {diffs}
          </div>
        ) : (
          diffs
        )}
        {changes.unmatched > 0 && (
          <p className="changed-files-note">
            {`${changes.unmatched} change${changes.unmatched === 1 ? "" : "s"} did not name a file.`}
          </p>
        )}
        {truncated && (
          <p className="changed-files-note">
            This checkout has more changes than fit here. Open it in your editor to see them all.
          </p>
        )}
        {hasEarlier && source === "reported" && (
          <div className="changed-files-earlier">
            <p>Earlier activity is not loaded, so this may not be the whole run.</p>
            <button className="text-button" type="button" disabled={loadingEarlier} onClick={onLoadEarlier}>
              {loadingEarlier ? "Loading earlier activity…" : "Load earlier activity"}
            </button>
          </div>
        )}
      </div>
    </>
  );
}

export interface ChangedFilesPanelProps extends ChangedFilesProps {
  onClose: () => void;
  onExpand: () => void;
  width: number;
  onResizeStart: (clientX: number) => void;
  onResetWidth: () => void;
  onResizeStep: (deltaWidth: number) => void;
}

export function ChangedFilesPanel({
  onClose,
  onExpand,
  width,
  onResizeStart,
  onResetWidth,
  onResizeStep,
  ...view
}: ChangedFilesPanelProps) {
  return (
    <aside
      id="changed-files-panel"
      className="changed-files-panel"
      aria-label="Changed files"
      style={{ width: `${width}px` }}
    >
      <div
        className="changed-files-resize-handle"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize changed files panel"
        aria-valuenow={width}
        aria-valuemin={CHANGED_FILES_MIN_WIDTH}
        aria-valuemax={CHANGED_FILES_MAX_WIDTH}
        tabIndex={0}
        onPointerDown={(event) => {
          event.preventDefault();
          onResizeStart(event.clientX);
        }}
        onDoubleClick={onResetWidth}
        title="Drag to resize · double-click to reset"
        onKeyDown={(event) => {
          if (event.key === "ArrowLeft") {
            event.preventDefault();
            onResizeStep(RESIZE_KEYBOARD_STEP);
          } else if (event.key === "ArrowRight") {
            event.preventDefault();
            onResizeStep(-RESIZE_KEYBOARD_STEP);
          } else if (event.key === "Enter") {
            event.preventDefault();
            onResetWidth();
          }
        }}
      />
      <ChangedFilesView layout="docked" onClose={onClose} onExpand={onExpand} {...view} />
    </aside>
  );
}

/** The same files over the whole window, for reading a run's diff end to end. */
export function ChangedFilesFullScreen({ onClose, ...view }: ChangedFilesProps & { onClose: () => void }) {
  return (
    <Modal
      open
      onClose={onClose}
      labelledBy={FULL_SCREEN_TITLE_ID}
      overlayClassName="modal-overlay-bleed"
      className="modal-dialog-review"
      hideClose
    >
      <ChangedFilesView layout="full" onClose={onClose} {...view} />
    </Modal>
  );
}
