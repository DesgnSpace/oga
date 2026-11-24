// Sidebar focus helpers for native menus and in-page shortcuts.

import type { SidebarController } from "@/state";

const SEARCH_SELECTOR = "input[data-task-search]";
const SIDEBAR_SELECTOR = ".task-sidebar";

export function focusTaskSearch(): void {
  document.querySelector<HTMLInputElement>(SEARCH_SELECTOR)?.focus();
}

export function focusSidebarNav(): void {
  const nav = document.querySelector(`${SIDEBAR_SELECTOR} nav`);
  const item = nav?.querySelector<HTMLElement>('[aria-current="page"]') ?? nav?.querySelector<HTMLElement>("button");
  item?.focus();
}

export function toggleSidebarAndManageFocus(sidebar: SidebarController): void {
  const wasCollapsed = sidebar.snapshot.sidebarCollapsed;
  sidebar.toggleSidebar();
  if (wasCollapsed) {
    requestAnimationFrame(focusSidebarNav);
    return;
  }
  const active = document.activeElement;
  if (active instanceof HTMLElement && active.closest(SIDEBAR_SELECTOR)) active.blur();
}
