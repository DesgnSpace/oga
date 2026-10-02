// The changed-files view: files a run touched, with bounded diffs.

import * as React from "react";
import type { TaskDiffFileStatus } from "@/bridge/types";
import type { ChangedFileSet, ChangedFileView } from "@/domain/changes";
import { runChangeSetAdded, runChangeSetRemoved } from "@/domain/changes";
import type { ChangeTurn, ChangeTurnSet } from "@/domain/changes/grouped";
import {
  CHANGE_SORTS,
  GENERATED_GROUP_KEY,
  GENERATED_GROUP_NAME,
  orderChangeFiles,
  orderChangeTurns,
  type ChangeSort,
  type OrderedChangeTurn,
} from "@/domain/changes/ordering";
import { buildFileTree, type TreeNode } from "@/domain/changes/tree";
import { absoluteTime, relativeTime } from "@/ui/time";
import { CheckCircleIcon, CheckIcon, ChevronIcon, CircleIcon, CloseIcon, CollapseIcon, DisclosureIcon, ExpandIcon, RefreshIcon } from "@/ui/icons";
import { EmptyState, LoadingState } from "@/components/atoms/ListState";
import { CodeDiff } from "@/components/CodeDiff";
import { Modal } from "@/components/primitives/Modal";
import { MenuPanel } from "@/components/menu/Menu";
import {
  CHANGED_FILES_MAX_WIDTH,
  CHANGED_FILES_MIN_WIDTH,
  type ChangesSource,
} from "@/state/changed-files-preferences";
import { useReviewedFiles, type ReviewedFiles } from "@/state/reviewed-files";
import { REVIEW_SHORTCUTS, useReviewKeys } from "./reviewKeys";
import { patchFromBlocks } from "@/lib/unified-patch";
import { DiffHeader } from "@/components/DiffHeader";

/** Arrow-key resize increment, in pixels. */
const RESIZE_KEYBOARD_STEP = 16;

/** How long a scroll the panel asked for keeps the file it aimed at on screen. */
const SCROLL_SETTLE_MS = 600;

/** Names the full-screen dialog for assistive technology. */
const FULL_SCREEN_TITLE_ID = "changed-files-full-screen-title";

/** Turn id for grouped turns, or a stable key for the pre-turn bucket. */
function turnKey(turn: ChangeTurn): string {
  return turn.turnId !== undefined ? `t${turn.turnId}` : `e${turn.ordinal}`;
}

/**
 * A turn's generated group is its own, so opening one turn's generated files
 * leaves the next turn's closed.
 */
function generatedTurnKey(turn: ChangeTurn): string {
  return `${turnKey(turn)}/${GENERATED_GROUP_KEY}`;
}

/** Which two sides the panel compares. */
const CHANGES_SOURCES = ["run", "uncommitted", "branch"] as const;

/** How many branches the picker offers before the rest are left out. */
const BRANCH_CHOICES = 12;

const CHANGES_SOURCE_LABELS = {
  run: "This run",
  uncommitted: "Uncommitted",
  branch: "Against a branch",
} satisfies Record<ChangesSource, string>;

const CHANGES_SOURCE_HINTS = {
  run: "Files this run touched",
  uncommitted: "Not yet committed in this checkout",
  branch: "This checkout compared with another branch",
} satisfies Record<ChangesSource, string>;

const CHANGE_SORT_LABELS = {
  folder: "By folder",
  size: "By size",
} satisfies Record<ChangeSort, string>;

const CHANGE_SORT_HINTS = {
  folder: "Folder by folder, in the order the changes came in",
  size: "Biggest changes first",
} satisfies Record<ChangeSort, string>;

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

function fileCount(files: number, noun = "file"): string {
  return `${files} ${noun}${files === 1 ? "" : "s"}`;
}

function toggled(previous: Set<string>, key: string): Set<string> {
  const next = new Set(previous);
  if (!next.delete(key)) next.add(key);
  return next;
}

function without(previous: Set<string>, key: string): Set<string> {
  if (!previous.has(key)) return previous;
  const next = new Set(previous);
  next.delete(key);
  return next;
}

