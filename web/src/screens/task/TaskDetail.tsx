// Task detail screen: header, transcript, controls, and changed files.

import * as React from "react";
import { broker } from "@/bridge/client";
import type { ProfileView, TaskDiff, TaskEventView } from "@/bridge/types";
import { LiveDuration } from "@/components/atoms/LiveDuration";
import { TaskStatusDot } from "@/components/atoms/TaskStatusDot";
import { RunChangeProjection, runChangeSetAdded, RUN_CHANGES_EMPTY } from "@/domain/changes";
import { gitChangeSet, RunChangeByTurnProjection } from "@/domain/changes/grouped";
import { formatCost, formatTokenCount } from "@/lib/format";
import { absoluteTime } from "@/ui/time";
import { watchTaskDetail, type TaskDetailState } from "@/state/taskDetail";
import { taskOutcomeKey, taskOutcomeViews } from "@/state/task-outcome-views";
import type { TaskTitleBarInfo } from "@/shell/TitleBar";
import {
  CHANGED_FILES_DEFAULT_WIDTH,
  clampChangedFilesWidth,
  loadChangesBase,
  loadChangesGrouped,
  loadChangesSource,
  loadChangedFilesWidth,
  storeChangesBase,
  storeChangesGrouped,
  storeChangesSource,
  storeChangedFilesWidth,
  type ChangesSource,
} from "@/state/changed-files-preferences";
import { TaskControls, TaskHeaderActions, WaitNotice } from "./Actions";
import { terminalResumeCommand } from "./terminalResume";
import { ChangedFilesFullScreen, ChangedFilesPanel, type ChangedFilesProps } from "./ChangedFiles";
import { usageTotals } from "./contextUsage";
import { effortDisplay, taskStatusLabel } from "./format";
import { useShowThinking } from "./Trace";
import { Transcript, transcriptHasThinking } from "./Transcript";
import { activityIsSettled, buildTranscript, WorkSegmentCache } from "./transcriptModel";

export const REFRESH_TASK_DETAIL_EVENT = "oga-refresh-task-detail";

const FRAME_MS = 16;

// A busy worker can push more updates than a reader can see per frame.
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

// Git diff reads are explicit because each one starts a process.
function useGitDiff(taskId: string, against: string | undefined): GitDiffState & { reload: () => void } {
  const [state, setState] = React.useState<GitDiffState>({ loading: false });
  const [attempt, setAttempt] = React.useState(0);
  React.useEffect(() => {
    if (against === undefined) return;
    let cancelled = false;
    setState({ loading: true });
    void broker.taskDiff(taskId, against).then((result) => {
      if (cancelled) return;
      setState(result.ok ? { loading: false, diff: result.value } : { loading: false, error: result.error.message });
    });
    return () => {
      cancelled = true;
    };
  }, [taskId, against, attempt]);
  return { ...state, reload: () => setAttempt((value) => value + 1) };
}

interface BranchChoices {
  loading: boolean;
  branches: string[];
  default?: string;
}

function useTaskBranches(taskId: string, active: boolean): BranchChoices {
  const [state, setState] = React.useState<BranchChoices>({ loading: false, branches: [] });
  React.useEffect(() => {
    if (!active) return;
    let cancelled = false;
    setState({ loading: true, branches: [] });
    void broker.taskBranches(taskId).then((result) => {
      if (cancelled) return;
      setState(
        result.ok
          ? { loading: false, branches: result.value.branches, default: result.value.default }
          : { loading: false, branches: [] },
      );
    });
    return () => {
      cancelled = true;
    };
  }, [taskId, active]);
  return state;
}

interface StatItem {
  text: React.ReactNode;
  title?: string;
}

function durationTitle(task: NonNullable<TaskDetailState["task"]>): string | undefined {
  const running = !activityIsSettled(task.state);
  if (running) return task.runningSince ? `Started ${absoluteTime(task.runningSince)}` : undefined;
  if (task.durationMs === undefined) return undefined;
  const finishedAt = task.updatedAt;
  const startedAt = new Date(new Date(finishedAt).getTime() - task.durationMs);
  return `Started ${absoluteTime(startedAt)} · Finished ${absoluteTime(finishedAt)}`;
}

