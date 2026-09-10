import type { ReactNode } from "react";
import type { Route } from "@/router";
import { BackArrowIcon, ChangedFilesIcon, ForwardArrowIcon, SidebarIcon, TerminalIcon } from "@/ui/icons";
import { copyText } from "@/lib/identifiers";
import { toast } from "@/state/toast";

export const CHANGED_FILES_LABEL = "Changed files";
export const TERMINAL_RESUME_LABEL = "Continue in terminal";

export interface TaskTitleBarInfo {
  title: string;
  diffAdded: number;
  showingChanges: boolean;
  onToggleChanges: () => void;
  status: ReactNode;
  secondary: ReactNode;
  /** Shell command that continues this task's session elsewhere. Absent when the task holds no resumable session. */
  terminalCommand?: string;
}

function sidebarToggleLabel(collapsed: boolean): string {
  return collapsed ? "Show task list" : "Hide task list";
}

async function copyTerminalCommand(command: string): Promise<void> {
  try {
    await copyText(command);
    toast.success("Command copied");
  } catch {
    toast.error("Couldn't copy the command");
  }
}

function TerminalResumeButton({ command }: { command: string }) {
  return (
    <button
      className="icon-button"
      type="button"
      aria-label={TERMINAL_RESUME_LABEL}
      title={TERMINAL_RESUME_LABEL}
      onClick={() => void copyTerminalCommand(command)}
    >
      <TerminalIcon />
    </button>
  );
}

function fallbackTitle(route: Route): string {
  if (route.kind === "home") return "Your workspace";
  if (route.kind === "settings") return "Settings";
  if (route.kind === "usage") return "Usage";
  if (route.kind === "icons") return "Icons";
  if (route.kind === "not-found") return "Page not found";
  return "";
}

export function TitleBar({
  route,
  sidebarCollapsed,
  onToggleSidebar,
  canGoBack,
  canGoForward,
  onBack,
  onForward,
  task,
}: {
  route: Route;
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
  canGoBack: boolean;
  canGoForward: boolean;
  onBack: () => void;
  onForward: () => void;
  task: TaskTitleBarInfo | undefined;
}) {
  const title = task?.title || fallbackTitle(route);

  return (
    <header className="title-bar" data-tauri-drag-region>
      <div className="title-bar-row" data-tauri-drag-region>
        {sidebarCollapsed && (
          <>
            <span className="title-bar-inset" data-tauri-drag-region />
            <button
              className="icon-button"
              type="button"
              aria-expanded={!sidebarCollapsed}
              aria-controls="task-list"
              aria-label={sidebarToggleLabel(sidebarCollapsed)}
              title={sidebarToggleLabel(sidebarCollapsed)}
              onClick={onToggleSidebar}
            >
              <SidebarIcon />
            </button>
            <button
              className="icon-button"
              type="button"
              aria-label="Back"
              title="Back ⌘["
              disabled={!canGoBack}
              onClick={onBack}
            >
              <BackArrowIcon />
            </button>
            <button
              className="icon-button"
              type="button"
              aria-label="Forward"
              title="Forward ⌘]"
              disabled={!canGoForward}
              onClick={onForward}
            >
              <ForwardArrowIcon />
            </button>
          </>
        )}
        {task && <span className="title-bar-task-status">{task.status}</span>}
        <h1 id={task ? "page-title" : undefined} className="title-bar-title" title={title || undefined}>
          {title}
        </h1>
        <span className="title-bar-drag" data-tauri-drag-region />
        {task && (
          <div className="title-bar-actions">
            {task.diffAdded > 0 && <span className="title-bar-diff">{`+${task.diffAdded}`}</span>}
            {task.diffAdded > 0 && <span className="title-bar-stat-separator" aria-hidden="true">·</span>}
            <span className="title-bar-meta" data-tauri-drag-region={undefined}>
              {task.secondary}
            </span>
            <button
              className={`icon-button${task.showingChanges ? " icon-button-active" : ""}`}
              type="button"
              aria-expanded={task.showingChanges}
              aria-controls="changed-files-panel"
              aria-label={CHANGED_FILES_LABEL}
              title={CHANGED_FILES_LABEL}
              onClick={task.onToggleChanges}
            >
              <ChangedFilesIcon />
            </button>
            {task.terminalCommand !== undefined && (
              <TerminalResumeButton command={task.terminalCommand} />
            )}
          </div>
        )}
      </div>
    </header>
  );
}
