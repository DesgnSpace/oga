// Task detail screen: header, transcript, controls, and the changed-files panel.
// Ported from rust/crates/oga-ui/src/task_detail/mod.rs's `TaskDetail` component.

import * as React from "react";
import { broker } from "@/bridge/client";
import type { TaskDiff, TaskEventView } from "@/bridge/types";
import { collectRunChanges, runChangeSetAdded, RUN_CHANGES_EMPTY } from "@/domain/changes";
import { collectRunChangesByTurn, gitChangeSet } from "@/domain/changes/grouped";
import { formatCost, formatTokenCount, taskWallTime } from "@/lib/format";
import { watchTaskDetail, type TaskDetailState } from "@/state/taskDetail";
import type { TaskTitleBarInfo } from "@/shell/TitleBar";
import {
  CHANGED_FILES_DEFAULT_WIDTH,
  clampChangedFilesWidth,
  loadChangesGrouped,
  loadChangesSource,
  loadChangedFilesWidth,
  storeChangesGrouped,
  storeChangesSource,
  storeChangedFilesWidth,
} from "@/state/changed-files-preferences";
import { TaskControls, TaskHeaderActions, WaitNotice } from "./Actions";
import { ChangedFilesPanel, type ChangesSource } from "./ChangedFiles";
import { effortDisplay, taskStatusLabel } from "./format";
import { Transcript } from "./Transcript";
import { activityIsSettled, buildTranscript, WorkSegmentCache } from "./transcriptModel";

/** Dispatched by the native "Refresh" menu command to reload the open task alongside the sidebar. */
export const REFRESH_TASK_DETAIL_EVENT = "oga-refresh-task-detail";

const FRAME_MS = 16;

/**
 * Redraws at most once a frame. A busy worker pushes updates faster than the
 * transcript can be recomposed, and a reader cannot see more than a frame's
 * worth anyway, so a burst costs one recomposition instead of one each.
 */
function useForceUpdate(): () => void {
  const [, setTick] = React.useState(0);
  const scheduled = React.useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  React.useEffect(() => () => clearTimeout(scheduled.current), []);
  return React.useCallback(() => {
    if (scheduled.current !== undefined) return;
    scheduled.current = setTimeout(() => {
      scheduled.current = undefined;
      setTick((tick) => tick + 1);
    }, FRAME_MS);
  }, []);
}

interface GitDiffState {
  loading: boolean;
  diff?: TaskDiff;
  error?: string;
}

/**
 * Reads the task's checkout while the panel is showing git, and again on
 * demand. A live task is not re-read on its own: the reported view is the one
 * that follows a worker, and every re-read costs a git process.
 */
function useGitDiff(taskId: string, active: boolean): GitDiffState & { reload: () => void } {
  const [state, setState] = React.useState<GitDiffState>({ loading: false });
  const [attempt, setAttempt] = React.useState(0);
  React.useEffect(() => {
    if (!active) return;
    let cancelled = false;
    setState({ loading: true });
    void broker.taskDiff(taskId).then((result) => {
      if (cancelled) return;
      setState(result.ok ? { loading: false, diff: result.value } : { loading: false, error: result.error.message });
    });
    return () => {
      cancelled = true;
    };
  }, [taskId, active, attempt]);
  return { ...state, reload: () => setAttempt((value) => value + 1) };
}

interface UsageTotals {
  tokensIn: number;
  tokensOut: number;
  tokensCached: number;
}

function usageTotals(events: TaskEventView[]): UsageTotals {
  let tokensIn = 0;
  let tokensOut = 0;
  let tokensCached = 0;
  for (const event of events) {
    const presentation = event.presentation;
    if (!presentation) continue;
    tokensIn += presentation.tokensIn ?? 0;
    tokensOut += presentation.tokensOut ?? 0;
    tokensCached += presentation.tokensCached ?? 0;
  }
  return { tokensIn, tokensOut, tokensCached };
}

interface StatItem {
  text: string;
  title?: string;
}

function taskDetailStatItems(task: NonNullable<TaskDetailState["task"]>, events: TaskEventView[]): StatItem[] {
  const cost = formatCost(task.costUsd, task.costUsdEstimated);
  const { tokensIn, tokensOut, tokensCached } = usageTotals(events);
  const wallTime = taskWallTime(task.createdAt, task.updatedAt, !activityIsSettled(task.state));

  const items: StatItem[] = [];
  if (cost) items.push({ text: cost, title: task.costUsdEstimated ? "Estimated from public pricing" : undefined });
  if (task.turns !== undefined && task.turns > 0) items.push({ text: `${task.turns} turn${task.turns === 1 ? "" : "s"}` });
  if (wallTime) items.push({ text: wallTime });
  if (tokensIn > 0) items.push({ text: `${formatTokenCount(tokensIn)} in` });
  if (tokensOut > 0) items.push({ text: `${formatTokenCount(tokensOut)} out` });
  if (tokensCached > 0) items.push({ text: `${formatTokenCount(tokensCached)} cached` });
  return items;
}

