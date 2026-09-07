// The persistent chrome: the task sidebar beside whichever screen the route
// picks. Mirrors rust/crates/oga-ui/src/lib.rs `App`.

import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { SidebarController } from "@/state";
import type { ConnectionState } from "@/state/sidebar-state";
import { readStorage, writeStorage } from "@/state/storage";
import { Sidebar } from "@/screens/sidebar";
import { LoadingState } from "@/components/atoms/ListState";
import { type Route, RouterProvider, handlesClick, routePath, useRouter } from "@/router";
import { Modal } from "@/components/primitives/Modal";
import { ToastViewport } from "@/components/ToastViewport";
import { TitleBar, type TaskTitleBarInfo } from "./TitleBar";
import { useKeyboardShortcuts } from "./useKeyboardShortcuts";
import { useAppUpdates } from "./useAppUpdates";

const LAST_SELECTED_TASK_KEY = "lastSelectedTask";

const SettingsPage = lazy(() => import("@/screens/settings").then(({ SettingsPage }) => ({ default: SettingsPage })));
const UsagePage = lazy(() => import("@/screens/usage").then(({ UsagePage }) => ({ default: UsagePage })));
const TaskDetailPage = lazy(() => import("@/screens/task/TaskDetail").then(({ TaskDetail }) => ({ default: TaskDetail })));
const IconGallery = lazy(() => import("@/ui/IconGallery").then(({ IconGallery }) => ({ default: IconGallery })));

type MenuCommandsModule = typeof import("./menuCommands");
let menuCommandsModule: Promise<MenuCommandsModule> | undefined;

function loadMenuCommands(): Promise<MenuCommandsModule> {
  return (menuCommandsModule ??= import("./menuCommands"));
}

type IdleWindow = Window & {
  requestIdleCallback?: (callback: () => void, options?: { timeout: number }) => number;
  cancelIdleCallback?: (handle: number) => void;
};

function scheduleMenuCommands(work: () => void): () => void {
  const browserWindow: IdleWindow | undefined = globalThis.window;
  if (browserWindow?.requestIdleCallback) {
    const handle = browserWindow.requestIdleCallback(work, { timeout: 200 });
    return () => browserWindow.cancelIdleCallback?.(handle);
  }
  const handle = globalThis.setTimeout(work, 0);
  return () => globalThis.clearTimeout(handle);
}

function ScreenLoading({ route }: { route: Route }) {
  const label = route.kind === "task" ? "Loading task activity…" : route.kind === "settings" ? "Loading settings…" : route.kind === "usage" ? "Loading usage…" : "Loading…";
  return <LoadingState label={label} />;
}

function TaskDetailRoute({ taskId, onHeader }: { taskId: string; onHeader: (info: TaskTitleBarInfo | undefined) => void }) {
  const [, forceUpdate] = useState(0);
  // Task detail starts its request during render, so read its snapshot again
  // after the subscription effects have been installed.
  useEffect(() => {
    const handle = setTimeout(() => forceUpdate((value) => value + 1), 0);
    return () => clearTimeout(handle);
  }, [taskId]);
  return <TaskDetailPage taskId={taskId} onHeader={onHeader} />;
}

function offlineBannerCopy(connection: ConnectionState): string | undefined {
  switch (connection) {
    case "connecting":
      return "Connecting to Oga…";
    case "reconnecting":
      return "Reconnecting to Oga…";
    case "offline":
      return "Oga is offline. New actions won't save until it's back.";
    case "connected":
      return undefined;
  }
}

function OfflineBanner({ connection, onRetry }: { connection: ConnectionState; onRetry: () => void }) {
  const copy = offlineBannerCopy(connection);
  if (!copy) return null;
  return (
    <div className="offline-banner" role="status">
      <span>{copy}</span>
      <button className="text-button" type="button" onClick={onRetry}>
        Retry
      </button>
    </div>
  );
}