/** One row per path, first sighting wins, whichever list is walked. */
function uniqueFiles(files: readonly ChangedFileView[]): ChangedFileView[] {
  const seen = new Set<string>();
  const unique: ChangedFileView[] = [];
  for (const file of files) {
    if (seen.has(file.path)) continue;
    seen.add(file.path);
    unique.push(file);
  }
  return unique;
}

/** Closes an open popover on Escape or on a press anywhere outside it. */
function useDismissOutside(
  open: boolean,
  close: () => void,
  ref: React.RefObject<HTMLElement | null>,
): void {
  React.useEffect(() => {
    if (!open) return;
    const onPointer = (event: PointerEvent) => {
      // SAFETY: pointer events always target a Node in the DOM tree.
      const target = event.target as Node | null;
      if (target && ref.current?.contains(target)) return;
      close();
    };
    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      close();
      // Read in the capture phase, so one Escape takes one layer: the popover
      // here, and not the full-screen review or the composer behind it.
      event.stopPropagation();
    };
    document.addEventListener("pointerdown", onPointer);
    document.addEventListener("keydown", onEscape, true);
    return () => {
      document.removeEventListener("pointerdown", onPointer);
      document.removeEventListener("keydown", onEscape, true);
    };
  }, [open, close, ref]);
}

function ChangedFileRow({
  file,
  expanded,
  onToggle,
  onToggleReviewed,
  reviewed,
  reviewable,
  active,
  registerRow,
  registerHeading,
  showDiffHeader,
}: {
  file: ChangedFileView;
  expanded: boolean;
  onToggle: () => void;
  onToggleReviewed: () => void;
  reviewed: ReviewedFiles;
  /** False for generated files, which the review does not count. */
  reviewable: boolean;
  active: boolean;
  registerRow: (path: string, element: HTMLElement | null) => void;
  registerHeading: (path: string, element: HTMLElement | null) => void;
  showDiffHeader: boolean;
}) {
  const patch = React.useMemo(
    () => file.patch ?? patchFromBlocks(file.path, file.change.blocks),
    [file.patch, file.path, file.change.blocks],
  );
  const status = file.status === undefined || file.status === "modified" ? undefined : STATUS_LABELS[file.status];
  const isReviewed = reviewed.isReviewed(file);
  return (
    <section
      className={`changed-file-row${active ? " changed-file-row-active" : ""}${isReviewed ? " changed-file-row-reviewed" : ""}`}
      ref={(element) => registerRow(file.path, element)}
    >
      <div className="changed-file-head">
        <button
          className="changed-file-heading"
          type="button"
          aria-expanded={expanded}
          ref={(element) => registerHeading(file.path, element)}
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
        {reviewable && (
          <button
            className="icon-button changed-file-reviewed"
            type="button"
            aria-pressed={isReviewed}
            aria-label={`Mark ${file.path} reviewed`}
            title="Mark reviewed"
            onClick={onToggleReviewed}
          >
            {isReviewed ? <CheckCircleIcon /> : <CircleIcon />}
          </button>
        )}
      </div>
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
  reviewed: ReviewedFiles;
  onToggleReviewed: (file: ChangedFileView) => void;
  activePath?: string;
  registerRow: (path: string, element: HTMLElement | null) => void;
  registerHeading: (path: string, element: HTMLElement | null) => void;
  showDiffHeader: boolean;
}

function ChangedFileList({ files, expandedPaths, onToggleFile, reviewed, onToggleReviewed, reviewable, activePath, registerRow, registerHeading, showDiffHeader }: FileRowProps & { files: ChangedFileView[]; reviewable: boolean }) {
  return (
    <div className="changed-files-list">
      {files.map((file) => (
        <ChangedFileRow
          file={file}
          key={file.path}
          expanded={expandedPaths.has(file.path)}
          onToggle={() => onToggleFile(file.path)}
          onToggleReviewed={() => onToggleReviewed(file)}
          reviewed={reviewed}
          reviewable={reviewable}
          active={activePath === file.path}
          registerRow={registerRow}
          registerHeading={registerHeading}
          showDiffHeader={showDiffHeader}
        />
      ))}
    </div>
  );
}

function ChangeTurnGroup({
  entry,
  expanded,
  onToggleTurn,
  generatedOpen,
  onToggleGenerated,
  ...fileRowProps
}: FileRowProps & {
  entry: OrderedChangeTurn;
  expanded: boolean;
  onToggleTurn: () => void;
  generatedOpen: boolean;
  onToggleGenerated: () => void;
}) {
  const { turn } = entry;
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
      {expanded && (
        <>
          <ChangedFileList files={entry.files} reviewable {...fileRowProps} />
          {entry.generated.length > 0 && (
            <GeneratedGroup
              files={entry.generated}
              open={generatedOpen}
              onToggle={onToggleGenerated}
              {...fileRowProps}
            />
          )}
        </>
      )}
    </section>
  );
}

