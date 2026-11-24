// Sidebar choices that outlive a launch: whether it is shown, and its width.

import { readStorage, writeStorage } from "./storage";

export const SIDEBAR_MIN_WIDTH = 208;
export const SIDEBAR_MAX_WIDTH = 360;
export const SIDEBAR_DEFAULT_WIDTH = 248;

const SIDEBAR_COLLAPSED_KEY = "taskSidebarCollapsed";
const SIDEBAR_WIDTH_KEY = "taskSidebarWidth";

export type TaskArchiveFilter = "active" | "only" | "include";

export interface SidebarPreferences {
  sidebarCollapsed: boolean;
  sidebarWidth: number;
}

function defaultSidebarPreferences(): SidebarPreferences {
  return { sidebarCollapsed: false, sidebarWidth: SIDEBAR_DEFAULT_WIDTH };
}

export function loadSidebarPreferences(): SidebarPreferences {
  const preferences = defaultSidebarPreferences();
  preferences.sidebarCollapsed = readStorage(SIDEBAR_COLLAPSED_KEY) === "1";

  const width = readStorage(SIDEBAR_WIDTH_KEY);
  if (width !== undefined) {
    const parsed = Number.parseInt(width, 10);
    if (Number.isFinite(parsed)) {
      preferences.sidebarWidth = Math.min(Math.max(parsed, SIDEBAR_MIN_WIDTH), SIDEBAR_MAX_WIDTH);
    }
  }

  return preferences;
}

export function storeSidebarPreferences(preferences: SidebarPreferences): void {
  writeStorage(SIDEBAR_COLLAPSED_KEY, preferences.sidebarCollapsed ? "1" : "0");
  writeStorage(SIDEBAR_WIDTH_KEY, String(preferences.sidebarWidth));
}