function EmptyWorkspace({ sidebarController, onOpenSettings }: { sidebarController: SidebarController; onOpenSettings: (tab: "workers" | "connections") => void }) {
  const sidebar = useSyncExternalStore(
    (listener) => sidebarController.subscribe(listener),
    () => sidebarController.snapshot,
  );
  const hasTasks = sidebar.tasks.length > 0;
  const hasConnectedAi = sidebar.profiles.length > 0;

  if (sidebar.loadState === "loading" && !hasTasks) {
    return (
      <>
        <p className="eyebrow">Workspace</p>
        <h1 id="page-title">Your workspace</h1>
      </>
    );
  }

  if (sidebar.loadState === "error" && !hasTasks) {
    return (
      <>
        <p className="eyebrow">Workspace</p>
        <h1 id="page-title">Your workspace</h1>
        <p className="app-description">Couldn&apos;t load your workspace. Use Retry to try again.</p>
      </>
    );
  }

  if (sidebar.loadState === "ready" && !hasConnectedAi) {
    return (
      <div className="app-first-run-card">
        <p className="eyebrow">Workspace</p>
        <h1 id="page-title">Connect your AI to start delegating</h1>
        <p className="app-description">Choose an AI account for the work you want to hand off.</p>
        <button className="settings-button settings-button-primary" type="button" onClick={() => onOpenSettings("workers")}>
          Connect your AI
        </button>
      </div>
    );
  }

  if (hasTasks) {
    return (
      <>
        <p className="eyebrow">Workspace</p>
        <h1 id="page-title">Your workspace</h1>
        <p className="app-description">Choose a task from the sidebar.</p>
      </>
    );
  }

  return (
    <>
      <p className="eyebrow">Workspace</p>
      <h1 id="page-title">Get your first task running</h1>
      <ol className="app-first-run-steps">
        <li>
          Add a worker in{" "}
          <a
            href="/settings"
            onClick={(event) => {
              if (!handlesClick(event)) return;
              event.preventDefault();
              onOpenSettings("workers");
            }}
          >
            Settings ▸ Workers
          </a>
          .
        </li>
        <li>
          Connect your editor or agent in{" "}
          <a
            href="/settings"
            onClick={(event) => {
              if (!handlesClick(event)) return;
              event.preventDefault();
              onOpenSettings("connections");
            }}
          >
            Settings ▸ Connect tools
          </a>
          .
        </li>
        <li>Delegate a task from there. It shows up here.</li>
      </ol>
    </>
  );
}

function NotFoundScreen({ onBack }: { onBack: () => void }) {
  return (
    <>
      <p className="eyebrow">Workspace</p>
      <h1 id="page-title">Page not found</h1>
      <a
        className="home-link"
        href="/"
        onClick={(event) => {
          if (!handlesClick(event)) return;
          event.preventDefault();
          onBack();
        }}
      >
        Back to workspace
      </a>
    </>
  );
}

function contentClassName(route: Route): string {
  switch (route.kind) {
    case "task":
      return "app-content app-content-task";
    case "icons":
      return "app-content app-content-icons";
    default:
      return "app-content";
  }
}

function isOverlayRoute(route: Route): boolean {
  return route.kind === "settings" || route.kind === "usage";
}

