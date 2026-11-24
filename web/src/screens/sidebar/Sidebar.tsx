// The app's one navigation. It also keeps the task list in step with the
// broker for every screen that reads it.

import { memo, useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { streamStatus } from "@/bridge/client";
import { onBrokerStatus, onEventBatch } from "@/bridge/events";
import type { EventBatch } from "@/bridge/types";
import { SearchField } from "@/components/SearchField";
import { isWindowVisible, useWindowShown } from "@/lib/window-visibility";
import { SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH } from "@/state/sidebar-preferences";
import { projectList, recentProjectIds, type TaskProject } from "@/state/task-labels";
import { searchTerm, SidebarController, type ConnectionState, type SidebarState } from "@/state/sidebar-state";
import { taskOutcomeViews } from "@/state/task-outcome-views";
import { TASK_VIEWS, viewTotal, type TaskView } from "@/state/task-views";
import { toast } from "@/state/toast";
import {
  ArchiveIcon,
  BackArrowIcon,
  BellIcon,
  CheckCircleIcon,
  FolderIcon,
  ForwardArrowIcon,
  ListIcon,
  PlayIcon,
  SettingsIcon,
  SidebarIcon,
} from "@/ui/icons";

/** Projects listed before the rest fold behind "Show all". */
const PROJECT_LIMIT = 8;

const VIEW_ICONS = {
  recent: <ListIcon />,
  "needs-you": <BellIcon />,
  "in-progress": <PlayIcon />,
  finished: <CheckCircleIcon />,
  archived: <ArchiveIcon />,
} satisfies Record<TaskView, ReactNode>;

function useStore(store: SidebarController): SidebarState {
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

function workersLabel(state: SidebarState): string {
  if (state.loadState !== "ready") return "Workspace";
  const ready = state.profiles.filter((profile) => profile.enabled).length;
  if (ready === 0) return "Add a worker to start";
  return `${ready} ${ready === 1 ? "worker" : "workers"}`;
}

export interface HistoryNavigation {
  canGoBack: boolean;
  canGoForward: boolean;
  onBack: () => void;
  onForward: () => void;
}

export interface SidebarProps {
  sidebarController?: SidebarController;
  onOpenSettings?: (tab: "workers") => void;
  onShowTasks?: () => void;
  /** True while the task list is the screen shown, so the current view reads as the current page. */
  tasksShown?: boolean;
  navigation?: HistoryNavigation;
}

function Sidebar({ sidebarController, onOpenSettings, onShowTasks, tasksShown = false, navigation }: SidebarProps) {
  const controller = useMemo(() => sidebarController ?? new SidebarController(), [sidebarController]);
  const sidebar = useStore(controller);
  const taskOutcomeVersion = useSyncExternalStore(taskOutcomeViews.subscribe, () => taskOutcomeViews.snapshot, () => 0);
  const [resizeStart, setResizeStart] = useState<{ x: number; width: number } | null>(null);
  const connectionWasLive = useRef(false);
  const connectionState = useRef<ConnectionState>(controller.snapshot.connection);
  const lastReadSearch = useRef(controller.snapshot.search.trim());
  const hiddenCursor = useRef(0);
  // Read only from the unfiltered list, so picking a project never reorders the projects.
  const [projectOrder, setProjectOrder] = useState<string[]>([]);
  const listUnfiltered = sidebar.projectFilter === undefined && sidebar.archiveFilter === "active" && searchTerm(sidebar) === "";

  useEffect(() => {
    if (listUnfiltered && sidebar.loadState === "ready") setProjectOrder(recentProjectIds(sidebar.tasks));
  }, [listUnfiltered, sidebar.loadState, sidebar.tasks]);

  useWindowShown(() => {
    const cursor = hiddenCursor.current;
    hiddenCursor.current = 0;
    if (cursor > controller.snapshot.eventCursor) {
      controller.update((state) => ({ ...state, eventCursor: cursor }));
    }
    void controller.refresh();
  });

  useEffect(() => {
    taskOutcomeViews.observeTasks(sidebar.tasks);
  }, [sidebar.tasks, taskOutcomeVersion]);

  useEffect(() => {
    void controller.refresh();
  }, [controller]);

  // Totals come from the broker filtered by search, so the list rereads once
  // typing settles instead of on every keystroke.
  useEffect(() => {
    const term = sidebar.search.trim();
    if (term === lastReadSearch.current) return;
    const timer = setTimeout(() => {
      if (lastReadSearch.current === term) return;
      lastReadSearch.current = term;
      void controller.refresh();
    }, 250);
    return () => clearTimeout(timer);
  }, [sidebar.search, controller]);

  useEffect(() => {
    let active = true;
    let receivedStatus = false;
    const unsub = onBrokerStatus((status) => {
      receivedStatus = true;
      const wasConnected = connectionState.current === "connected";
      if (!status.connected && wasConnected && connectionWasLive.current) {
        toast.error("Connection lost", { description: "Live task updates are reconnecting." });
      }
      if (status.connected && connectionWasLive.current && !wasConnected) {
        toast.success("Connection restored", { description: "Live task updates are back." });
      }
      if (status.connected) connectionWasLive.current = true;
      connectionState.current = status.connected ? "connected" : "reconnecting";
      const returning = controller.applyConnection(status);
      if (returning) void controller.refresh();
    });
    void streamStatus().then((result) => {
      if (!active || receivedStatus || !result.ok) return;
      connectionState.current = result.value.connected ? "connected" : "reconnecting";
      connectionWasLive.current ||= result.value.connected;
      controller.applyConnection(result.value);
    });
    return () => {
      active = false;
      unsub();
    };
  }, [controller]);

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
        void controller.refresh().finally(() => {
          refreshing = false;
          if (refreshAgain) scheduleRefresh();
        });
      }, 100);
    };

    const unsub = onEventBatch((batch: EventBatch) => {
      if (!isWindowVisible()) {
        hiddenCursor.current = Math.max(hiddenCursor.current, batch.cursor);
        return;
      }
      if (controller.applyBatch(batch) === "refresh") {
        refreshAgain = true;
        scheduleRefresh();
      }
    });
    return () => {
      unsub();
      if (refreshTimer !== undefined) clearTimeout(refreshTimer);
    };
  }, [controller]);

  useEffect(() => {
    if (!resizeStart) return;
    const onMove = (event: PointerEvent) => {
      const width = Math.round(resizeStart.width + event.clientX - resizeStart.x);
      controller.setSidebarWidth(Math.min(Math.max(width, SIDEBAR_MIN_WIDTH), SIDEBAR_MAX_WIDTH));
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
  }, [resizeStart, controller]);

  const showView = useCallback((view: TaskView) => {
    controller.showView(view);
    onShowTasks?.();
  }, [controller, onShowTasks]);

  const showProject = useCallback((project: string) => {
    controller.showProject(project);
    onShowTasks?.();
  }, [controller, onShowTasks]);

  const handleSearch = useCallback((value: string) => {
    controller.setSearch(value);
    if (value.trim() !== "") onShowTasks?.();
  }, [controller, onShowTasks]);

  const connectionStatus = connectionLabel(sidebar.connection);
  const showingProject = sidebar.projectFilter;

  return (
    <aside
      id="app-sidebar"
      className={`task-sidebar${sidebar.sidebarCollapsed ? " task-sidebar-collapsed" : ""}`}
      style={{ width: `${sidebar.sidebarWidth}px` }}
      aria-label="Sidebar"
    >
      <div className="sidebar-topbar" data-tauri-drag-region>
        <button
          className="icon-button"
          type="button"
          aria-expanded={!sidebar.sidebarCollapsed}
          aria-controls="app-sidebar"
          aria-label={sidebar.sidebarCollapsed ? "Show sidebar" : "Hide sidebar"}
          title={sidebar.sidebarCollapsed ? "Show sidebar ⌘B" : "Hide sidebar ⌘B"}
          onClick={() => controller.toggleSidebar()}
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
      </div>

      <div
        className="sidebar-resize-handle"
        onPointerDown={(event) => {
          event.preventDefault();
          setResizeStart({ x: event.clientX, width: sidebar.sidebarWidth });
        }}
      />

      <div className="sidebar-body">
        <button
          className="sidebar-workspace"
          type="button"
          title="Settings ⌘,"
          onClick={() => onOpenSettings?.("workers")}
        >
          <span className="sidebar-avatar" aria-hidden="true">O</span>
          <span className="sidebar-workspace-text">
            <strong>Oga</strong>
            <small>{workersLabel(sidebar)}</small>
          </span>
          <SettingsIcon />
          <span className="visually-hidden">Open settings</span>
        </button>

        <SearchField
          className="sidebar-search"
          value={sidebar.search}
          onChange={handleSearch}
          placeholder="Search tasks"
          aria-label="Search tasks"
          title="Search tasks ⌘K"
          shortcut="⌘K"
          data-task-search
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === "ArrowDown") {
              event.preventDefault();
              onShowTasks?.();
              requestAnimationFrame(() => document.querySelector<HTMLElement>("#task-list [tabindex='0']")?.focus());
            }
          }}
        />

        <nav className="sidebar-nav" aria-label="Task views">
          <h2 className="sidebar-label">Tasks</h2>
          <ul>
            {TASK_VIEWS.map((view) => (
              <ViewItem
                key={view.id}
                view={view.id}
                label={view.label}
                current={tasksShown && showingProject === undefined && sidebar.view === view.id}
                count={viewTotal(view.id, sidebar.overview.states)}
                onShow={showView}
              />
            ))}
          </ul>
          <ProjectItems
            projects={projectList(projectOrder, sidebar.overview.projects)}
            current={tasksShown ? showingProject : undefined}
            onShow={showProject}
          />
        </nav>
      </div>

      {connectionStatus !== undefined && (
        <footer className="sidebar-connection" role="status" aria-live="polite">
          {connectionStatus}
        </footer>
      )}
    </aside>
  );
}