/** The title bar's secondary strip: status, model, and usage detail in one
 * row that scrolls sideways instead of wrapping the title bar underneath it. */
function TaskDetailSecondary({
  task,
  events,
  onChanged,
}: {
  task: NonNullable<TaskDetailState["task"]>;
  events: TaskEventView[];
  onChanged: () => void;
}) {
  const effort = effortDisplay(task);
  const items = taskDetailStatItems(task, events);
  return (
    <div className="title-bar-secondary-row" aria-label="Task status and usage">
      <span className="task-detail-fact" title={task.model}>
        {task.model}
        {effort && (
          <span title={effort.title}>
            {" · "}
            {effort.label}
          </span>
        )}
      </span>
      {items.map((item, index) => (
        <span key={index} className="title-bar-stat-group">
          <span className="title-bar-stat-separator" aria-hidden="true">·</span>
          <span className="task-detail-stat" title={item.title}>{item.text}</span>
        </span>
      ))}
      <span className="title-bar-secondary-spacer" />
      <TaskHeaderActions task={task} onChanged={onChanged} />
      <WaitNotice task={task} onChanged={onChanged} />
    </div>
  );
}

export function TaskDetail({ taskId, onHeader }: { taskId: string; onHeader: (info: TaskTitleBarInfo | undefined) => void }) {
  const forceUpdate = useForceUpdate();
  const [showingChanges, setShowingChanges] = React.useState(false);
  const [changesSource, setChangesSource] = React.useState<ChangesSource>(loadChangesSource);
  const [groupByTurn, setGroupByTurn] = React.useState(loadChangesGrouped);
  const [changedFilesWidth, setChangedFilesWidth] = React.useState(loadChangedFilesWidth);
  const [resizeStart, setResizeStart] = React.useState<{ x: number; width: number } | null>(null);

  const applyChangedFilesWidth = React.useCallback((width: number) => {
    const clamped = clampChangedFilesWidth(width);
    setChangedFilesWidth(clamped);
    storeChangedFilesWidth(clamped);
  }, []);

  // The changed-files panel is dragged from its left edge, so moving the
  // pointer right shrinks it and moving left grows it.
  React.useEffect(() => {
    if (!resizeStart) return;
    const onMove = (event: PointerEvent) => {
      applyChangedFilesWidth(resizeStart.width - (event.clientX - resizeStart.x));
    };
    const onEnd = () => setResizeStart(null);
    document.addEventListener("pointermove", onMove);
    document.addEventListener("pointerup", onEnd);
    document.addEventListener("pointercancel", onEnd);
    return () => {
      document.removeEventListener("pointermove", onMove);
      document.removeEventListener("pointerup", onEnd);
      document.removeEventListener("pointercancel", onEnd);
    };
  }, [resizeStart, applyChangedFilesWidth]);

  const watched = React.useMemo(() => watchTaskDetail(taskId), [taskId]);
  React.useEffect(() => {
    const unsubscribe = watched.controller.subscribe(forceUpdate);
    return () => {
      unsubscribe();
      watched.dispose();
    };
  }, [watched, forceUpdate]);

  const state = watched.controller.snapshot;
  const task = state.task;

  // The newest activity is what a reader opens the task for, so the view sits
  // at the end and stays there as activity arrives — until they scroll away,
  // after which it holds their place.
  const contentRef = React.useRef<HTMLDivElement>(null);
  const stickToEnd = React.useRef(true);
  React.useEffect(() => {
    stickToEnd.current = true;
  }, [taskId]);
  const trackScroll = (event: React.UIEvent<HTMLDivElement>) => {
    const content = event.currentTarget;
    stickToEnd.current = content.scrollHeight - content.scrollTop - content.clientHeight < 24;
  };

  // The sentinel sits above the transcript; entering view triggers the next
  // page, and the scroll offset is restored afterward so the list doesn't jump.
  const topSentinelRef = React.useRef<HTMLDivElement>(null);
  const loadEarlier = React.useCallback(() => {
    const content = contentRef.current;
    const state = watched.controller.snapshot;
    if (!content || state.loadingEarlier || !state.hasEarlier) return;
    const previousScrollHeight = content.scrollHeight;
    const previousScrollTop = content.scrollTop;
    void watched.controller.loadEarlier().then(() => {
      forceUpdate();
      requestAnimationFrame(() => {
        if (!content) return;
        content.scrollTop = previousScrollTop + (content.scrollHeight - previousScrollHeight);
      });
    });
  }, [watched, forceUpdate]);

  React.useEffect(() => {
    const sentinel = topSentinelRef.current;
    const content = contentRef.current;
    if (!sentinel || !content) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) loadEarlier();
      },
      { root: content },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [loadEarlier, task, state.hasEarlier]);

  const events = watched.controller.withEvents((all) => all);
  const segmentCache = React.useMemo(() => new WorkSegmentCache(), [taskId]);
  const transcriptItems = React.useMemo(
    () => (task ? buildTranscript(task, events, segmentCache) : []),
    [task, events, segmentCache],
  );
  React.useLayoutEffect(() => {
    const content = contentRef.current;
    if (content && stickToEnd.current) content.scrollTop = content.scrollHeight;
  }, [transcriptItems]);

  const retry = React.useCallback(() => {
    void watched.controller.loadInitial().then(forceUpdate);
  }, [watched, forceUpdate]);
  const refreshDetail = React.useCallback(() => {
    void watched.controller.loadInitial().then(forceUpdate);
  }, [watched, forceUpdate]);

  const git = useGitDiff(taskId, showingChanges && changesSource === "git");
  const chooseSource = (source: ChangesSource) => {
    setChangesSource(source);
    storeChangesSource(source);
  };
  const chooseGrouping = (grouped: boolean) => {
    setGroupByTurn(grouped);
    storeChangesGrouped(grouped);
  };
  const reportedChanges = React.useMemo(
    () => (task ? collectRunChanges(events, task.cwd) : RUN_CHANGES_EMPTY),
    [task, events],
  );
  const reportedTurns = React.useMemo(
    () => (task && groupByTurn ? collectRunChangesByTurn(events, task.cwd) : undefined),
    [task, events, groupByTurn],
  );
  const gitChanges = React.useMemo(
    () => (git.diff ? gitChangeSet(git.diff) : RUN_CHANGES_EMPTY),
    [git.diff],
  );

  React.useEffect(() => {
    if (!task) {
      onHeader(undefined);
      return;
    }
    const title = task.title ?? task.tldr ?? task.prompt.split("\n").find((line) => line.trim() !== "") ?? "Untitled task";
    const statusLabel = taskStatusLabel(task);
    onHeader({
      title,
      diffAdded: runChangeSetAdded(reportedChanges),
      showingChanges,
      onToggleChanges: () => setShowingChanges((value) => !value),
      status: (
        <span
          className={`task-dot task-dot-${task.state}`}
          role="img"
          aria-label={statusLabel}
          title={statusLabel}
        />
      ),
      secondary: <TaskDetailSecondary task={task} events={events} onChanged={refreshDetail} />,
    });
    return () => onHeader(undefined);
  }, [task, reportedChanges, showingChanges, events, onHeader]);

  React.useEffect(() => {
    const listener = () => refreshDetail();
    window.addEventListener(REFRESH_TASK_DETAIL_EVENT, listener);
    return () => window.removeEventListener(REFRESH_TASK_DETAIL_EVENT, listener);
  }, [taskId]);

  return (
    <div
      className={`task-detail${showingChanges ? " task-detail-with-changes" : ""}`}
      // SAFETY: React.CSSProperties permits custom properties at runtime; the type just doesn't model them.
      style={showingChanges ? ({ "--changed-files-width": `${changedFilesWidth}px` } as React.CSSProperties) : undefined}
    >
      <div className="task-detail-primary">
        {!task ? (
          state.loading ? (
            <p className="detail-message">Loading task activity…</p>
          ) : (
            <div className="detail-message detail-message-error">
              <p>We couldn't load this task. Check the connection and try again.</p>
              <button className="load-more" type="button" onClick={retry}>
                Try again
              </button>
            </div>
          )
        ) : (
          <>
            {state.error !== undefined && (
              <div className="detail-refresh-error" role="alert">
                <span>We couldn't update activity. Try again.</span>
                <button className="text-button" type="button" onClick={retry}>
                  Try again
                </button>
              </div>
            )}
            <div className="task-detail-content-main" ref={contentRef} onScroll={trackScroll}>
              {state.hasEarlier && (
                <div ref={topSentinelRef} className="transcript-load-earlier" aria-hidden={!state.loadingEarlier}>
                  {state.loadingEarlier && <span className="transcript-load-earlier-spinner" />}
                </div>
              )}
              <Transcript items={transcriptItems} cwd={task.cwd} />
            </div>
          </>
        )}
        {task && <TaskControls task={task} onChanged={refreshDetail} />}
      </div>
      {showingChanges && task && (
        <ChangedFilesPanel
          key={taskId}
          source={changesSource}
          onSourceChange={chooseSource}
          groupByTurn={groupByTurn}
          onGroupByTurn={chooseGrouping}
          onReload={git.reload}
          changes={changesSource === "git" ? gitChanges : reportedChanges}
          turns={changesSource === "git" ? undefined : reportedTurns}
          loading={changesSource === "git" ? git.loading : state.loading}
          error={changesSource === "git" ? git.error : undefined}
          truncated={changesSource === "git" && git.diff?.truncated === true}
          live={!activityIsSettled(task.state)}
          hasEarlier={state.hasEarlier}
          loadingEarlier={state.loadingEarlier}
          onLoadEarlier={loadEarlier}
          onClose={() => setShowingChanges(false)}
          width={changedFilesWidth}
          onResizeStart={(clientX) => setResizeStart({ x: clientX, width: changedFilesWidth })}
          onResetWidth={() => applyChangedFilesWidth(CHANGED_FILES_DEFAULT_WIDTH)}
          onResizeStep={(deltaWidth) => applyChangedFilesWidth(changedFilesWidth + deltaWidth)}
        />
      )}
    </div>
  );
}
