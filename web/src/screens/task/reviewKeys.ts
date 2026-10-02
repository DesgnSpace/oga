// Keyboard review inside the changed-files panel: move between files, open one,
// mark it read, and skip to the one still waiting.

import * as React from "react";
import type { ChangedFileView } from "@/domain/changes";
import { isTextField } from "@/shell/useKeyboardShortcuts";

export interface ReviewKeysOptions {
  /** The element the keys answer inside: the panel, or the full-screen dialog. */
  root: HTMLElement | null;
  /** The files as the panel lists them, in reading order. */
  files: readonly ChangedFileView[];
  /** The file the panel is on, which is none until a key or a click picks one. */
  activeFile?: ChangedFileView;
  isReviewed: (file: ChangedFileView) => boolean;
  /** Opens the file, brings it into view, and focuses its row. */
  goTo: (path: string) => void;
  /** Opens a closed file and closes an open one. */
  toggleOpen: (path: string) => void;
  /** Ticks the file off, or clears the tick it already has. */
  toggleReviewed: (file: ChangedFileView) => void;
}

/** One key, or a pair the reader holds one of. */
export interface ReviewShortcut {
  keys: string[][];
  action: string;
}

/**
 * What the review keys do, in the words the reader would use. The panel's
 * header and Settings both read this, so a key is described the same way in
 * both places. Enter opens or closes a file because the row on screen holds
 * the focus, not because the handler answers it.
 */
export const REVIEW_SHORTCUTS: ReviewShortcut[] = [
  { keys: [["J"], ["K"]], action: "Go to the next or previous file" },
  { keys: [["O"], ["↵"]], action: "Open or close the file you are on" },
  { keys: [["R"]], action: "Mark the file reviewed" },
  { keys: [["N"]], action: "Go to the next file you have not reviewed" },
];

/** The file `offset` places away from the one on screen, without wrapping. */
function stepFrom(
  files: readonly ChangedFileView[],
  activePath: string | undefined,
  offset: number,
): ChangedFileView | undefined {
  if (files.length === 0) return undefined;
  const from = files.findIndex((file) => file.path === activePath);
  if (from === -1) return offset > 0 ? files[0] : files[files.length - 1];
  const next = from + offset;
  return next < 0 || next >= files.length ? undefined : files[next];
}

/** The next file still waiting for a mark, wrapping round the end of the list. */
function nextUnreviewed(
  files: readonly ChangedFileView[],
  activePath: string | undefined,
  isReviewed: (file: ChangedFileView) => boolean,
): ChangedFileView | undefined {
  const from = files.findIndex((file) => file.path === activePath);
  for (let offset = 1; offset <= files.length; offset += 1) {
    const candidate = files[(from + offset + files.length) % files.length];
    if (!isReviewed(candidate)) return candidate;
  }
  return undefined;
}

export function useReviewKeys({
  root,
  files,
  activeFile,
  isReviewed,
  goTo,
  toggleOpen,
  toggleReviewed,
}: ReviewKeysOptions): void {
  React.useEffect(() => {
    if (!root) return;
    const onKeyDown = (event: KeyboardEvent) => {
      // A shortcut of the app's own always carries a modifier, and typing
      // always happens in a field: neither is the panel's to answer.
      if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
      if (isTextField(event.target)) return;
      const key = event.key.toLowerCase();
      const activePath = activeFile?.path;
      let action: (() => void) | undefined;
      switch (key) {
        case "j":
        case "k": {
          const file = stepFrom(files, activePath, key === "j" ? 1 : -1);
          if (file !== undefined) action = () => goTo(file.path);
          break;
        }
        case "n": {
          const file = nextUnreviewed(files, activePath, isReviewed);
          if (file !== undefined) action = () => goTo(file.path);
          break;
        }
        case "o":
          if (activeFile !== undefined) action = () => toggleOpen(activeFile.path);
          break;
        case "r":
          if (activeFile !== undefined) action = () => toggleReviewed(activeFile);
          break;
        default:
          return;
      }
      if (action === undefined) return;
      event.preventDefault();
      action();
    };
    root.addEventListener("keydown", onKeyDown);
    return () => root.removeEventListener("keydown", onKeyDown);
  }, [root, files, activeFile, isReviewed, goTo, toggleOpen, toggleReviewed]);
}