function ViewItem({ view, label, current, count, onShow }: {
  view: TaskView;
  label: string;
  current: boolean;
  count: number | undefined;
  onShow: (view: TaskView) => void;
}) {
  // Only the views that ask for attention carry a number; the rest would be noise.
  const shown = count !== undefined && count > 0 && (view === "needs-you" || view === "in-progress") ? count : undefined;
  return (
    <li>
      <button
        className="sidebar-item"
        type="button"
        aria-current={current ? "page" : undefined}
        onClick={() => onShow(view)}
      >
        {VIEW_ICONS[view]}
        <span className="sidebar-item-label">{label}</span>{" "}
        {shown !== undefined && (
          <span className={view === "needs-you" ? "sidebar-badge" : "sidebar-count"}>
            {shown}
            <span className="visually-hidden">{view === "needs-you" ? " need you" : " in progress"}</span>
          </span>
        )}
      </button>
    </li>
  );
}

function ProjectItems({ projects, current, onShow }: {
  projects: TaskProject[];
  current: string | undefined;
  onShow: (project: string) => void;
}) {
  const [showAll, setShowAll] = useState(false);
  if (projects.length === 0) return null;
  const folded = !showAll && projects.length > PROJECT_LIMIT;
  // The project on screen stays listed even when it sits past the fold.
  const listed = folded
    ? projects.filter((project, index) => index < PROJECT_LIMIT || project.id === current)
    : projects;
  return (
    <>
      <h2 className="sidebar-label">Projects</h2>
      <ul>
        {listed.map((project) => (
          <li key={project.id}>
            <button
              className="sidebar-item"
              type="button"
              title={project.id}
              aria-current={project.id === current ? "page" : undefined}
              onClick={() => onShow(project.id)}
            >
              <FolderIcon />
              <span className="sidebar-item-label">
                {project.name}
                {project.parent && <span className="sidebar-item-hint">{` ${project.parent}`}</span>}
              </span>{" "}
              <span className="sidebar-count">
                {project.count}
                <span className="visually-hidden">{project.count === 1 ? " task" : " tasks"}</span>
              </span>
            </button>
          </li>
        ))}
      </ul>
      {projects.length > PROJECT_LIMIT && (
        <button className="sidebar-more" type="button" aria-expanded={showAll} onClick={() => setShowAll(!showAll)}>
          {showAll ? "Show fewer" : `Show all ${projects.length}`}
        </button>
      )}
    </>
  );
}

export default memo(Sidebar);