function Shell() {
  const { route, navigate, canGoBack, canGoForward, goBack, goForward } = useRouter();
  const sidebarController = useMemo(() => new SidebarController(), []);
  const { updateStatus, checkForUpdates, installUpdate } = useAppUpdates();
  const initialTask = useRef(route.kind === "task" ? route.id : undefined).current;

  // The settings route now opens as a modal over whichever screen was
  // showing, so the content area keeps rendering that screen underneath
  // rather than swapping to settings. A direct load of /settings has no
  // screen underneath, so fall back to the last task the user had open.
  const [underlyingRoute, setUnderlyingRoute] = useState<Route>(() => {
    if (!isOverlayRoute(route)) return route;
    const lastTaskId = readStorage(LAST_SELECTED_TASK_KEY);
    return lastTaskId ? { kind: "task", id: lastTaskId } : { kind: "home" };
  });
  useEffect(() => {
    if (!isOverlayRoute(route)) setUnderlyingRoute(route);
    if (route.kind === "task") writeStorage(LAST_SELECTED_TASK_KEY, route.id);
  }, [route]);

  const closeSettings = useCallback(() => navigate(underlyingRoute), [navigate, underlyingRoute]);
  const openSettings = useCallback((tab: "workers" | "connections") => navigate({ kind: "settings", tab }), [navigate]);
  const openUsage = useCallback(() => navigate({ kind: "usage" }), [navigate]);

  const contextRef = useRef({ sidebar: sidebarController, route, navigate, checkForUpdates: () => void checkForUpdates() });
  contextRef.current = { sidebar: sidebarController, route, navigate, checkForUpdates: () => void checkForUpdates() };

  useEffect(() => {
    let active = true;
    let unsubscribe: (() => void) | undefined;
    const cancel = scheduleMenuCommands(() => {
      void loadMenuCommands().then(({ subscribeMenuCommands }) => {
        if (!active) return;
        unsubscribe = subscribeMenuCommands(() => contextRef.current);
      });
    });
    return () => {
      active = false;
      cancel();
      unsubscribe?.();
    };
  }, []);
  useEffect(() => {
    let active = true;
    const cancel = scheduleMenuCommands(() => {
      void loadMenuCommands().then(({ syncMenuAvailability }) => {
        if (active) syncMenuAvailability(route);
      });
    });
    return () => {
      active = false;
      cancel();
    };
  }, [route]);

  const connection = useSyncExternalStore(
    sidebarController.subscribe.bind(sidebarController),
    () => sidebarController.snapshot.connection,
    () => sidebarController.snapshot.connection,
  );
  const offline = connection !== "connected";
  const handleRetry = useCallback(() => void sidebarController.refresh(), [sidebarController]);
  useKeyboardShortcuts({ onBack: goBack, onForward: goForward, onSettings: () => openSettings("workers"), onUsage: openUsage, onRefresh: handleRetry });
  const sidebarCollapsed = useSyncExternalStore(
    sidebarController.subscribe.bind(sidebarController),
    () => sidebarController.snapshot.sidebarCollapsed,
    () => sidebarController.snapshot.sidebarCollapsed,
  );
  const [taskHeader, setTaskHeader] = useState<TaskTitleBarInfo>();
  const titleRoute = underlyingRoute;

  return (
    <main className="app-shell" data-route={routePath(route)}>
      <OfflineBanner connection={connection} onRetry={handleRetry} />
      <div className="app-layout">
        <Sidebar
          sidebarController={sidebarController}
          initialTask={initialTask}
          onSelectTask={(id) => navigate({ kind: "task", id })}
            onOpenSettings={openSettings}
            onOpenUsage={openUsage}
          navigation={{ canGoBack, canGoForward, onBack: goBack, onForward: goForward }}
        />
        <div className="app-main">
          <TitleBar
            route={titleRoute}
            sidebarCollapsed={sidebarCollapsed}
            onToggleSidebar={() => sidebarController.toggleSidebar()}
            canGoBack={canGoBack}
            canGoForward={canGoForward}
            onBack={goBack}
            onForward={goForward}
            task={taskHeader}
          />
          <section className={contentClassName(underlyingRoute)} aria-labelledby="page-title">
            <Suspense fallback={<ScreenLoading route={underlyingRoute} />}>
              {underlyingRoute.kind === "task" && <TaskDetailRoute taskId={underlyingRoute.id} onHeader={setTaskHeader} />}
              {underlyingRoute.kind === "home" && (
                <EmptyWorkspace sidebarController={sidebarController} onOpenSettings={openSettings} />
              )}
              {underlyingRoute.kind === "icons" && <IconGallery />}
              {underlyingRoute.kind === "not-found" && <NotFoundScreen onBack={() => navigate({ kind: "home" })} />}
            </Suspense>
          </section>
        </div>
      </div>
      <Modal
        open={route.kind === "settings"}
        onClose={closeSettings}
        labelledBy="settings-modal-title"
        className="modal-dialog-settings"
      >
        <Suspense fallback={<ScreenLoading route={route} />}>
          <SettingsPage
            open={route.kind === "settings"}
            offline={offline}
            initialTab={route.kind === "settings" ? route.tab : undefined}
            updateStatus={updateStatus}
            onCheckForUpdates={() => void checkForUpdates()}
            onInstallUpdate={() => void installUpdate()}
          />
        </Suspense>
      </Modal>
      <Modal open={route.kind === "usage"} onClose={() => navigate(underlyingRoute)} labelledBy="usage-modal-title" className="modal-dialog-usage">
        <Suspense fallback={<ScreenLoading route={route} />}>
          <UsagePage />
        </Suspense>
      </Modal>
      <ToastViewport />
    </main>
  );
}

export function AppShell() {
  return (
    <RouterProvider>
      <Shell />
    </RouterProvider>
  );
}
