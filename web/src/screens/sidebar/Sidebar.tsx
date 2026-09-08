"use client";

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { broker, streamStatus } from "@/bridge/client";
import { onBrokerStatus, onEventBatch } from "@/bridge/events";
import type { BridgeError, EventBatch } from "@/bridge/types";
import type { TaskSummary } from "@/bridge/types";
import { MenuPanel, type MenuAction } from "@/components/menu/Menu";
import { EmptyState } from "@/components/atoms/ListState";
import {
  ArchiveIcon,
  CancelIcon,
  CheckIcon,
  ChevronIcon,
  MoreIcon,
  PlayIcon,
  RestoreIcon,
} from "@/ui/icons";
import { SearchField } from "@/components/SearchField";
import {
  archiveTitles,
  ArchiveBranchDialog,
  canComplete,
  canPause,
  canResume,
  completeTitles,
  executeArchive,
  executeCancel,
  executeComplete,
  executeResume,
  resumeTitles,
  stopTitles,
  type TaskToastTitles,
  type ArchiveBranchTask,
} from "@/screens/task/Actions";
import { isExplainedWait, taskStatusLabel, waitLabel } from "@/screens/task/format";
import {
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
  TASK_ARCHIVE_FILTERS,
  TASK_GROUPINGS,
  TASK_SORTS,
  taskArchiveFilterLabel,
  taskGroupingLabel,
  taskSortLabel,
} from "@/state/sidebar-preferences";
import {
  filtersActive,
  SidebarController,
  type ConnectionState,
  type SidebarState,
} from "@/state/sidebar-state";
import {
  displayLabel,
  newVirtualList,
  NO_TASKS_MESSAGE,
  projectionEmptyMessage,
  projectionFromState,
  projectionRows,
  sidebarRowKey,
  taskProjectLabel,
  VIRTUAL_LIST_DEFAULT_ITEM_HEIGHT,
  VIRTUAL_LIST_DEFAULT_VIEWPORT_HEIGHT,
  virtualListOffsetFor,
  virtualListTotalHeight,
  virtualListVisibleRange,
  withItemHeight,
  withScrollOffset,
  withViewportHeight,
  type SidebarRow,
} from "@/state/sidebar-projection";
import { BackArrowIcon, FilterIcon, ForwardArrowIcon, RefreshIcon, SettingsIcon, SidebarIcon, UsageIcon } from "@/ui/icons";
import { formatCost, taskDuration } from "@/lib/format";
import { absoluteTime, relativeTime } from "@/ui/time";
import { handlesClick } from "@/router";
import { toast } from "@/state/toast";

const FILTERS_LABEL = "Filter and sort tasks";

/** Gap kept between the row menu and the window edge it would otherwise cross. */
const ROW_MENU_MARGIN = 8;

function useStore<T>(store: { snapshot: T; subscribe: (l: () => void) => () => void }): T {
  return useSyncExternalStore(store.subscribe.bind(store), () => store.snapshot, () => store.snapshot);
}

function connectionLabel(connection: ConnectionState): string | undefined {
  switch (connection) {
    case "connecting":
      return "Connecting to Oga…";
    case "connected":
      return undefined;
    case "reconnecting":
      return "Reconnecting…";
    case "offline":
      return "Oga is offline";
  }
}

const SETTLED_STATES = new Set(["completed", "failed", "cancelled"]);

function taskSubtitle(task: TaskSummary, state: SidebarState): string {
  const profile = state.profiles.find((p) => p.id === task.profileId);
  const worker = profile?.label ?? "Unknown worker";
  const parts = [worker, taskProjectLabel(task)];
  // A task nobody stopped is waiting for something; the row says what, because
  // otherwise it reads as stalled.
  if (isExplainedWait(task.hold)) parts.push(waitLabel(task.hold));
  const duration = taskDuration(task.durationMs, task.runningSince, !SETTLED_STATES.has(task.state));
  if (duration) parts.push(duration);
  const cost = formatCost(task.costUsd, task.costUsdEstimated);
  if (cost) parts.push(cost);
  return parts.join(" · ");
}