function taskDetailStatItems(task: NonNullable<TaskDetailState["task"]>, events: TaskEventView[]): StatItem[] {
  const cost = formatCost(task.costUsd, task.costUsdEstimated);
  const { tokensIn, tokensOut, tokensCached } = usageTotals(events);

  const items: StatItem[] = [];
  if (cost) items.push({ text: cost, title: task.costUsdEstimated ? "Estimated from public pricing" : undefined });
  if (task.turns !== undefined && task.turns > 0) items.push({ text: `${task.turns} turn${task.turns === 1 ? "" : "s"}` });
  items.push({
    text: <LiveDuration durationMs={task.durationMs} runningSince={task.runningSince} running={!activityIsSettled(task.state)} />,
    title: durationTitle(task),
  });
  if (tokensIn > 0) items.push({ text: `${formatTokenCount(tokensIn)} in` });
  if (tokensOut > 0) items.push({ text: `${formatTokenCount(tokensOut)} out` });
  if (tokensCached > 0) items.push({ text: `${formatTokenCount(tokensCached)} cached` });
  return items;
}

// The title bar's secondary strip scrolls instead of wrapping the bar.
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
        <span className="task-detail-model">{task.model}</span>
        {effort && (
          <span className="task-detail-effort" title={effort.title}>
            {" · "}
            {effort.label.charAt(0).toUpperCase() + effort.label.slice(1)}
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

export function TaskDetail({ taskId, onHeader, focusRequest, onFocusRequestConsumed }: {
  taskId: string;
  onHeader: (info: TaskTitleBarInfo | undefined) => void;
  focusRequest?: { taskId: string; nonce: number };
  onFocusRequestConsumed: (nonce: number) => void;
}) {
  const forceUpdate = useForceUpdate();
  const [showingChanges, setShowingChanges] = React.useState(false);
  const [reviewingChanges, setReviewingChanges] = React.useState(false);
  const [changesSource, setChangesSource] = React.useState<ChangesSource>(loadChangesSource);
  const [changesBase, setChangesBase] = React.useState<string | undefined>(loadChangesBase);
  const [groupByTurn, setGroupByTurn] = React.useState(loadChangesGrouped);
  const [changedFilesWidth, setChangedFilesWidth] = React.useState(loadChangedFilesWidth);
  const [resizeStart, setResizeStart] = React.useState<{ x: number; width: number } | null>(null);
  const [profiles, setProfiles] = React.useState<ProfileView[] | undefined>(undefined);

  const applyChangedFilesWidth = React.useCallback((width: number) => {
    const clamped = clampChangedFilesWidth(width);
    setChangedFilesWidth(clamped);
    storeChangedFilesWidth(clamped);
  }, []);

// The panel is dragged from its left edge: right shrinks, left grows.
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
  React.useLayoutEffect(() => {
    taskOutcomeViews.setActiveTask(taskId);
    return () => taskOutcomeViews.clearActiveTask(taskId);
  }, [taskId]);
  React.useEffect(() => {
    const unsubscribe = watched.controller.subscribe(forceUpdate);
    return () => {
      unsubscribe();
      watched.dispose();
    };
  }, [watched, forceUpdate]);

  const state = watched.controller.snapshot;
  const task = state.task;
  const currentOutcome = task ? taskOutcomeKey(task) : undefined;
  const eventRevision = state.revision;
  const viewState = watched.controller.viewState;

  React.useEffect(() => {
    if (task) taskOutcomeViews.markViewed(task);
  }, [task, currentOutcome]);

  // The newest activity is what a reader opens the task for, so the view sits
  // at the end and stays there as activity arrives — until they scroll away,
  // after which it holds their place.
  const contentRef = React.useRef<HTMLDivElement>(null);
  const stickToEnd = React.useRef(true);
  React.useLayoutEffect(() => {
    stickToEnd.current = viewState.stickToEnd;
  }, [taskId, viewState]);
  const trackScroll = (event: React.UIEvent<HTMLDivElement>) => {
    const content = event.currentTarget;
    const atEnd = content.scrollHeight - content.scrollTop - content.clientHeight < 24;
    stickToEnd.current = atEnd;
    watched.controller.setScrollPosition(content.scrollTop, atEnd);
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
        watched.controller.setScrollPosition(content.scrollTop, stickToEnd.current);
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
  const changeProjection = React.useMemo(() => new RunChangeProjection(), [taskId]);
  const changeTurnsProjection = React.useMemo(() => new RunChangeByTurnProjection(), [taskId]);
  const transcriptItems = React.useMemo(
    () => (task ? buildTranscript(task, events, segmentCache) : []),
    [task, eventRevision, events, segmentCache],
  );
  const [showThinking, toggleThinking] = useShowThinking();
  const hasThinking = React.useMemo(() => transcriptHasThinking(transcriptItems), [transcriptItems]);
  React.useLayoutEffect(() => {
    const content = contentRef.current;
    if (!content) return;
    if (stickToEnd.current) {
      content.scrollTop = content.scrollHeight;
      return;
    }
    const maximum = Math.max(0, content.scrollHeight - content.clientHeight);
    content.scrollTop = Math.min(viewState.scrollTop, maximum);
  }, [taskId, transcriptItems, viewState]);

  const setWorkExpansion = React.useCallback(
    (id: number, expanded: boolean) => watched.controller.setWorkExpansion(id, expanded),
    [watched],
  );

  const retry = React.useCallback(() => {
    void watched.controller.loadInitial().then(forceUpdate);
  }, [watched, forceUpdate]);
  const refreshDetail = React.useCallback(() => {
    void watched.controller.loadInitial().then(forceUpdate);
  }, [watched, forceUpdate]);

  const changesVisible = showingChanges || reviewingChanges;
  const branches = useTaskBranches(taskId, changesVisible);
  const base = changesBase ?? branches.default;
  const against =
    !changesVisible || changesSource === "run"
      ? undefined
      : changesSource === "uncommitted"
        ? "HEAD"
        : base;
  const git = useGitDiff(taskId, against);
  const showingGit = changesSource !== "run";
  const awaitingBranches = changesSource === "branch" && base === undefined;
  const chooseSource = (source: ChangesSource) => {
    setChangesSource(source);
    storeChangesSource(source);
  };
  const chooseBase = (branch: string) => {
    setChangesBase(branch);
    storeChangesBase(branch);
    chooseSource("branch");
  };
  const chooseGrouping = (grouped: boolean) => {
    setGroupByTurn(grouped);
    storeChangesGrouped(grouped);
  };
  const cwd = task?.cwd;
  const reportedChanges = React.useMemo(
    () => (cwd !== undefined ? changeProjection.update(events, cwd) : RUN_CHANGES_EMPTY),
    [cwd, eventRevision, events, changeProjection],
  );
  const reportedTurns = React.useMemo(
    () =>
      cwd !== undefined && changesVisible && changesSource === "run" && groupByTurn
        ? changeTurnsProjection.update(events, cwd)
        : undefined,
    [cwd, eventRevision, events, changesVisible, changesSource, groupByTurn, changeTurnsProjection],
  );
  const gitChanges = React.useMemo(
    () => (git.diff ? gitChangeSet(git.diff) : RUN_CHANGES_EMPTY),
    [git.diff],
  );

  const changedFiles: ChangedFilesProps | undefined = task ? {
    source: changesSource,
    onSourceChange: chooseSource,
    base: changesSource === "branch" ? base : undefined,
    onBaseChange: chooseBase,
    branches: branches.branches,
    groupByTurn,
    onGroupByTurn: chooseGrouping,
    onReload: git.reload,
    changes: showingGit ? gitChanges : reportedChanges,
    turns: showingGit ? undefined : reportedTurns,
    loading: showingGit ? (awaitingBranches ? branches.loading : git.loading) : state.loading,
    error: showingGit ? git.error : undefined,
    truncated: showingGit && git.diff?.truncated === true,
    live: !activityIsSettled(task.state),
    hasEarlier: state.hasEarlier,
    loadingEarlier: state.loadingEarlier,
    onLoadEarlier: loadEarlier,
  } : undefined;

  // Profiles carry each worker's provider and environment, which the
  // terminal resume command is built from. Only read when the task holds a
  // session worth continuing.
  React.useEffect(() => {
    if (!task?.sessionId) return;
    let disposed = false;
    setProfiles(undefined);
    void broker.summary({ compact: true, limit: 1 }).then((result) => {
      if (disposed || !result.ok) return;
      setProfiles(result.value.profiles);
    });
    return () => {
      disposed = true;
    };
  }, [taskId, task?.profileId, task?.sessionId]);

  const terminalCommand = React.useMemo(
    () => (task ? (terminalResumeCommand(task, profiles?.find((profile) => profile.id === task.profileId)) ?? undefined) : undefined),
    [task, profiles],
  );

  React.useEffect(() => {
    if (!task) {
      onHeader(undefined);
      return;
    }
    const title = task.title ?? task.tldr ?? task.prompt.split("\n").find((line) => line.trim() !== "") ?? "Untitled task";
    const statusLabel = state.loading
      ? "Updating activity"
      : state.error !== undefined
        ? "Activity update unavailable"
        : taskStatusLabel(task);
    onHeader({
      title,
      diffAdded: runChangeSetAdded(reportedChanges),
      showingChanges,
      onToggleChanges: () => setShowingChanges((value) => !value),
      terminalCommand,
      status: <TaskStatusDot state={task.state} label={statusLabel} />,
      secondary: <TaskDetailSecondary task={task} events={events} onChanged={refreshDetail} />,
    });
    return () => onHeader(undefined);
  }, [task, reportedChanges, showingChanges, events, eventRevision, state.loading, state.error, terminalCommand, onHeader]);

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
              <button className="text-button" type="button" onClick={retry}>
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
            {state.loading && state.error === undefined && (
              <p className="detail-message" role="status">Updating activity…</p>
            )}
            <div className="task-detail-content-main" ref={contentRef} onScroll={trackScroll}>
              {state.hasEarlier && (
                <div ref={topSentinelRef} className="transcript-load-earlier" role="status">
                  {state.loadingEarlier && (
                    <>
                      <span className="transcript-load-earlier-spinner" />
                      <span className="visually-hidden">Loading earlier activity</span>
                    </>
                  )}
                </div>
              )}
              <Transcript
                key={taskId}
                items={transcriptItems}
                cwd={task.cwd}
                showThinking={showThinking}
                scrollRoot={contentRef}
                expansionState={viewState.workExpansion}
                onExpansionChange={setWorkExpansion}
              />
            </div>
          </>
        )}
        {task && (
          <TaskControls
            task={task}
            events={events}
            onChanged={refreshDetail}
            thinkingToggle={hasThinking ? { active: showThinking, onToggle: toggleThinking } : undefined}
            focusRequest={focusRequest}
            onFocusRequestConsumed={onFocusRequestConsumed}
          />
        )}
      </div>
      {showingChanges && changedFiles && (
        <ChangedFilesPanel
          key={taskId}
          {...changedFiles}
          onClose={() => setShowingChanges(false)}
          onExpand={() => setReviewingChanges(true)}
          width={changedFilesWidth}
          onResizeStart={(clientX) => setResizeStart({ x: clientX, width: changedFilesWidth })}
          onResetWidth={() => applyChangedFilesWidth(CHANGED_FILES_DEFAULT_WIDTH)}
          onResizeStep={(deltaWidth) => applyChangedFilesWidth(changedFilesWidth + deltaWidth)}
        />
      )}
      {reviewingChanges && changedFiles && (
        <ChangedFilesFullScreen key={taskId} {...changedFiles} onClose={() => setReviewingChanges(false)} />
      )}
    </div>
  );
}
