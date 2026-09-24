// Native menu commands dispatched into the running app.

import { onMenuCommand } from "@/bridge";
import { getTransport } from "@/bridge/transport";
import { setMenuItemEnabled } from "@/bridge/client";
import type { MenuCommand } from "@/bridge/types";
import type { SidebarController } from "@/state";
import { type Route, routePath } from "@/router";
import { REFRESH_TASK_DETAIL_EVENT } from "@/screens/task/TaskDetail";
import { focusTaskSearch, toggleSidebarAndManageFocus } from "./taskSearch";

const ZOOM_STEPS = [0.75, 0.85, 1, 1.1, 1.25, 1.4, 1.6, 1.8, 2] as const;
const DEFAULT_ZOOM_INDEX = ZOOM_STEPS.indexOf(1);

// Zoom lasts for the window's lifetime.
let zoomIndex = DEFAULT_ZOOM_INDEX;

// The desktop shell zooms the webview; CSS is the fallback outside it.
function applyZoom(): void {
  if (typeof document === "undefined") return;
  const scale = ZOOM_STEPS[zoomIndex];
  if ("__TAURI__" in window) {
    void getTransport().invoke("plugin:webview|set_webview_zoom", { label: "main", value: scale }).catch(() => undefined);
    return;
  }
  (document.documentElement.style as CSSStyleDeclaration & { zoom?: string }).zoom = String(scale);
}

function zoomIn(): void {
  zoomIndex = Math.min(zoomIndex + 1, ZOOM_STEPS.length - 1);
  applyZoom();
}

function zoomOut(): void {
  zoomIndex = Math.max(zoomIndex - 1, 0);
  applyZoom();
}

function zoomReset(): void {
  zoomIndex = DEFAULT_ZOOM_INDEX;
  applyZoom();
}

function focusPanel(selector: string): void {
  if (typeof document === "undefined") return;
  document.querySelector(selector)?.scrollIntoView({ block: "start" });
}

function clickControl(selector: string): void {
  if (typeof document === "undefined") return;
  document.querySelector<HTMLElement>(selector)?.click();
}

function scrollAllIntoView(selector: string, block: ScrollLogicalPosition): void {
  if (typeof document === "undefined") return;
  document.querySelectorAll(selector).forEach((el) => el.scrollIntoView({ block }));
}

export interface MenuCommandContext {
  sidebar: SidebarController;
  route: Route;
  navigate: (route: Route) => void;
  checkForUpdates?: () => void;
}

function runMenuCommand(command: MenuCommand, context: MenuCommandContext): void {
  switch (command) {
    case "toggle-sidebar":
      toggleSidebarAndManageFocus(context.sidebar);
      return;
    case "toggle-inspector":
      if (context.route.kind !== "task") return;
      clickControl('[aria-controls="changed-files-panel"]');
      return;
    case "refresh-tasks":
      void context.sidebar.refresh();
      if (context.route.kind === "task") window.dispatchEvent(new Event(REFRESH_TASK_DETAIL_EVENT));
      return;
    case "clear-selection":
      context.sidebar.clearSelection();
      if (routePath(context.route) !== "/") context.navigate({ kind: "home" });
      return;
    case "show-activity":
      focusPanel(".transcript");
      return;
    case "show-request":
      scrollAllIntoView(".transcript-row-user", "start");
      return;
    case "show-response":
      scrollAllIntoView(".detail-response", "end");
      return;
    case "zoom-in":
      zoomIn();
      return;
    case "zoom-out":
      zoomOut();
      return;
    case "zoom-reset":
      zoomReset();
      return;
    case "open-settings":
      context.navigate({ kind: "settings" });
      return;
    case "check-for-updates":
      context.navigate({ kind: "settings", tab: "about" });
      context.checkForUpdates?.();
      return;
    case "history-back":
      window.history.back();
      return;
    case "history-forward":
      window.history.forward();
      return;
    case "find-task":
      // The search field lives in the sidebar, so a collapsed sidebar has to
      // open before there is anything to type into.
      if (context.sidebar.snapshot.sidebarCollapsed) {
        context.sidebar.toggleSidebar();
        requestAnimationFrame(focusTaskSearch);
        return;
      }
      focusTaskSearch();
      return;
  }
}

/** Keeps the native menu's enabled state in step with what the current route supports. */
export function syncMenuAvailability(route: Route): void {
  void setMenuItemEnabled("toggle-inspector", route.kind === "task");
}

// The native "Zoom In" accelerator is CmdOrCtrl+=, matching the menu label.
// Many people instead press Cmd+Shift+= (i.e. Cmd++), which isn't a second
// accelerator the native menu can bind to the same item, so it's handled
// here instead. Matched on `code` rather than `key`: Shift+= reliably
// reports as physical key "Equal" across layouts, whereas the `key` value
// with Cmd held varies by browser/OS.
function isZoomInAltShortcut(event: KeyboardEvent): boolean {
  return (event.metaKey || event.ctrlKey) && event.shiftKey && event.code === "Equal";
}

function handleZoomAltShortcut(event: KeyboardEvent): void {
  if (!isZoomInAltShortcut(event)) return;
  event.preventDefault();
  zoomIn();
}

/** Subscribes to native menu commands for the lifetime of the caller. */
export function subscribeMenuCommands(getContext: () => MenuCommandContext): () => void {
  applyZoom();
  window.addEventListener("keydown", handleZoomAltShortcut);
  const unsubscribe = onMenuCommand((command) => runMenuCommand(command, getContext()));
  return () => {
    window.removeEventListener("keydown", handleZoomAltShortcut);
    unsubscribe();
  };
}