function rowMenuSections(
  task: TaskSummary,
  run: (
    action: () => Promise<{ ok: true; value: unknown } | { ok: false; error: BridgeError }>,
    titles: TaskToastTitles,
  ) => void,
  offline: boolean,
  onDeleteBranch: (task: TaskSummary) => void,
): MenuAction[][] {
  const archived = task.archivedAt !== undefined;
  const canDeleteBranch = !archived && task.originCwd !== undefined && task.branch !== undefined;
  const lifecycle: MenuAction[] = [];
  if (canPause(task)) {
    lifecycle.push({
      key: "stop",
      label: "Stop",
      icon: <CancelIcon />,
      disabled: offline,
      onSelect: () => run(() => executeCancel(task.id), stopTitles(task)),
    });
  }
  if (canResume(task)) {
    lifecycle.push({
      key: "resume",
      label: "Resume",
      icon: <PlayIcon />,
      disabled: offline,
      onSelect: () => run(() => executeResume(task.id), resumeTitles(task)),
    });
  }

  const admin: MenuAction[] = [
    {
      key: "archive",
      label: archived ? "Restore" : "Archive",
      icon: archived ? <RestoreIcon /> : <ArchiveIcon />,
      disabled: offline,
      onSelect: () => run(() => executeArchive(task.id, !archived), archiveTitles(task, archived)),
    },
  ];
  if (canDeleteBranch) {
    admin.push({
      key: "archive-delete-branch",
      label: "Archive and delete branch",
      icon: <ArchiveIcon />,
      destructive: true,
      disabled: offline,
      onSelect: () => onDeleteBranch(task),
    });
  }
  if (canComplete(task)) {
    admin.push({
      key: "complete",
      label: "Mark as completed",
      icon: <CheckIcon />,
      disabled: offline,
      onSelect: () => run(() => executeComplete(task.id), completeTitles(task)),
    });
  }

  return [lifecycle, admin];
}

export interface SidebarProps {
  sidebarController?: SidebarController;
  onSelectTask?: (id: string) => void;
  onOpenSettings?: (tab: "workers") => void;
  onOpenUsage?: () => void;
  initialTask?: string;
  navigation?: HistoryNavigation;
}

export interface HistoryNavigation {
  canGoBack: boolean;
  canGoForward: boolean;
  onBack: () => void;
  onForward: () => void;
}

