// The sidebar's search field, reachable from both the native File menu and
// the in-page shortcut without either owning the other.

const SEARCH_SELECTOR = "input[data-task-search]";

export function focusTaskSearch(): void {
  document.querySelector<HTMLInputElement>(SEARCH_SELECTOR)?.focus();
}
