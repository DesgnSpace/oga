import { useLayoutEffect, useRef, useState } from "react";
import type { BridgeError, TaskSummary } from "@/bridge/types";
import { MenuPanel, type MenuAction } from "@/components/menu/Menu";
import {
  archiveTitles,
  canComplete,
  canPause,
  canResume,
  completeTitles,
  executeArchive,
  executeCancel,
  executeComplete,
  executeResume,
  resumeTitles,
  stopTitles,
  type TaskToastTitles,
} from "@/screens/task/Actions";
import { toast } from "@/state/toast";
import { ArchiveIcon, CancelIcon, CheckIcon, PlayIcon, RestoreIcon } from "@/ui/icons";

/** Gap kept between the menu, its button, and the window edge. */
const MENU_MARGIN = 8;

type TaskAction = () => Promise<{ ok: true; value: unknown } | { ok: false; error: BridgeError }>;

function sections(task: TaskSummary, run: (action: TaskAction, titles: TaskToastTitles) => void, offline: boolean, onDeleteBranch: () => void): MenuAction[][] {
  const archived = task.archivedAt !== undefined;
  const lifecycle: MenuAction[] = [];
  if (canPause(task)) {
    lifecycle.push({ key: "stop", label: "Stop", icon: <CancelIcon />, disabled: offline, onSelect: () => run(() => executeCancel(task.id), stopTitles(task)) });
  }
  if (canResume(task)) {
    lifecycle.push({ key: "resume", label: "Resume", icon: <PlayIcon />, disabled: offline, onSelect: () => run(() => executeResume(task.id), resumeTitles(task)) });
  }
  const admin: MenuAction[] = [{
    key: "archive",
    label: archived ? "Restore" : "Archive",
    icon: archived ? <RestoreIcon /> : <ArchiveIcon />,
    disabled: offline,
    onSelect: () => run(() => executeArchive(task.id, !archived), archiveTitles(task, archived)),
  }];
  if (!archived && task.originCwd !== undefined && task.branch !== undefined) {
    admin.push({ key: "archive-delete-branch", label: "Archive and delete branch", icon: <ArchiveIcon />, destructive: true, disabled: offline, onSelect: onDeleteBranch });
  }
  if (canComplete(task)) {
    admin.push({ key: "complete", label: "Mark as completed", icon: <CheckIcon />, disabled: offline, onSelect: () => run(() => executeComplete(task.id), completeTitles(task)) });
  }
  return [lifecycle, admin];
}

function runTaskAction(action: TaskAction, titles: TaskToastTitles, onDone: () => void): void {
  const run = () => {
    const lifecycle = toast.pending(titles.pending, { retry: run });
    void action().then((result) => {
      if (result.ok) {
        lifecycle.dismiss();
        onDone();
      } else {
        lifecycle.error(`Couldn't ${titles.failure}`, { description: "Try again.", detail: result.error.message });
      }
    }).catch((reason) => {
      lifecycle.error("Couldn't reach Oga", { description: "Check the connection and try again.", detail: String(reason) });
    });
  };
  run();
}

export function TaskMenu({ task, anchor, offline, onClose, onChanged, onDeleteBranch }: {
  task: TaskSummary;
  anchor: HTMLElement;
  offline: boolean;
  onClose: () => void;
  onChanged: () => void;
  onDeleteBranch: () => void;
}) {
  const panelRef = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState<{ top: number; left: number }>();

  useLayoutEffect(() => {
    const panel = panelRef.current;
    if (!panel) return;
    const anchorRect = anchor.getBoundingClientRect();
    const { width, height } = panel.getBoundingClientRect();
    const left = Math.max(MENU_MARGIN, Math.min(anchorRect.right - width, window.innerWidth - width - MENU_MARGIN));
    const below = anchorRect.bottom + MENU_MARGIN;
    const top = below + height + MENU_MARGIN <= window.innerHeight ? below : Math.max(MENU_MARGIN, anchorRect.top - height - MENU_MARGIN);
    // The app zooms the whole page, and a fixed box is placed in unzoomed units.
    const zoom = Number.parseFloat(getComputedStyle(document.documentElement).zoom);
    const scale = Number.isFinite(zoom) && zoom > 0 ? zoom : 1;
    setPlace({ top: top / scale, left: left / scale });
  }, [anchor]);

  useLayoutEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("resize", onClose);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("resize", onClose);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [onClose]);

  const run = (action: TaskAction, titles: TaskToastTitles) => {
    onClose();
    runTaskAction(action, titles, onChanged);
  };

  return (
    <div
      ref={panelRef}
      className="task-row-menu"
      style={{ top: place?.top, left: place?.left, visibility: place ? undefined : "hidden" }}
    >
      <MenuPanel sections={sections(task, run, offline, onDeleteBranch)} onClose={onClose} />
    </div>
  );
}