export default function Sidebar({ sidebarController, onSelectTask, onOpenSettings, onOpenUsage, initialTask, navigation }: SidebarProps) {
  const sidebarRef = useMemo(() => sidebarController ?? new SidebarController(), [sidebarController]);

  const sidebar = useStore(sidebarRef as unknown as { snapshot: SidebarState; subscribe: (l: () => void) => () => void });

  const [filtersOpen, setFiltersOpen] = useState(false);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(VIRTUAL_LIST_DEFAULT_VIEWPORT_HEIGHT);
  const [itemHeight, setItemHeight] = useState(VIRTUAL_LIST_DEFAULT_ITEM_HEIGHT);
  const [resizeStart, setResizeStart] = useState<{ x: number; width: number } | null>(null);
  const [rowMenu, setRowMenu] = useState<{ task: TaskSummary } | null>(null);
  const [archiveBranchTask, setArchiveBranchTask] = useState<ArchiveBranchTask | null>(null);
  const [rowMenuPlacement, setRowMenuPlacement] = useState<{ top: number; left: number } | null>(null);
  const connectionWasLive = useRef(false);
  const connectionState = useRef<ConnectionState>(sidebarRef.snapshot.connection);

  const listShellRef = useRef<HTMLDivElement>(null);
  const sidebarElementRef = useRef<HTMLElement>(null);
  const listWindowRef = useRef<HTMLDivElement>(null);
  const filterButtonRef = useRef<HTMLButtonElement>(null);
  const filterPopoverRef = useRef<HTMLDivElement>(null);
  const rowMenuRef = useRef<HTMLDivElement>(null);
  const rowMenuAnchorRef = useRef<HTMLElement>(null);
  const searchFieldRef = useRef<HTMLInputElement>(null);
  const taskRowRefs = useRef(new Map<string, HTMLAnchorElement>());
  const [focusedTaskId, setFocusedTaskId] = useState<string | undefined>(initialTask);
  const [, setClock] = useState(0);

  useEffect(() => {
    if (!sidebar.tasks.some((task) => task.state === "running")) return;
    const timer = setInterval(() => setClock((clock) => clock + 1), 1_000);
    return () => clearInterval(timer);
  }, [sidebar.tasks]);

  const projection = useMemo(
    () => projectionFromState(sidebar),
    [sidebar.tasks, sidebar.search, sidebar.projectFilter, sidebar.grouping, sidebar.sort],
  );
  const effectiveRows = useMemo(
    () => projectionRows(projection, sidebar.collapsed, sidebar.grouping),
    [projection, sidebar.collapsed, sidebar.grouping],
  );

  const emptyMessage = useMemo(
    () => projectionEmptyMessage(projection, sidebar),
    [projection, sidebar.search, sidebar.archiveFilter, sidebar.projectFilter],
  );

  // Nothing started yet could mean "no workers configured", which the plain
  // message doesn't explain, so this checks once and points at the fix.
  const [hasNoWorkers, setHasNoWorkers] = useState(false);
  useEffect(() => {
    if (emptyMessage !== NO_TASKS_MESSAGE) return;
    let cancelled = false;
    void broker.modelSettings().then((result) => {
      if (!cancelled && result.ok) setHasNoWorkers(result.value.workers.length === 0);
    });
    return () => {
      cancelled = true;
    };
  }, [emptyMessage]);

  const virtual = useMemo(
    () =>
      withScrollOffset(
        withItemHeight(withViewportHeight(newVirtualList(effectiveRows.length), viewportHeight), itemHeight),
        scrollTop,
      ),
    [effectiveRows.length, viewportHeight, itemHeight, scrollTop],
  );
  const [start, end] = virtualListVisibleRange(virtual);
  const visibleRows = effectiveRows.slice(start, end);
  const totalHeight = virtualListTotalHeight(virtual);
  const offset = virtualListOffsetFor(virtual, start);

  const taskIds = useMemo(
    () => effectiveRows.flatMap((row) => (row.type === "task" ? [row.task.id] : [])),
    [effectiveRows],
  );

  useEffect(() => {
    setFocusedTaskId((current) => (current && taskIds.includes(current) ? current : taskIds[0]));
  }, [taskIds]);

  const focusTask = useCallback((id: string) => {
    const index = effectiveRows.findIndex((row) => row.type === "task" && row.task.id === id);
    if (index < 0) return;
    const top = index * itemHeight;
    const bottom = top + itemHeight;
    const nextScrollTop = top < scrollTop
      ? top
      : bottom > scrollTop + viewportHeight
        ? bottom - viewportHeight
        : scrollTop;
    if (nextScrollTop !== scrollTop) {
      listShellRef.current?.scrollTo({ top: nextScrollTop });
      setScrollTop(nextScrollTop);
    }
    setFocusedTaskId(id);
    requestAnimationFrame(() => taskRowRefs.current.get(id)?.focus());
  }, [effectiveRows, itemHeight, scrollTop, viewportHeight]);

  const handleListKeyDown = useCallback((event: React.KeyboardEvent<HTMLDivElement>) => {
    if (taskIds.length === 0) return;
    const currentIndex = focusedTaskId ? taskIds.indexOf(focusedTaskId) : -1;
    let nextIndex: number | undefined;
    if (event.key === "ArrowDown") nextIndex = Math.min(currentIndex + 1, taskIds.length - 1);
    if (event.key === "ArrowUp") nextIndex = Math.max(currentIndex - 1, 0);
    if (event.key === "Home") nextIndex = 0;
    if (event.key === "End") nextIndex = taskIds.length - 1;
    if (nextIndex === undefined) return;
    event.preventDefault();
    focusTask(taskIds[nextIndex]);
  }, [focusTask, focusedTaskId, taskIds]);

  // The window decides how many rows fit and the rows decide how tall they
  // are, so both are read back from the layout rather than assumed.
  useLayoutEffect(() => {
    const shell = listShellRef.current;
    if (shell) setViewportHeight((height) => (shell.clientHeight > 0 ? shell.clientHeight : height));
    const row = taskRowRefs.current.values().next().value;
    if (row) setItemHeight((height) => (row.offsetHeight > 0 ? row.offsetHeight : height));
  });

  // A resized window changes how many rows fit without changing any state.
  useEffect(() => {
    const measure = () => {
      const shell = listShellRef.current;
      if (shell && shell.clientHeight > 0) setViewportHeight(shell.clientHeight);
    };
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, []);

  useEffect(() => {
    if (initialTask) {
      sidebarRef.update((s) => ({ ...s, selectedTask: initialTask }));
    }
  }, [initialTask, sidebarRef]);

  useEffect(() => {
    void sidebarRef.refresh();
  }, [sidebarRef]);

  useEffect(() => {
    void streamStatus().then((result) => {
      if (result.ok) {
        connectionState.current = result.value.connected ? "connected" : "reconnecting";
        connectionWasLive.current ||= result.value.connected;
        sidebarRef.applyConnection(result.value);
      }
    });
    const unsub = onBrokerStatus((status) => {
      const wasConnected = connectionState.current === "connected";
      if (!status.connected && wasConnected && connectionWasLive.current) {
        toast.error("Connection lost", { description: "Live task updates are reconnecting." });
      }
      if (status.connected && connectionWasLive.current && !wasConnected) {
        toast.success("Connection restored", { description: "Live task updates are back." });
      }
      if (status.connected) connectionWasLive.current = true;
      connectionState.current = status.connected ? "connected" : "reconnecting";
      sidebarRef.applyConnection(status);
    });
    return unsub;
  }, [sidebarRef]);

  useEffect(() => {
    let refreshing = false;
    let refreshAgain = false;
    let refreshTimer: ReturnType<typeof setTimeout> | undefined;

    const scheduleRefresh = () => {
      if (refreshing || refreshTimer !== undefined) return;
      refreshTimer = setTimeout(() => {
        refreshTimer = undefined;
        if (!refreshAgain) return;
        refreshAgain = false;
        refreshing = true;
        void sidebarRef.refresh().finally(() => {
          refreshing = false;
          if (refreshAgain) scheduleRefresh();
        });
      }, 100);
    };

    const requestRefresh = () => {
      refreshAgain = true;
      scheduleRefresh();
    };

    const unsub = onEventBatch((batch: EventBatch) => {
      const action = sidebarRef.applyBatch(batch);
      if (action === "refresh") {
        requestRefresh();
      }
    });
    return () => {
      unsub();
      if (refreshTimer !== undefined) clearTimeout(refreshTimer);
    };
  }, [sidebarRef]);

  useEffect(() => {
    if (filtersOpen) filterPopoverRef.current?.focus();
  }, [filtersOpen]);

  useEffect(() => {
    if (!filtersOpen) return;
    const handle = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (!target) return;
      if (filterPopoverRef.current?.contains(target)) return;
      if (filterButtonRef.current?.contains(target)) return;
      setFiltersOpen(false);
    };
    document.addEventListener("pointerdown", handle);
    return () => document.removeEventListener("pointerdown", handle);
  }, [filtersOpen]);

  // Read the live trigger position so virtualization and scrolling cannot
  // leave the menu behind its row.
  useLayoutEffect(() => {
    if (!rowMenu) {
      setRowMenuPlacement(null);
      return;
    }
    const anchor = rowMenuAnchorRef.current;
    const panel = rowMenuRef.current;
    const sidebarElement = sidebarElementRef.current;
    if (!anchor || !panel || !sidebarElement) return;
    if (!anchor.isConnected) {
      setRowMenu(null);
      return;
    }
    const anchorRect = anchor.getBoundingClientRect();
    const { width, height } = panel.getBoundingClientRect();
    const rightSideLeft = anchorRect.right + ROW_MENU_MARGIN;
    const leftSideLeft = anchorRect.left - width - ROW_MENU_MARGIN;
    const left = rightSideLeft + width + ROW_MENU_MARGIN <= window.innerWidth
      ? rightSideLeft
      : Math.max(ROW_MENU_MARGIN, Math.min(leftSideLeft, window.innerWidth - width - ROW_MENU_MARGIN));
    const fitsBelow = anchorRect.bottom + height + ROW_MENU_MARGIN <= window.innerHeight;
    const top = fitsBelow
      ? anchorRect.bottom + ROW_MENU_MARGIN
      : Math.max(ROW_MENU_MARGIN, anchorRect.top - height - ROW_MENU_MARGIN);
    const sidebarRect = sidebarElement.getBoundingClientRect();
    const zoom = Number.parseFloat(getComputedStyle(document.documentElement).zoom);
    const scale = Number.isFinite(zoom) && zoom > 0 ? zoom : 1;
    setRowMenuPlacement({ top: (top - sidebarRect.top) / scale, left: (left - sidebarRect.left) / scale });
  }, [rowMenu, scrollTop]);

  useEffect(() => {
    if (!rowMenu) return;
    const closeOnPointer = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (target && rowMenuRef.current?.contains(target)) return;
      setRowMenu(null);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setRowMenu(null);
    };
    document.addEventListener("pointerdown", closeOnPointer);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointer);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [rowMenu]);

  useEffect(() => {
    if (!resizeStart) return;
    const onMove = (event: PointerEvent) => {
      const width = Math.round(resizeStart.width + event.clientX - resizeStart.x);
      const clamped = Math.min(Math.max(width, SIDEBAR_MIN_WIDTH), SIDEBAR_MAX_WIDTH);
      sidebarRef.setSidebarWidth(clamped);
    };
    const onUp = () => {
      setResizeStart(null);
    };
    const onCancel = () => setResizeStart(null);
    document.addEventListener("pointermove", onMove);
    document.addEventListener("pointerup", onUp);
    document.addEventListener("pointercancel", onCancel);
    return () => {
      document.removeEventListener("pointermove", onMove);
      document.removeEventListener("pointerup", onUp);
      document.removeEventListener("pointercancel", onCancel);
    };
  }, [resizeStart, sidebarRef]);

  const handleRefresh = useCallback(() => {
    void sidebarRef.refresh();
  }, [sidebarRef]);

  const handleLoadMore = useCallback(() => {
    void sidebarRef.loadMore();
  }, [sidebarRef]);

  const handleRowContextMenu = useCallback((event: React.MouseEvent, task: TaskSummary) => {
    event.preventDefault();
    rowMenuAnchorRef.current = event.currentTarget as HTMLElement;
    setRowMenu({ task });
  }, []);

  const handleRowMenuButton = useCallback((event: React.MouseEvent<HTMLButtonElement>, task: TaskSummary) => {
    event.preventDefault();
    rowMenuAnchorRef.current = event.currentTarget;
    setRowMenu({ task });
  }, []);

  const handleDeleteBranch = useCallback((task: TaskSummary) => {
    setRowMenu(null);
    if (task.originCwd !== undefined && task.branch !== undefined) {
      setArchiveBranchTask({ ...task, branch: task.branch });
    }
  }, []);

  const runRowAction = useCallback(
    (
      action: () => Promise<{ ok: true; value: unknown } | { ok: false; error: BridgeError }>,
      titles: TaskToastTitles,
    ) => {
      setRowMenu(null);
      const run = () => {
        const lifecycle = toast.pending(titles.pending, { retry: run });
        void action().then((result) => {
          if (result.ok) {
            lifecycle.dismiss();
            void sidebarRef.refresh();
          } else {
            lifecycle.error(`Couldn't ${titles.failure}`, { description: "Try again.", detail: result.error.message });
          }
        }).catch((error: unknown) => {
          lifecycle.error("Couldn't reach Oga", { description: "Check the connection and try again.", detail: error instanceof Error ? error.message : String(error) });
        });
      };
      run();
    },
    [sidebarRef],
  );

  const handleSelectTask = useCallback(
    (id: string) => {
      sidebarRef.update((s) => {
        const has = s.tasks.some((t) => t.id === id);
        if (!has) return s;
        return { ...s, selectedTask: id };
      });
      if (onSelectTask) onSelectTask(id);
      else window.history.pushState(null, "", `/tasks/${id}`);
    },
    [sidebarRef, onSelectTask],
  );

  const handleToggleGroup = useCallback(
    (id: string) => {
      sidebarRef.toggleGroup(id);
    },
    [sidebarRef],
  );

  const handleSearch = useCallback(
    (value: string) => {
      sidebarRef.setSearch(value);
    },
    [sidebarRef],
  );

  const handleGrouping = useCallback(
    (value: string) => {
      if ((TASK_GROUPINGS as readonly string[]).includes(value)) {
        sidebarRef.setGrouping(value as typeof sidebar.grouping);
      }
    },
    [sidebarRef, sidebar.grouping],
  );

  const handleSort = useCallback(
    (value: string) => {
      if ((TASK_SORTS as readonly string[]).includes(value)) {
        sidebarRef.setSort(value as typeof sidebar.sort);
      }
    },
    [sidebarRef, sidebar.sort],
  );

  const handleArchive = useCallback(
    (value: string) => {
      if ((TASK_ARCHIVE_FILTERS as readonly string[]).includes(value)) {
        void sidebarRef.setArchiveFilter(value as typeof sidebar.archiveFilter);
      }
    },
    [sidebarRef],
  );

  const handleProject = useCallback(
    (value: string) => {
      sidebarRef.setProjectFilter(value === "" ? undefined : value);
    },
    [sidebarRef],
  );

  const handleResetFilters = useCallback(() => {
    const changed = sidebarRef.resetFilters();
    setFiltersOpen(false);
    filterButtonRef.current?.focus();
    if (changed) void sidebarRef.refresh();
  }, [sidebarRef]);

  const filtersAreActive = filtersActive(sidebar);
  const connectionStatus = connectionLabel(sidebar.connection);

  const sidebarStyle: React.CSSProperties = { width: `${sidebar.sidebarWidth}px` };

  return (
    <aside
      ref={sidebarElementRef}
      className={`task-sidebar${sidebar.sidebarCollapsed ? " task-sidebar-collapsed" : ""}`}
      style={sidebarStyle}
      aria-label="Tasks"
    >
      <div className="sidebar-topbar" data-tauri-drag-region>
        <button
          className="icon-button"
          type="button"
          aria-expanded={!sidebar.sidebarCollapsed}
          aria-controls="task-list"
          aria-label={sidebar.sidebarCollapsed ? "Show task list" : "Hide task list"}
          title={sidebar.sidebarCollapsed ? "Show task list" : "Hide task list"}
          onClick={() => sidebarRef.toggleSidebar()}
        >
          <SidebarIcon />
        </button>
        {navigation && (
          <>
            <button
              className="icon-button"
              type="button"
              aria-label="Back"
              title="Back ⌘["
              disabled={!navigation.canGoBack}
              onClick={navigation.onBack}
            >
              <BackArrowIcon />
            </button>
            <button
              className="icon-button"
              type="button"
              aria-label="Forward"
              title="Forward ⌘]"
              disabled={!navigation.canGoForward}
              onClick={navigation.onForward}
            >
              <ForwardArrowIcon />
            </button>
          </>
        )}
        <span className="sidebar-topbar-drag" data-tauri-drag-region />
        <a
          className="icon-button"
          href="/settings"
          aria-label="Settings"
          title="Settings ⌘,"
          onClick={(event) => {
            if (!handlesClick(event)) return;
            event.preventDefault();
            if (onOpenSettings) onOpenSettings("workers");
            else {
              window.history.pushState(null, "", "/settings");
              window.dispatchEvent(new PopStateEvent("popstate"));
            }
          }}
        >
          <SettingsIcon />
        </a>
        <a className="icon-button" href="/usage" aria-label="Usage" title="Usage ⇧⌘U" onClick={(event) => {
          if (!handlesClick(event)) return;
          event.preventDefault();
          onOpenUsage?.();
        }}><UsageIcon /></a>
        <button
          className="icon-button"
          type="button"
          aria-label="Refresh tasks"
          title="Refresh tasks ⌘R"
          onClick={handleRefresh}
        >
          <RefreshIcon />
        </button>
      </div>

      <div
        className="sidebar-resize-handle"
        onPointerDown={(event) => {
          event.preventDefault();
          setResizeStart({ x: event.clientX, width: sidebar.sidebarWidth });
        }}
      />

      <div className="sidebar-search-row">
        <SearchField
          className="sidebar-search"
          inputRef={searchFieldRef}
          value={sidebar.search}
          onChange={handleSearch}
          placeholder="Search tasks"
          aria-label="Search tasks by title"
          title="Search tasks ⌘K"
          onKeyDown={(event) => {
            if (event.key === "ArrowDown" && taskIds.length > 0) {
              event.preventDefault();
              focusTask(taskIds[0]);
            }
         }}
        />

        <div className="sidebar-filter">
          <button
            ref={filterButtonRef}
            className={`icon-button sidebar-filter-button${filtersAreActive ? " sidebar-filter-button-active" : ""}`}
            type="button"
            aria-haspopup="dialog"
            aria-expanded={filtersOpen}
            aria-controls="task-filters"
            aria-label={FILTERS_LABEL}
            title={FILTERS_LABEL}
            onClick={() => setFiltersOpen((o) => !o)}
          >
            <FilterIcon />
          </button>
          {filtersOpen && (
            <div
              ref={filterPopoverRef}
              id="task-filters"
              className="filter-popover"
              role="dialog"
              aria-label={FILTERS_LABEL}
              tabIndex={-1}
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  setFiltersOpen(false);
                  filterButtonRef.current?.focus();
                }
              }}
            >
              <div className="filter-popover-header">
                <h2>Filter and sort</h2>
                {filtersAreActive && (
                  <button className="text-button" type="button" onClick={handleResetFilters}>
                    Reset
                  </button>
                )}
              </div>

              <fieldset className="filter-section">
                <legend>Group by</legend>
                {TASK_GROUPINGS.map((value) => (
                  <label key={value} className="filter-option">
                    <input
                      type="radio"
                      name="task-grouping"
                      value={value}
                      checked={sidebar.grouping === value}
                      onChange={() => handleGrouping(value)}
                    />
                    <span>{taskGroupingLabel(value)}</span>
                  </label>
                ))}
              </fieldset>

              <fieldset className="filter-section">
                <legend>Sort by</legend>
                {TASK_SORTS.map((value) => (
                  <label key={value} className="filter-option">
                    <input
                      type="radio"
                      name="task-sort"
                      value={value}
                      checked={sidebar.sort === value}
                      onChange={() => handleSort(value)}
                    />
                    <span>{taskSortLabel(value)}</span>
                  </label>
                ))}
              </fieldset>

              <fieldset className="filter-section">
                <legend>Show</legend>
                {TASK_ARCHIVE_FILTERS.map((value) => (
                  <label key={value} className="filter-option">
                    <input
                      type="radio"
                      name="task-archive"
                      value={value}
                      checked={sidebar.archiveFilter === value}
                      onChange={() => handleArchive(value)}
                    />
                    <span>{taskArchiveFilterLabel(value)}</span>
                  </label>
                ))}
              </fieldset>

              <fieldset className="filter-section">
                <legend>Project</legend>
                <label className="filter-option">
                  <input
                    type="radio"
                    name="task-project"
                    value=""
                    checked={sidebar.projectFilter === undefined}
                    onChange={() => handleProject("")}
                  />
                  <span>All projects</span>
                </label>
                {projection.projects.map((project) => (
                  <label key={project.id} className="filter-option">
                    <input
                      type="radio"
                      name="task-project"
                      value={project.id}
                      checked={sidebar.projectFilter === project.id}
                      onChange={() => handleProject(project.id)}
                    />
                    <span>{project.name}</span>
                    <span className="filter-option-count">{project.count}</span>
                  </label>
                ))}
              </fieldset>
            </div>
          )}
        </div>
      </div>

      <div
        id="task-list"
        ref={listShellRef}
        className="sidebar-list-shell"
        role="listbox"
        aria-label="Task list"
        onKeyDown={handleListKeyDown}
        onScroll={(event) => setScrollTop((event.target as HTMLDivElement).scrollTop)}
      >
        <div className="sidebar-list-spacer" style={{ height: `${totalHeight}px` }}>
          <div ref={listWindowRef} className="sidebar-list-window" style={{ transform: `translateY(${offset}px)` }}>
            {visibleRows.map((row) => (
              <SidebarRowView
                key={sidebarRowKey(row)}
                row={row}
                sidebar={sidebar}
                onSelect={handleSelectTask}
                onToggle={handleToggleGroup}
                onContextMenu={handleRowContextMenu}
                onOpenMenu={handleRowMenuButton}
                openMenuTaskId={rowMenu?.task.id}
                focusedTaskId={focusedTaskId}
                taskRowRefs={taskRowRefs}
                onFocusTask={setFocusedTaskId}
              />
            ))}
          </div>
        </div>
        <SidebarFooter
          state={sidebar}
          emptyMessage={emptyMessage}
          hasNoWorkers={hasNoWorkers}
          onOpenSettings={onOpenSettings}
          onRefresh={handleRefresh}
          onLoadMore={handleLoadMore}
        />
      </div>

      {rowMenu && (
        <div
          ref={rowMenuRef}
          className="sidebar-row-context-menu"
          style={{
            top: rowMenuPlacement?.top,
            left: rowMenuPlacement?.left,
            visibility: rowMenuPlacement ? undefined : "hidden",
          }}
        >
          <MenuPanel
            sections={rowMenuSections(rowMenu.task, runRowAction, sidebar.connection !== "connected", handleDeleteBranch)}
            onClose={() => setRowMenu(null)}
          />
        </div>
      )}

      {archiveBranchTask && (
        <ArchiveBranchDialog
          task={archiveBranchTask}
          open
          onClose={() => setArchiveBranchTask(null)}
          onChanged={() => {
            setArchiveBranchTask(null);
            void sidebarRef.refresh();
          }}
        />
      )}

      {connectionStatus !== undefined && (
        <footer className="sidebar-connection" role="status" aria-live="polite">
          {connectionStatus}
        </footer>
      )}
    </aside>
  );
}

