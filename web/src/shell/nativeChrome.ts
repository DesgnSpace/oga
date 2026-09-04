// Webview defaults a desktop app should not have: the macOS beep on any
// keystroke the page doesn't consume, the browser's own right-click menu, and
// links that would replace the app with a web page.

import { getTransport } from "@/bridge/transport";

/** Where selection is allowed, so is the right-click menu that acts on it.
 * Kept in step with the `user-select: text` opt-ins in oga.css. */
const COPYABLE = "input, textarea, [contenteditable], pre, code, .markdown-content, .code-line-text, .task-detail-fact";

function isRoutedElsewhere(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) return true;
  if (target.isContentEditable) return true;
  // Elements with their own default keydown behaviour (button/link
  // activation via Space or Enter, focus-managed controls) keep it.
  return target.closest("button, a, select, [role='button'], [tabindex]") !== null;
}

function suppressKeystrokeBeep(): void {
  window.addEventListener("keydown", (event) => {
    // A modifier held means this is a shortcut (menu accelerator or
    // otherwise), not a stray keystroke that would beep. preventDefault
    // here would stop the webview from ever offering it to the native menu.
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const target = event.composedPath()[0] ?? event.target;
    if (!isRoutedElsewhere(target)) {
      event.preventDefault();
    }
  });
}

function suppressContextMenu(): void {
  document.addEventListener("contextmenu", (event) => {
    const target = event.target as HTMLElement | null;
    if (!target?.closest(COPYABLE)) {
      event.preventDefault();
    }
  });
}

function isExternal(href: string): boolean {
  return /^(https?:|mailto:)/.test(href);
}

/** Hands a link to the browser or mail client. Falls back to a new tab when
 * the app is running outside the desktop shell. */
async function openExternal(url: string): Promise<void> {
  try {
    await getTransport().invoke("open_external_link", { url });
  } catch {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}

/** Every external link leaves the window, whichever screen drew it, so the app
 * is never replaced by a web page. Internal routes are the router's. */
function keepLinksOutOfTheWindow(): void {
  document.addEventListener("click", (event) => {
    if (event.defaultPrevented || event.button !== 0) return;
    if (!(event.target instanceof HTMLElement)) return;
    const href = event.target.closest("a")?.getAttribute("href");
    if (!href || !isExternal(href)) return;
    event.preventDefault();
    void openExternal(href);
  });
}

/** Installs the webview-level fixes that make the app stop feeling like a page. */
export function installNativeChrome(): void {
  suppressKeystrokeBeep();
  keepLinksOutOfTheWindow();
  if (import.meta.env.PROD) {
    suppressContextMenu();
  }
}
