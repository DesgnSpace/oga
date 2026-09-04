import { useEffect } from "react";
import { focusTaskSearch } from "./taskSearch";

interface KeyboardShortcutHandlers {
  onBack: () => void;
  onForward: () => void;
  onSettings: () => void;
  onUsage: () => void;
  onRefresh: () => void;
}

function hasDesktopBridge(): boolean {
  return typeof window !== "undefined" && "__TAURI__" in window;
}

function isTextField(target: EventTarget | null): boolean {
  return target instanceof HTMLInputElement
    || target instanceof HTMLTextAreaElement
    || target instanceof HTMLSelectElement
    || (target instanceof HTMLElement && target.isContentEditable);
}

/** In the desktop app these same shortcuts are menu accelerators, so binding
 * them here as well would run each action twice. */
export function useKeyboardShortcuts({ onBack, onForward, onSettings, onUsage, onRefresh }: KeyboardShortcutHandlers): void {
  useEffect(() => {
    if (hasDesktopBridge()) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey) return;

      const key = event.key.toLowerCase();
      const allowedInTextField = key === "," || key === "k";
      if (isTextField(event.target) && !allowedInTextField) return;

      let action: (() => void) | undefined;
      switch (key) {
        case "k":
          action = focusTaskSearch;
          break;
        case "[":
          action = onBack;
          break;
        case "]":
          action = onForward;
          break;
        case ",":
          action = onSettings;
          break;
        case "u":
          if (event.shiftKey) action = onUsage;
          break;
        case "r":
          action = onRefresh;
          break;
      }
      if (!action) return;
      event.preventDefault();
      action();
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onBack, onForward, onRefresh, onSettings, onUsage]);
}