function SidebarRowView({
  row,
  sidebar,
  onSelect,
  onToggle,
  onContextMenu,
  onOpenMenu,
  openMenuTaskId,
  focusedTaskId,
  taskRowRefs,
  onFocusTask,
}: {
  row: SidebarRow;
  sidebar: SidebarState;
  onSelect: (id: string) => void;
  onToggle: (id: string) => void;
  onContextMenu: (event: React.MouseEvent, task: TaskSummary) => void;
  onOpenMenu: (event: React.MouseEvent<HTMLButtonElement>, task: TaskSummary) => void;
  openMenuTaskId?: string;
  focusedTaskId?: string;
  taskRowRefs: React.RefObject<Map<string, HTMLAnchorElement>>;
  onFocusTask: (id: string) => void;
}) {
  if (row.type === "groupHeader") {
    const className = row.indented ? "sidebar-group sidebar-group-indented" : "sidebar-group";
    return (
      <button
        className={className}
        type="button"
        tabIndex={-1}
        aria-expanded={!row.collapsed}
        onClick={() => onToggle(row.id)}
      >
        <span className={`group-disclosure${row.collapsed ? "" : " group-disclosure-open"}`} aria-hidden="true">
          <ChevronIcon />
        </span>
        <span className="group-title">{row.title}</span>
        <span className="group-count">{row.count}</span>
      </button>
    );
  }

  const task = row.task;
  const subtitle = taskSubtitle(task, sidebar);
  const label = displayLabel(task);
  const isSelected = sidebar.selectedTask === task.id;
  const href = `/tasks/${task.id}`;
  const className = [
    "sidebar-task",
    row.indented && "sidebar-task-indented",
    task.archivedAt !== undefined && "sidebar-task-archived",
  ].filter(Boolean).join(" ");

  return (
    <div className="sidebar-task-row">
      <a
        ref={(element) => {
          if (element) taskRowRefs.current.set(task.id, element);
          else taskRowRefs.current.delete(task.id);
        }}
        className={className}
        href={href}
        role="option"
        tabIndex={focusedTaskId === task.id ? 0 : -1}
        aria-selected={isSelected}
        aria-current={isSelected ? "page" : undefined}
        onFocus={() => onFocusTask(task.id)}
        onClick={(event) => {
          if (!handlesClick(event)) return;
          event.preventDefault();
          onSelect(task.id);
        }}
        onContextMenu={(event) => onContextMenu(event, task)}
      >
        <span className={`task-dot task-dot-${task.state}`} aria-hidden="true" />
        <span className="visually-hidden">
          {taskStatusLabel(task)}
        </span>
        <span className="task-copy">
          <span className="task-line">
            <span className="task-title">{label}</span>
            <span className="task-time" title={absoluteTime(task.updatedAt)}>{relativeTime(task.updatedAt)}</span>
          </span>
          <span className="task-subtitle" title={task.model}>{subtitle}</span>
        </span>
      </a>
      <button
        className="icon-button sidebar-row-menu-button"
        type="button"
        aria-label={`Actions for ${label}`}
        title={openMenuTaskId === task.id ? undefined : "Task actions"}
        onClick={(event) => onOpenMenu(event, task)}
      >
        <MoreIcon />
      </button>
    </div>
  );
}