/**
 * Lockfiles, snapshots and build output, closed until a reader opens them.
 */
function GeneratedGroup({
  files,
  open,
  onToggle,
  ...fileRowProps
}: FileRowProps & { files: ChangedFileView[]; open: boolean; onToggle: () => void }) {
  return (
    <section className="changed-files-generated">
      <button
        className="changed-files-generated-heading"
        type="button"
        aria-expanded={open}
        title="Lockfiles, snapshots and build output"
        onClick={onToggle}
      >
        <span className="changed-file-disclosure" aria-hidden="true">
          <DisclosureIcon open={open} />
        </span>
        <span className="changed-files-generated-label">{GENERATED_GROUP_NAME}</span>
        <span className="changed-files-generated-meta">{fileCount(files.length)}</span>
      </button>
      {open && <ChangedFileList files={files} reviewable={false} {...fileRowProps} />}
    </section>
  );
}

function FileTreeNodes({
  nodes,
  activePath,
  reviewed,
  collapsedDirs,
  onToggleDir,
  onSelectFile,
}: {
  nodes: TreeNode[];
  activePath?: string;
  reviewed: ReviewedFiles;
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
                reviewed={reviewed}
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
              className={`changed-files-tree-file${activePath === node.path ? " changed-files-tree-file-active" : ""}${reviewed.isReviewed(node.file) ? " changed-files-tree-file-reviewed" : ""}`}
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

/** The most recent branches, with the one in use always among them. */
function branchChoices(branches: string[], base?: string): string[] {
  const offered = branches.slice(0, BRANCH_CHOICES);
  if (base === undefined || offered.includes(base)) return offered;
  return [base, ...offered.slice(0, BRANCH_CHOICES - 1)];
}

function SourcePicker({
  source,
  base,
  branches,
  onSourceChange,
  onBaseChange,
}: {
  source: ChangesSource;
  base?: string;
  branches: string[];
  onSourceChange: (source: ChangesSource) => void;
  onBaseChange: (branch: string) => void;
}) {
  const [open, setOpen] = React.useState(false);
  const label = source === "branch" && base !== undefined ? `Against ${base}` : CHANGES_SOURCE_LABELS[source];
  const sources = CHANGES_SOURCES.map((option) => ({
    key: option,
    label: CHANGES_SOURCE_LABELS[option],
    icon: source === option ? <CheckIcon /> : undefined,
    onSelect: () => {
      setOpen(false);
      onSourceChange(option);
    },
  }));
  const bases = branchChoices(branches, base).map((branch) => ({
    key: `branch:${branch}`,
    label: branch,
    icon: source === "branch" && base === branch ? <CheckIcon /> : undefined,
    onSelect: () => {
      setOpen(false);
      onBaseChange(branch);
    },
  }));
  return (
    <div className="changed-files-source-picker">
      <button
        className="changed-files-source-trigger"
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={`Show changes: ${label}`}
        title={CHANGES_SOURCE_HINTS[source]}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="changed-files-source-label">{label}</span>
        <span className="changed-files-source-chevron" aria-hidden="true">
          <ChevronIcon />
        </span>
      </button>
      {open && (
        <MenuPanel
          className="changed-files-source-menu"
          sections={[sources, bases]}
          onClose={() => setOpen(false)}
        />
      )}
    </div>
  );
}

function SortPicker({
  sort,
  onSortChange,
}: {
  sort: ChangeSort;
  onSortChange: (sort: ChangeSort) => void;
}) {
  const [open, setOpen] = React.useState(false);
  const label = CHANGE_SORT_LABELS[sort];
  const choices = CHANGE_SORTS.map((option) => ({
    key: option,
    label: CHANGE_SORT_LABELS[option],
    icon: sort === option ? <CheckIcon /> : undefined,
    onSelect: () => {
      setOpen(false);
      onSortChange(option);
    },
  }));
  return (
    <div className="changed-files-sort">
      <button
        className="text-button"
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={`Order files ${CHANGE_SORT_LABELS[sort].toLowerCase()}`}
        title={CHANGE_SORT_HINTS[sort]}
        onClick={() => setOpen((value) => !value)}
      >
        {label}
        <span className="changed-files-sort-chevron" aria-hidden="true">
          <ChevronIcon />
        </span>
      </button>
      {open && (
        <MenuPanel
          className="changed-files-sort-menu"
          sections={[choices]}
          onClose={() => setOpen(false)}
        />
      )}
    </div>
  );
}

/** The files and their state, shared by both chromes. */
export interface ChangedFilesProps {
  /** Marks are kept per task, so two tasks never share them. */
  taskId: string;
  source: ChangesSource;
  onSourceChange: (source: ChangesSource) => void;
  /** The branch the checkout is compared with, while one is selected. */
  base?: string;
  onBaseChange: (branch: string) => void;
  /** The branches the checkout offers as a comparison, most recent first. */
  branches: string[];
  groupByTurn: boolean;
  onGroupByTurn: (grouped: boolean) => void;
  /** Whether the panel lists files by folder or by size. */
  sort: ChangeSort;
  onSortChange: (sort: ChangeSort) => void;
  /** Reads the checkout again. Only offered for the two git comparisons. */
  onReload: () => void;
  /** The flat view of whichever comparison is showing. */
  changes: ChangedFileSet;
  /** The same files split by turn, present only for this run's own edits. */
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
  taskId,
  source,
  onSourceChange,
  base,
  onBaseChange,
  branches,
  groupByTurn,
  onGroupByTurn,
  sort,
  onSortChange,
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
  // Both the list and the file tree read the same order: the files a reviewer
  // reads, then the generated ones the panel keeps closed until they are asked for.
  const listed = React.useMemo(() => orderChangeFiles(changes.files, sort), [changes.files, sort]);
  const listedTurns = React.useMemo(
    () => (turns !== undefined && groupByTurn ? orderChangeTurns(turns, sort) : undefined),
    [turns, groupByTurn, sort],
  );
  const empty = listedTurns ? listedTurns.length === 0 : changes.files.length === 0;
  const tree = React.useMemo(
    () => buildFileTree(listed.files, listed.generated, sort),
    [listed, sort],
  );
  const reviewed = useReviewedFiles(taskId);

  const [filePopoverOpen, setFilePopoverOpen] = React.useState(false);
  const filePopoverRef = React.useRef<HTMLDivElement>(null);
  const [keysOpen, setKeysOpen] = React.useState(false);
  const keysRef = React.useRef<HTMLDivElement>(null);
  // The generated group is closed in the tree until a reader opens it.
  const [collapsedDirs, setCollapsedDirs] = React.useState<Set<string>>(() => new Set([GENERATED_GROUP_KEY]));
  // In the list, a generated group opens only when a reader asks for it.
  const [openGenerated, setOpenGenerated] = React.useState<Set<string>>(new Set());
  const [expandedPaths, setExpandedPaths] = React.useState<Set<string>>(new Set());
  const [expandedTurns, setExpandedTurns] = React.useState<Set<string>>(new Set());
  const [activePath, setActivePath] = React.useState<string | undefined>(undefined);
  const [scrollRequest, setScrollRequest] = React.useState<{ path: string; takeFocus?: boolean } | undefined>(undefined);
  const [viewRoot, setViewRoot] = React.useState<HTMLDivElement | null>(null);
  const seenTurnsRef = React.useRef<Set<string>>(new Set());
  const rowElementsRef = React.useRef<Map<string, HTMLElement>>(new Map());
  const headingElementsRef = React.useRef<Map<string, HTMLElement>>(new Map());
  const scrollFrameRef = React.useRef<number | undefined>(undefined);
  const heldUntilRef = React.useRef(0);

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

  const closeFilePopover = React.useCallback(() => setFilePopoverOpen(false), []);
  const closeKeys = React.useCallback(() => setKeysOpen(false), []);
  useDismissOutside(filePopoverOpen, closeFilePopover, filePopoverRef);
  useDismissOutside(keysOpen, closeKeys, keysRef);

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
        // The row takes the focus rather than the document, so Enter and the
        // rest of the review keys act on the file that just came into view.
        if (scrollRequest.takeFocus) {
          headingElementsRef.current.get(scrollRequest.path)?.focus({ preventScroll: true });
        }
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

  const registerHeading = React.useCallback((path: string, element: HTMLElement | null) => {
    if (element) headingElementsRef.current.set(path, element);
  }, []);

  const toggleDir = (path: string) => {
    setCollapsedDirs((prev) => toggled(prev, path));
  };

  const toggleFile = React.useCallback((path: string) => {
    setExpandedPaths((prev) => toggled(prev, path));
  }, []);

  const toggleTurn = (key: string) => {
    setExpandedTurns((prev) => toggled(prev, key));
  };

  const toggleGenerated = (key: string) => {
    setOpenGenerated((prev) => toggled(prev, key));
  };

  // The files a reviewer reads, across turns: what the progress counts and what
  // Open all opens. A generated file is in neither.
  const toReview = React.useMemo(
    () => (listedTurns ? uniqueFiles(listedTurns.flatMap((entry) => entry.files)) : listed.files),
    [listedTurns, listed.files],
  );

  /** Which generated group holds a file, so a key that asks for one can open it. */
  const generatedKeys = React.useMemo(() => {
    const keys = new Map<string, string>();
    for (const file of listed.generated) keys.set(file.path, GENERATED_GROUP_KEY);
    for (const entry of listedTurns ?? []) {
      for (const file of entry.generated) keys.set(file.path, generatedTurnKey(entry.turn));
    }
    return keys;
  }, [listed.generated, listedTurns]);

  // Marking a file retires it: the diff closes, so the next one is the only one
  // left open. The file keeps the panel's place, so the next key moves off the
  // file just read rather than back onto it.
  const toggleReviewed = React.useCallback((file: ChangedFileView) => {
    // A generated file is outside the review, so there is no mark to set on it.
    if (generatedKeys.has(file.path)) return;
    if (!reviewed.isReviewed(file)) {
      setExpandedPaths((prev) => without(prev, file.path));
    }
    reviewed.toggle(file);
  }, [reviewed, generatedKeys]);

  const openAll = () => {
    setExpandedPaths(new Set(toReview.map((file) => file.path)));
    // Grouped, a file stays out of sight until its turn is open.
    if (listedTurns) setExpandedTurns(new Set(listedTurns.map((entry) => turnKey(entry.turn))));
  };

  const closeAll = () => setExpandedPaths(new Set());

  const revealFile = React.useCallback((path: string) => {
    if (turns) {
      setExpandedTurns((prev) => {
        const next = new Set(prev);
        for (const turn of turns.turns) {
          if (turn.files.some((file) => file.path === path)) next.add(turnKey(turn));
        }
        return next;
      });
    }
    // A generated file sits in a group the reader left closed, so asking for it
    // opens the group too.
    const group = generatedKeys.get(path);
    if (group !== undefined) setOpenGenerated((prev) => new Set(prev).add(group));
    setExpandedPaths((prev) => new Set(prev).add(path));
  }, [turns, generatedKeys]);

  // The file the reader picked holds the panel still for a moment, so the row
  // it asked for stays the one on screen while the list glides to it.
  const requestScroll = (path: string, takeFocus = false) => {
    heldUntilRef.current = Date.now() + SCROLL_SETTLE_MS;
    setScrollRequest({ path, takeFocus });
  };

  const selectFile = (path: string) => {
    revealFile(path);
    requestScroll(path);
  };

  /** Opens the file, brings it into view, and puts the keyboard on it. */
  const goTo = React.useCallback((path: string) => {
    revealFile(path);
    setActivePath(path);
    requestScroll(path, true);
  }, [revealFile]);

  // Scrolling by hand means it: the file on screen follows the reader's own
  // scroll from here on, glide or no glide.
  const endScrollHold = () => {
    heldUntilRef.current = 0;
  };

  // The files as the panel lists them, which is the order the keys walk: grouped
  // means turn by turn, each turn's generated files just after its own, and a
  // file two turns touched is one file to move through.
  const order = React.useMemo(() => {
    if (!listedTurns) return [...listed.files, ...listed.generated];
    return uniqueFiles(listedTurns.flatMap((entry) => [...entry.files, ...entry.generated]));
  }, [listed, listedTurns]);

  const activeFile = React.useMemo(
    () => order.find((file) => file.path === activePath),
    [order, activePath],
  );

  // A generated file needs no mark, so the key that skips to the next unread
  // file steps over the group instead of landing on a lockfile.
  const countsAsReviewed = React.useCallback(
    (file: ChangedFileView) => generatedKeys.has(file.path) || reviewed.isReviewed(file),
    [generatedKeys, reviewed],
  );

  useReviewKeys({
    root: viewRoot,
    files: order,
    activeFile,
    isReviewed: countsAsReviewed,
    goTo,
    toggleOpen: toggleFile,
    toggleReviewed,
  });

  const handleDiffsScroll = (event: React.UIEvent<HTMLDivElement>) => {
    if (scrollFrameRef.current !== undefined) return;
    const container = event.currentTarget;
    scrollFrameRef.current = requestAnimationFrame(() => {
      scrollFrameRef.current = undefined;
      // Scrolling the panel moves the file on screen, except while it is still
      // gliding to the file a click or a key asked for: that one is the file
      // the reader chose, whatever the top of the list happens to be.
      if (Date.now() < heldUntilRef.current) return;
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

  // Both counts read the files the review asks about, so a path left behind by a
  // collapsed turn cannot make the header claim the wrong thing.
  const allOpen = toReview.length > 0 && toReview.every((file) => expandedPaths.has(file.path));
  const noneOpen = toReview.every((file) => !expandedPaths.has(file.path));

  const fileRowProps: FileRowProps = {
    expandedPaths,
    onToggleFile: toggleFile,
    reviewed,
    onToggleReviewed: toggleReviewed,
    activePath,
    registerRow,
    registerHeading,
    showDiffHeader: !full,
  };

  const fileTree = (
    <FileTreeNodes
      nodes={tree}
      activePath={activePath}
      reviewed={reviewed}
      collapsedDirs={collapsedDirs}
      onToggleDir={toggleDir}
      onSelectFile={(path) => {
        selectFile(path);
        setFilePopoverOpen(false);
      }}
    />
  );

  const generatedGroup = (files: ChangedFileView[], key: string) =>
    files.length === 0 ? null : (
      <GeneratedGroup
        files={files}
        key={key}
        open={openGenerated.has(key)}
        onToggle={() => toggleGenerated(key)}
        {...fileRowProps}
      />
    );

  const diffs = (
    <div
      className="changed-files-diffs"
      onScroll={handleDiffsScroll}
      onWheel={endScrollHold}
      onTouchMove={endScrollHold}
    >
      {listedTurns ? (
        <div className="changed-files-turns">
          {listedTurns.map((entry) => (
            <ChangeTurnGroup
              key={turnKey(entry.turn)}
              entry={entry}
              expanded={expandedTurns.has(turnKey(entry.turn))}
              onToggleTurn={() => toggleTurn(turnKey(entry.turn))}
              generatedOpen={openGenerated.has(generatedTurnKey(entry.turn))}
              onToggleGenerated={() => toggleGenerated(generatedTurnKey(entry.turn))}
              {...fileRowProps}
            />
          ))}
        </div>
      ) : (
        <>
          <ChangedFileList files={listed.files} reviewable {...fileRowProps} />
          {generatedGroup(listed.generated, GENERATED_GROUP_KEY)}
        </>
      )}
    </div>
  );

  return (
    <div className="changed-files-view" ref={setViewRoot}>
      <header className={`changed-files-header${full ? " changed-files-header-full" : ""}`}>
        <div className="changed-files-header-row">
          <div className="changed-files-header-title">
            <h2 className="visually-hidden" id={full ? FULL_SCREEN_TITLE_ID : undefined}>
              Changed files
            </h2>
            <SourcePicker
              source={source}
              base={base}
              branches={branches}
              onSourceChange={onSourceChange}
              onBaseChange={onBaseChange}
            />
            {activePath !== undefined ? (
              <p className="changed-files-active-file" title={activePath}>
                {activePath}
              </p>
            ) : (
              changes.files.length > 0 && (
                <p className="changed-files-summary">
                  {`${fileCount(listedTurns ? listedTurns.reduce((count, entry) => count + entry.files.length + entry.generated.length, 0) : changes.files.length)} · `}
                  <span className="diff-stat-added">{`+${added}`}</span>
                  {" · "}
                  <span className="diff-stat-removed">{`-${removed}`}</span>
                </p>
              )
            )}
            {toReview.length > 0 && (
              <p className="changed-files-reviewed-progress">
                {`${reviewed.count(toReview)} of ${toReview.length} reviewed`}
              </p>
            )}
            {listed.generated.length > 0 && (
              <p className="changed-files-generated-count">
                {`${fileCount(listed.generated.length, "generated file")}, not part of the review`}
              </p>
            )}
          </div>
          <div className="changed-files-header-actions">
            {full && <DiffHeader />}
            {!empty && error === undefined && <SortPicker sort={sort} onSortChange={onSortChange} />}
            {source === "run" ? (
              <label className="changed-files-group">
                <input type="checkbox" checked={groupByTurn} onChange={(event) => onGroupByTurn(event.target.checked)} />
                Group by turn
              </label>
            ) : null}
            {full && !empty && (
              <>
                <button className="text-button" type="button" disabled={allOpen} onClick={openAll}>
                  Open all
                </button>
                <button className="text-button" type="button" disabled={noneOpen} onClick={closeAll}>
                  Close all
                </button>
              </>
            )}
            {!empty && error === undefined && (
              <div className="changed-files-keys" ref={keysRef}>
                <button
                  className="text-button changed-files-keys-trigger"
                  type="button"
                  aria-haspopup="true"
                  aria-expanded={keysOpen}
                  title="Keys for reviewing files"
                  onClick={() => setKeysOpen((value) => !value)}
                >
                  <kbd className="keycap" aria-hidden="true">?</kbd>
                  <span className="visually-hidden">Keys for reviewing files</span>
                </button>
                {keysOpen && (
                  <div className="changed-files-keys-panel">
                    <p className="changed-files-keys-heading">While the changed files have focus</p>
                    <dl className="changed-files-keys-list">
                      {REVIEW_SHORTCUTS.map((row) => (
                        <div className="changed-files-keys-row" key={row.action}>
                          <dt className="changed-files-keys-cap">
                            {row.keys.map((combo) => (
                              <span className="changed-files-keys-combo" key={combo.join("")}>
                                {combo.map((key) => (
                                  <kbd className="keycap" key={key}>{key}</kbd>
                                ))}
                              </span>
                            ))}
                          </dt>
                          <dd className="changed-files-keys-action">{row.action}</dd>
                        </div>
                      ))}
                    </dl>
                  </div>
                )}
              </div>
            )}
            <button
              className="icon-button"
              type="button"
              disabled={loading || source === "run"}
              aria-label="Refresh"
              title={source === "run" ? "This run's edits appear on their own" : "Refresh"}
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
            hint={live && source === "run" ? "Changes appear here as the run continues." : "Changes appear here when this run edits files."}
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
        {hasEarlier && source === "run" && (
          <div className="changed-files-earlier">
            <p>Earlier activity is not loaded, so this may not be the whole run.</p>
            <button className="text-button" type="button" disabled={loadingEarlier} onClick={onLoadEarlier}>
              {loadingEarlier ? "Loading earlier activity…" : "Load earlier activity"}
            </button>
          </div>
        )}
      </div>
    </div>
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
