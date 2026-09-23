// The sidebar's search field and task list, reachable from both the native
// File/View menus and the in-page shortcuts without either owning the other.

import type { SidebarController } from "@/state";

const SEARCH_SELECTOR = "input[data-task-search]";
const SIDEBAR_SELECTOR = ".task-sidebar";

export function focusTaskSearch(): void {
  document.querySelector<HTMLInputElement>(SEARCH_SELECTOR)?.focus();
}

export function focusTaskList(): void {
  const list = document.getElementById("task-list");
  const row = list?.querySelector<HTMLElement>('[aria-selected="true"]')
    ?? list?.querySelector<HTMLElement>('[tabindex="0"]');
  row?.focus();
}

/** Toggles the sidebar and keeps focus sane either way: opening it focuses
 * the task list, and hiding it moves focus off whatever was inside it. */
export function toggleSidebarAndManageFocus(sidebar: SidebarController): void {
  const wasCollapsed = sidebar.snapshot.sidebarCollapsed;
  sidebar.toggleSidebar();
  if (wasCollapsed) {
    requestAnimationFrame(focusTaskList);
    return;
  }
  const active = document.activeElement;
  if (active instanceof HTMLElement && active.closest(SIDEBAR_SELECTOR)) active.blur();
}