function SidebarFooter({
  state,
  emptyMessage,
  hasNoWorkers,
  onOpenSettings,
  onRefresh,
  onLoadMore,
}: {
  state: SidebarState;
  emptyMessage: string | undefined;
  hasNoWorkers: boolean;
  onOpenSettings?: (tab: "workers") => void;
  onRefresh: () => void;
  onLoadMore: () => void;
}) {
  if (state.loadState === "loading" && state.tasks.length === 0) {
    return (
      <div className="sidebar-skeleton" aria-label="Loading tasks…" aria-busy="true">
        <span className="sidebar-skeleton-row" />
        <span className="sidebar-skeleton-row" />
        <span className="sidebar-skeleton-row" />
        <span className="sidebar-skeleton-row" />
        <span className="sidebar-skeleton-row" />
      </div>
    );
  }
  if (state.loadState === "error" && state.tasks.length === 0) {
    return (
      <div className="sidebar-message sidebar-message-error">
        <p>Couldn&apos;t load tasks.</p>
        <button className="text-button" type="button" onClick={onRefresh}>
          Try again
        </button>
      </div>
    );
  }
  if (emptyMessage === NO_TASKS_MESSAGE && hasNoWorkers && onOpenSettings) {
    return (
      <EmptyState
        title="No workers yet"
        hint="Add a worker in Settings to start a task."
        className="sidebar-message"
        action={
          <a
            href="/settings"
            onClick={(event) => {
              if (!handlesClick(event)) return;
              event.preventDefault();
              onOpenSettings("workers");
            }}
          >
            Open worker settings
          </a>
        }
      />
    );
  }
  if (emptyMessage) {
    return (
      <EmptyState
        title="No tasks yet"
        hint={emptyMessage === NO_TASKS_MESSAGE ? "Start a task from the command line." : "Try changing your search or filters."}
        className="sidebar-message"
      />
    );
  }
  if (state.error !== undefined && !state.loadMoreFailed) {
    return (
      <div className="sidebar-refresh-error" role="status">
        <span>Couldn&apos;t refresh tasks.</span>
        <button className="text-button" type="button" onClick={onRefresh}>
          Try again
        </button>
      </div>
    );
  }
  if (state.tasksHasMore) {
    const label = state.isLoadingMore ? "Loading…" : state.loadMoreFailed ? "Couldn't load more. Try again" : "Load more";
    return (
      <button className="load-more" type="button" disabled={state.isLoadingMore} onClick={onLoadMore}>
        {label}
      </button>
    );
  }
  return null;
}
