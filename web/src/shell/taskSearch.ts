// Sidebar focus helpers for native menus and in-page shortcuts.

import type { SidebarController } from "@/state";

const SEARCH_SELECTOR = "input[data-task-search]";
const SEARCH_TOGGLE_SELECTOR = "button[data-task-search-toggle]";
const SIDEBAR_SELECTOR = ".task-sidebar";

/** The field only exists while search is open; the toggle opens and focuses it. */
export function focusTaskSearch(): void {
  const field = document.querySelector<HTMLInputElement>(SEARCH_SELECTOR);
  if (field) field.focus();
  else document.querySelector<HTMLButtonElement>(SEARCH_TOGGLE_SELECTOR)?.click();
}

export function focusTaskList(): void {
  const list = document.getElementById("task-list");
  const row = list?.querySelector<HTMLElement>('[aria-selected="true"]')
    ?? list?.querySelector<HTMLElement>('[tabindex="0"]');
  row?.focus();
}

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
