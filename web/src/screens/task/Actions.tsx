// Task mutations, the header actions menu, and blocked-task explanations.
// Ported from rust/crates/oga-ui/src/actions/mod.rs — keep behavior and copy identical.

import * as React from "react";
import { createPortal } from "react-dom";
import { broker } from "@/bridge/client";
import type {
  ArchiveTaskResponse,
  BridgeError,
  CompletionCode,
  ModelSettingsSnapshot,
  ProfileView,
  Task,
  TaskCompletion,
  TaskEventView,
  TaskScope,
  TaskState,
} from "@/bridge/types";
import { MenuPanel, type MenuAction } from "@/components/menu/Menu";
import { Modal } from "@/components/primitives/Modal";
import { ArchiveIcon, CancelIcon, CheckIcon, MoreIcon, RestoreIcon } from "@/ui/icons";
import { MarkdownContent } from "@/domain/markdown";
import { ComposerRequest, ConversationComposer, isResume, routingForState } from "./Composer";
import { isExplainedWait, nextTryLabel } from "./format";
import { TaskMetadata } from "./TaskMetadata";
import { taskToastName } from "@/lib/toast-subject";
import { toast } from "@/state/toast";

/** Any task-like value with just the state a menu needs to gate on. */
export type TaskLike = { state: TaskState; archivedAt?: string };

export function executeArchive(taskId: string, archived: boolean, deleteBranch = false) {
  return broker.archiveTask(taskId, archived, deleteBranch);
}

export function executeCancel(taskId: string) {
  return broker.cancelTask(taskId);
}

export function executeResume(taskId: string, request: { instruction?: string; scope?: TaskScope } = {}) {
  return broker.resumeTask(taskId, request);
}

export function executeReply(taskId: string, answer: string, scope: TaskScope) {
  return broker.replyTask(taskId, { answer, scope });
}

export function executeSteer(taskId: string, instruction: string) {
  return broker.steerTask(taskId, { instruction });
}

export function executeQueue(taskId: string, instruction: string) {
  return broker.resumeTask(taskId, { instruction, queue: "add" });
}

export function executeHandoff(taskId: string, profile: string, model: string) {
  return broker.handoffTask(taskId, { profile, model });
}

export function executeComplete(taskId: string) {
  return broker.completeTask(taskId, { assertedBy: "Oga app", reason: "marked completed from the app" });
}

export function executeRemoveFollowUp(taskId: string, index: number) {
  return broker.removeFollowUp(taskId, index);
}

export function canCancel(task: TaskLike): boolean {
  return (["queued", "preparing_checkout", "pending", "running", "needs_input", "blocked"] as string[]).includes(task.state);
}

export function canComplete(task: TaskLike): boolean {
  return (
    (["queued", "preparing_checkout", "pending", "running", "needs_input", "answered", "blocked", "failed", "cancelled"] as string[]).includes(
      task.state,
    )
  );
}

/** Oga has no dedicated pause state: stopping a running task cancels its
 * worker, the same stop the header's confirmed "Stop" performs. */
export function canPause(task: TaskLike): boolean {
  return task.state === "running";
}

/** The counterpart to canPause: resuming picks a stopped (cancelled) task back
 * up. Other settled states (completed, failed) go through the task's own
 * "Continue" flow instead, not this menu. */
export function canResume(task: TaskLike): boolean {
  return task.state === "cancelled";
}

export function canHandoff(task: TaskLike): boolean {
  return !(["completed", "preparing_checkout", "removing_checkout"] as TaskState[]).includes(task.state);
}

function actionFailure(error: BridgeError, action: string): { title: string; options: { description?: string; detail?: string } } {
  if (error.status !== undefined) {
    return { title: `Couldn't ${action}`, options: { description: "Try again.", detail: error.message } };
  }
  return {
    title: "Couldn't reach Oga",
    options: { description: "Check the connection and try again.", detail: error.message },
  };
}

/**
 * Toast wording for one task action across its whole lifecycle: what is
 * starting, what finished, and what failed. The task name keeps each toast
 * pointed at the task it acts on; without a name the generic wording stands.
 */
export interface TaskToastTitles {
  pending: string;
  success: string;
  /** Completes `Couldn't ${failure}`, naming the same task. */
  failure: string;
}

export interface TaskToastSource {
  title?: string;
  prompt?: string;
  promptPreview?: string;
}

export function taskToastTitles(
  task: TaskToastSource,
  named: (quoted: string) => { pending: string; success: string; failure: string },
  generic: { pending: string; success: string; failure: string },
): TaskToastTitles {
  const name = taskToastName(task);
  if (!name) return generic;
  return named(`"${name}"`);
}

export function resumeTitles(task: TaskToastSource): TaskToastTitles {
  return taskToastTitles(task,
    (name) => ({ pending: `Resuming ${name}`, success: `${name} resumed`, failure: `resume ${name}` }),
    { pending: "Resuming task", success: "Task resumed", failure: "resume this task" });
}

export function stopTitles(task: TaskToastSource): TaskToastTitles {
  return taskToastTitles(task,
    (name) => ({ pending: `Stopping ${name}`, success: `${name} stopped`, failure: `stop ${name}` }),
    { pending: "Stopping task", success: "Task stopped", failure: "stop this task" });
}

export function completeTitles(task: TaskToastSource): TaskToastTitles {
  return taskToastTitles(task,
    (name) => ({ pending: `Completing ${name}`, success: `${name} marked completed`, failure: `complete ${name}` }),
    { pending: "Completing task", success: "Task marked completed", failure: "complete this task" });
}

export function archiveTitles(task: TaskToastSource, archived: boolean): TaskToastTitles {
  return taskToastTitles(task,
    (name) => archived
      ? { pending: `Restoring ${name}`, success: `${name} restored`, failure: `restore ${name}` }
      : { pending: `Archiving ${name}`, success: `${name} archived`, failure: `archive ${name}` },
    archived
      ? { pending: "Restoring task", success: "Task restored", failure: "restore this task" }
      : { pending: "Archiving task", success: "Task archived", failure: "archive this task" });
}

export function archiveBranchTitles(task: TaskToastSource): TaskToastTitles {
  return taskToastTitles(task,
    (name) => ({
      pending: `Archiving ${name} and deleting its branch`,
      success: `${name} archived; checkout removal started`,
      failure: `archive ${name} and delete its branch`,
    }),
    {
      pending: "Archiving task and deleting its branch",
      success: "Task archived; checkout removal started",
      failure: "archive this task and delete its branch",
    });
}

export function archiveBranchSuccess(task: TaskToastSource, response: ArchiveTaskResponse | undefined): string {
  const name = taskToastName(task);
  const subject = name ? `"${name}"` : "Task";
  if (!response) return `${subject} archived; branch status unavailable`;
  if (response.branchOutcome === "deleted") return `${subject} archived and branch deleted`;
  if (response.branchOutcome === "already_gone") return `${subject} archived; branch was already gone`;
  if (response.branchOutcome === "kept") {
    return response.branchReason
      ? `${subject} archived; branch kept: ${response.branchReason}`
      : `${subject} archived; branch kept`;
  }
  return `${subject} archived`;
}

export function replyTitles(task: TaskToastSource): TaskToastTitles {
  return taskToastTitles(task,
    (name) => ({ pending: `Sending reply for ${name}`, success: `Reply sent for ${name}`, failure: `send the reply for ${name}` }),
    { pending: "Sending reply", success: "Reply sent", failure: "send the reply" });
}

export function instructionTitles(task: TaskToastSource): TaskToastTitles {
  return taskToastTitles(task,
    (name) => ({ pending: `Sending instruction for ${name}`, success: `Instruction sent for ${name}`, failure: `send the instruction for ${name}` }),
    { pending: "Sending instruction", success: "Instruction sent", failure: "send the instruction" });
}

export function followUpTitles(task: TaskToastSource): TaskToastTitles {
  return taskToastTitles(task,
    (name) => ({ pending: `Queueing follow-up for ${name}`, success: `Follow-up queued for ${name}`, failure: `queue the follow-up for ${name}` }),
    { pending: "Queueing follow-up", success: "Follow-up queued", failure: "queue the follow-up" });
}

export function removeFollowUpTitles(task: TaskToastSource): TaskToastTitles {
  return taskToastTitles(task,
    (name) => ({ pending: `Removing follow-up for ${name}`, success: `Follow-up removed for ${name}`, failure: `remove the follow-up for ${name}` }),
    { pending: "Removing follow-up", success: "Follow-up removed", failure: "remove the follow-up" });
}

export function moveTitles(task: TaskToastSource): TaskToastTitles {
  return taskToastTitles(task,
    (name) => ({ pending: `Moving ${name}`, success: `${name} moved to another worker`, failure: `move ${name}` }),
    { pending: "Moving task", success: "Task moved to another worker", failure: "move this task" });
}

export interface BlockedExplanation {
  headline: string;
  rawReason?: string;
  deniedPaths: string[];
  suggestedScope?: TaskScope;
}

export interface ArchiveBranchTask extends TaskToastSource {
  id: string;
  branch: string;
  state: TaskState;
}

export function ArchiveBranchDialog({
  task,
  open,
  onClose,
  onChanged,
}: {
  task: ArchiveBranchTask;
  open: boolean;
  onClose: () => void;
  onChanged: () => void;
}) {
  const [busy, setBusy] = React.useState(false);
  const titleId = React.useId();
  // SAFETY: task.state is the domain state used by the archive action.
  const stopsBeforeArchive = !(["completed", "failed", "cancelled", "removing_checkout"] as TaskState[]).includes(task.state);

  const archive = async () => {
    if (busy) return;
    setBusy(true);
    const titles = archiveBranchTitles(task);
    const lifecycle = toast.pending(titles.pending);
    try {
      const result = await executeArchive(task.id, true, true);
      setBusy(false);
      if (result.ok) {
        lifecycle.success(archiveBranchSuccess(task, result.value));
        onChanged();
      } else {
        const failure = actionFailure(result.error, titles.failure);
        lifecycle.error(failure.title, failure.options);
      }
    } catch (error) {
      setBusy(false);
      const failure = actionFailure(
        { message: error instanceof Error ? error.message : "Unknown error" },
        titles.failure,
      );
      lifecycle.error(failure.title, failure.options);
    }
  };

  return (
    <Modal open={open} onClose={onClose} labelledBy={titleId} className="modal-dialog-cancel">
      <h2 id={titleId}>Archive task and delete branch?</h2>
      <p>
        {stopsBeforeArchive
          ? "Oga stops and archives this task, then removes its checkout if it is clean and unused. "
          : "This archives the task and removes its checkout if it is clean and unused. "}
        Oga deletes <code>{task.branch}</code> only when it has no unmerged commits and no other checkout uses it. Uncommitted work keeps the checkout and branch.
      </p>
      <div className="handoff-actions">
        <button className="settings-button" type="button" onClick={onClose} disabled={busy}>
          Keep task
        </button>
        <button className="settings-button settings-button-danger" type="button" onClick={() => void archive()} disabled={busy}>
          Archive and delete
        </button>
      </div>
    </Modal>
  );
}

function deniedPaths(current: TaskScope | undefined, suggested: TaskScope | undefined): string[] {
  if (!suggested) return [];
  const currentWrite = current?.write ?? [];
  const newWrite = suggested.write.filter((path) => !currentWrite.includes(path));
  if (newWrite.length > 0) return newWrite;
  const currentRead = current?.read ?? [];
  return suggested.read.filter((path) => !currentRead.includes(path));
}

/** Explains why a task is blocked, in the copy the reference draws. */
export function explainBlocked(
  completion: TaskCompletion | undefined,
  currentScope: TaskScope | undefined,
): BlockedExplanation {
  if (!completion) {
    return {
      headline: "This task stopped unexpectedly, and there's no record of why.",
      deniedPaths: [],
    };
  }
  const rawReason = completion.reason?.trim();
  const reason = rawReason && rawReason !== "" ? rawReason : undefined;
  const code: CompletionCode = completion.code;

  if (code === "permission_denied") {
    const paths = deniedPaths(currentScope, completion.suggestedScope);
    const headline =
      paths.length === 0
        ? "The worker was stopped before it could reach a file or folder it needed."
        : `The worker was stopped before it could reach ${paths.join(", ")}.`;
    return { headline, rawReason: reason, deniedPaths: paths, suggestedScope: completion.suggestedScope };
  }
  if (code === "needs_authority") {
    return {
      headline: "The worker stopped because it needed a decision it could not make on its own.",
      rawReason: reason,
      deniedPaths: [],
    };
  }
  if (reason?.toLowerCase().includes("broker restarted")) {
    return {
      headline: "The run was interrupted when Oga restarted.",
      rawReason: reason,
      deniedPaths: [],
    };
  }
  return {
    headline: "The worker stopped before it could finish.",
    rawReason: reason,
    deniedPaths: [],
  };
}

function StopConfirmDialog({
  open,
  onClose,
  onConfirm,
  busy,
}: {
  open: boolean;
  onClose: () => void;
  onConfirm: () => void;
  busy?: boolean;
}) {
  return (
    <Modal open={open} onClose={onClose} labelledBy="cancel-dialog-title" className="modal-dialog-cancel">
      <h2 id="cancel-dialog-title">Stop this task?</h2>
      <p>The worker stops. You can resume it later.</p>
      <div className="handoff-actions">
        <button className="settings-button" type="button" onClick={onClose} disabled={busy}>
          Don&apos;t stop
        </button>
        <button className="settings-button settings-button-danger" type="button" onClick={onConfirm} disabled={busy}>
          Stop task
        </button>
      </div>
    </Modal>
  );
}

/** Gap between the header trigger and its menu, and between the menu and the viewport edge. */
const HEADER_MENU_MARGIN = 8;

interface HeaderMenuPlacement {
  top: number;
  left: number;
  maxHeight: number;
}

/** The header's ellipsis menu: cancel, archive, mark completed. */
export function TaskHeaderActions({ task, onChanged }: { task: Task; onChanged: () => void }) {
  const [busy, setBusy] = React.useState(false);
  const [menuOpen, setMenuOpen] = React.useState(false);
  const [confirmingCancel, setConfirmingCancel] = React.useState(false);
  const [confirmingArchiveBranch, setConfirmingArchiveBranch] = React.useState(false);
  const [handoffOpen, setHandoffOpen] = React.useState(false);
  const [placement, setPlacement] = React.useState<HeaderMenuPlacement | null>(null);
  const triggerRef = React.useRef<HTMLDivElement>(null);
  const panelRef = React.useRef<HTMLDivElement>(null);
  const cancellable = canCancel(task);
  const completable = canComplete(task) && task.state !== "blocked";
  const handoffable = canHandoff(task);
  const archived = task.archivedAt !== undefined;
  const branch = task.worktree?.branch;
  const canDeleteBranch = !archived && branch !== undefined;

  React.useEffect(() => {
    if (!menuOpen) return;
    const closeOnPointer = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (target && (triggerRef.current?.contains(target) || panelRef.current?.contains(target))) return;
      setMenuOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenuOpen(false);
    };
    document.addEventListener("pointerdown", closeOnPointer);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointer);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [menuOpen]);

  // The menu portals to the body: the title bar clips its own row to
  // truncate the stats at narrow widths, and that clipping would clip an
  // inline menu too.
  React.useLayoutEffect(() => {
    if (!menuOpen) {
      setPlacement(null);
      return;
    }
    const update = () => {
      const trigger = triggerRef.current;
      const panel = panelRef.current;
      if (!trigger || !panel) return;
      const triggerRect = trigger.getBoundingClientRect();
      const { width, height } = panel.getBoundingClientRect();
      const viewportWidth = window.innerWidth;
      const viewportHeight = window.innerHeight;
      const left = Math.max(
        HEADER_MENU_MARGIN,
        Math.min(triggerRect.right - width, viewportWidth - width - HEADER_MENU_MARGIN),
      );
      const below = triggerRect.bottom + HEADER_MENU_MARGIN;
      let next: HeaderMenuPlacement;
      if (below + height + HEADER_MENU_MARGIN <= viewportHeight) {
        next = { top: below, left, maxHeight: viewportHeight - below - HEADER_MENU_MARGIN };
      } else {
        const above = triggerRect.top - height - HEADER_MENU_MARGIN;
        next =
          above >= HEADER_MENU_MARGIN
            ? { top: above, left, maxHeight: Math.max(0, triggerRect.top - HEADER_MENU_MARGIN * 2) }
            : { top: HEADER_MENU_MARGIN, left, maxHeight: viewportHeight - HEADER_MENU_MARGIN * 2 };
      }
      setPlacement((previous) =>
        previous && previous.top === next.top && previous.left === next.left && previous.maxHeight === next.maxHeight
          ? previous
          : next,
      );
    };
    update();
    window.addEventListener("resize", update);
    document.addEventListener("scroll", update, true);
    return () => {
      window.removeEventListener("resize", update);
      document.removeEventListener("scroll", update, true);
    };
  }, [menuOpen]);

  const run = async (
    action: () => Promise<{ ok: true; value: unknown } | { ok: false; error: BridgeError }>,
    titles: TaskToastTitles,
  ) => {
    if (busy) return;
    setBusy(true);
    const lifecycle = toast.pending(titles.pending);
    try {
      const result = await action();
      setBusy(false);
      if (result.ok) {
        lifecycle.success(titles.success);
        onChanged();
      } else {
        const failure = actionFailure(result.error, titles.failure);
        lifecycle.error(failure.title, failure.options);
      }
    } catch (error) {
      setBusy(false);
      const failure = actionFailure({ message: error instanceof Error ? error.message : "Unknown error" }, titles.failure);
      lifecycle.error(failure.title, failure.options);
    }
  };

  const sections: MenuAction[][] = [
    handoffable
      ? [
          {
            key: "handoff",
            label: "Move to another worker",
            disabled: busy,
            onSelect: () => {
              setMenuOpen(false);
              setHandoffOpen(true);
            },
          },
        ]
      : [],
    cancellable
      ? [
          {
            key: "cancel",
            label: "Stop",
            icon: <CancelIcon />,
            disabled: busy,
            onSelect: () => {
              setMenuOpen(false);
              setConfirmingCancel(true);
            },
          },
        ]
      : [],
    [
      {
        key: "archive",
        label: archived ? "Restore" : "Archive",
        icon: archived ? <RestoreIcon /> : <ArchiveIcon />,
        disabled: busy,
        onSelect: () => {
          setMenuOpen(false);
          void run(() => executeArchive(task.id, !archived), archiveTitles(task, archived));
        },
      },
      ...(canDeleteBranch
        ? [
            {
              key: "archive-delete-branch",
              label: "Archive and delete branch",
              icon: <ArchiveIcon />,
              destructive: true,
              disabled: busy,
              onSelect: () => {
                setMenuOpen(false);
                setConfirmingArchiveBranch(true);
              },
            },
          ]
        : []),
      ...(completable
        ? [
            {
              key: "complete",
              label: "Mark as completed",
              icon: <CheckIcon />,
              disabled: busy,
              onSelect: () => {
                setMenuOpen(false);
                void run(() => executeComplete(task.id), completeTitles(task));
              },
            },
          ]
        : []),
    ],
  ];

  return (
    <div className="task-detail-header-actions">
      <div className="task-action-menu" ref={triggerRef}>
        <button
          className="icon-button task-action-menu-trigger"
          type="button"
          aria-label="More actions"
          title="More actions"
          aria-expanded={menuOpen}
          onClick={() => setMenuOpen((value) => !value)}
        >
          <MoreIcon />
        </button>
      </div>
      {menuOpen && typeof document !== "undefined" &&
        createPortal(
          <div
            ref={panelRef}
            className="task-action-menu-portal"
            style={
              placement
                ? { top: placement.top, left: placement.left, maxHeight: placement.maxHeight }
                : { visibility: "hidden" }
            }
          >
            <MenuPanel sections={sections} onClose={() => setMenuOpen(false)} />
          </div>,
          document.body,
        )}
      <StopConfirmDialog
        open={confirmingCancel}
        onClose={() => setConfirmingCancel(false)}
        onConfirm={() => {
          setConfirmingCancel(false);
          void run(() => executeCancel(task.id), stopTitles(task));
        }}
        busy={busy}
      />
      {branch && (
        <ArchiveBranchDialog
          task={{ ...task, branch }}
          open={confirmingArchiveBranch}
          onClose={() => setConfirmingArchiveBranch(false)}
          onChanged={() => {
            setConfirmingArchiveBranch(false);
            onChanged();
          }}
        />
      )}
      <HandoffDialog
        task={task}
        open={handoffOpen}
        onClose={() => setHandoffOpen(false)}
        onChanged={() => {
          setHandoffOpen(false);
          onChanged();
        }}
      />
    </div>
  );
}

interface HandoffWorker {
  profile: ProfileView;
  settings: ModelSettingsSnapshot["workers"][number];
}

function HandoffDialog({
  task,
  open,
  onClose,
  onChanged,
}: {
  task: Task;
  open: boolean;
  onClose: () => void;
  onChanged: () => void;
}) {
  const [workers, setWorkers] = React.useState<HandoffWorker[]>([]);
  const [workerId, setWorkerId] = React.useState("");
  const [modelId, setModelId] = React.useState("");
  const [loading, setLoading] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [noOtherWorker, setNoOtherWorker] = React.useState(false);
  const [currentLabels, setCurrentLabels] = React.useState<{ profile?: string; model?: string }>({});

  React.useEffect(() => {
    if (!open) return;
    let disposed = false;
    setLoading(true);
    setNoOtherWorker(false);
    void Promise.all([
      broker.summary({ compact: true, limit: 1 }),
      broker.modelSettings(task.cwd),
    ]).then(([summary, settings]) => {
      if (disposed) return;
      if (!summary.ok) {
        toast.error("Couldn't load workers", { description: "Close this window and try again.", detail: summary.error.message });
        return;
      }
      if (!settings.ok) {
        toast.error("Couldn't load workers", { description: "Close this window and try again.", detail: settings.error.message });
        return;
      }
      const currentWorker = settings.value.workers.find((worker) => worker.id === task.profileId);
      const currentProfile = summary.value.profiles.find((candidate) => candidate.id === task.profileId);
      setCurrentLabels({
        profile: currentProfile?.label,
        model: currentWorker?.models.find((model) => model.id === task.model)?.label,
      });
      const available = settings.value.workers.flatMap((worker) => {
        const profile = summary.value.profiles.find((candidate) => candidate.id === worker.id);
        return profile?.enabled && worker.enabled && worker.models.some((model) => model.enabled)
          ? [{ profile, settings: worker }]
          : [];
      });
      setWorkers(available);
      const firstWorker = available.find(({ profile }) => profile.id !== task.profileId) ?? available[0];
      const firstModel = firstWorker?.settings.models.find((model) => model.enabled && (
        firstWorker.profile.id !== task.profileId || model.id !== task.model
      ));
      setWorkerId(firstWorker?.profile.id ?? "");
      setModelId(firstModel?.id ?? "");
      setNoOtherWorker(!firstWorker || !firstModel);
    }).finally(() => {
      if (!disposed) setLoading(false);
    });
    return () => {
      disposed = true;
    };
  }, [open, task.cwd, task.model, task.profileId]);

  const selectedWorker = workers.find(({ profile }) => profile.id === workerId);
  const models = selectedWorker?.settings.models.filter((model) => model.enabled) ?? [];
  const canSubmit = !loading && !busy && selectedWorker !== undefined && modelId !== "" && (
    workerId !== task.profileId || modelId !== task.model
  );

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!canSubmit) return;
    setBusy(true);
    const titles = moveTitles(task);
    const lifecycle = toast.pending(titles.pending);
    try {
      const result = await executeHandoff(task.id, workerId, modelId);
      setBusy(false);
      if (result.ok) {
        lifecycle.success(titles.success);
        onChanged();
      } else {
        const failure = actionFailure(result.error, titles.failure);
        lifecycle.error(failure.title, failure.options);
      }
    } catch (error) {
      setBusy(false);
      const failure = actionFailure({ message: error instanceof Error ? error.message : "Unknown error" }, titles.failure);
      lifecycle.error(failure.title, failure.options);
    }
  };

  return (
    <Modal open={open} onClose={onClose} labelledBy="handoff-dialog-title" className="modal-dialog-handoff">
      <form className="handoff-form" onSubmit={submit}>
        <h2 id="handoff-dialog-title">Move this task</h2>
        <p className="handoff-description">Keep this task&apos;s history while changing its worker.</p>
        {loading ? <p role="status">Loading workers…</p> : null}
        {!loading && noOtherWorker ? (
          <p className="handoff-description">No other worker is set up yet.</p>
        ) : null}
        {!loading && workers.length > 0 && !noOtherWorker ? (
          <>
            <label>
              Worker
              <select
                value={workerId}
                onChange={(event) => {
                  const nextWorker = workers.find(({ profile }) => profile.id === event.target.value);
                  setWorkerId(event.target.value);
                  setModelId(nextWorker?.settings.models.find((model) => model.enabled)?.id ?? "");
                }}
              >
                {workers.map(({ profile }) => <option key={profile.id} value={profile.id}>{profile.label}</option>)}
              </select>
            </label>
            <label>
              Model
              <select value={modelId} onChange={(event) => setModelId(event.target.value)}>
                {models.map((model) => <option key={model.id} value={model.id}>{model.label}</option>)}
              </select>
            </label>
            <p className="handoff-current">
              Current: {currentLabels.profile && currentLabels.model
                ? `${currentLabels.profile} / ${currentLabels.model}`
                : "Current worker"}
            </p>
          </>
        ) : null}
        <div className="handoff-actions">
          <button className="settings-button" type="button" onClick={onClose} disabled={busy}>Keep worker</button>
          <button className="settings-button settings-button-primary" type="submit" disabled={!canSubmit}>
            {busy ? "Moving…" : "Move task"}
          </button>
        </div>
      </form>
    </Modal>
  );
}

/**
 * What a task that has not started yet is waiting for, and the two ways out of
 * the wait: start it now, or stop waiting altogether.
 */
export function WaitNotice({ task, onChanged }: { task: Task; onChanged: () => void }) {
  const [busy, setBusy] = React.useState(false);
  const [confirmingCancel, setConfirmingCancel] = React.useState(false);
  if (!isExplainedWait(task.hold) || !task.hold) return null;
  const nextTry = nextTryLabel(task.hold);

  const run = async (
    action: () => Promise<{ ok: true; value: unknown } | { ok: false; error: BridgeError }>,
    titles: TaskToastTitles,
  ) => {
    if (busy) return;
    setBusy(true);
    const lifecycle = toast.pending(titles.pending);
    const result = await action();
    setBusy(false);
    if (result.ok) {
      lifecycle.success(titles.success);
      onChanged();
      return;
    }
    const failure = actionFailure(result.error, titles.failure);
    lifecycle.error(failure.title, failure.options);
  };

  return (
    <div className="waiting-task" role="status">
      <strong>{task.hold.note}</strong>
      {nextTry && <span className="waiting-task-next" title={nextTry.title}>{nextTry.label}</span>}
      <div className="waiting-task-actions">
        <button
          className="task-action task-action-primary"
          type="button"
          disabled={busy}
          onClick={() => void run(() => executeResume(task.id), resumeTitles(task))}
        >
          Continue now
        </button>
        <button
          className="task-action"
          type="button"
          disabled={busy}
          onClick={() => setConfirmingCancel(true)}
        >
          Stop task
        </button>
      </div>
      <StopConfirmDialog
        open={confirmingCancel}
        onClose={() => setConfirmingCancel(false)}
        onConfirm={() => {
          setConfirmingCancel(false);
          void run(() => executeCancel(task.id), stopTitles(task));
        }}
        busy={busy}
      />
    </div>
  );
}

/** The composer plus the blocked-task explanation, in the footer of the transcript. */
export function TaskControls({
  task,
  events,
  onChanged,
  thinkingToggle,
}: {
  task: Task;
  events: TaskEventView[];
  onChanged: () => void;
  thinkingToggle?: { active: boolean; onToggle: () => void };
}) {
  const [busy, setBusy] = React.useState(false);
  const routing = routingForState(task.state, false, task.question);
  const queued = task.queuedFollowUpItems ?? [];
  const pinnedQuestionRef = React.useRef<HTMLDivElement>(null);
  const pinnedQuestion = routing.type === "reply" ? routing.question : undefined;
  // The model's published window, read once per worker and model. Absent when
  // the catalog names none — the footer then shows no context read at all.
  const [contextWindow, setContextWindow] = React.useState<number | undefined>(undefined);
  React.useEffect(() => {
    let disposed = false;
    setContextWindow(undefined);
    void broker
      .modelSettings(task.worktree?.originCwd ?? task.cwd)
      .then((result) => {
        if (disposed || !result.ok) return;
        const window = result.value.workers
          .find((worker) => worker.id === task.profileId)
          ?.models.find((model) => model.id === task.model)?.contextWindow;
        setContextWindow(window);
      });
    return () => {
      disposed = true;
    };
  }, [task.worktree?.originCwd, task.cwd, task.profileId, task.model]);

  React.useEffect(() => {
    if (pinnedQuestion !== undefined) pinnedQuestionRef.current?.scrollIntoView({ block: "nearest" });
  }, [pinnedQuestion]);

  const run = async (
    action: () => Promise<{ ok: true; value: unknown } | { ok: false; error: BridgeError }>,
    titles: TaskToastTitles,
  ): Promise<boolean> => {
    if (busy) return false;
    setBusy(true);
    const lifecycle = toast.pending(titles.pending);
    try {
      const result = await action();
      setBusy(false);
      if (result.ok) {
        lifecycle.success(titles.success);
        onChanged();
        return true;
      }
      const failure = actionFailure(result.error, titles.failure);
      lifecycle.error(failure.title, failure.options);
      return false;
    } catch (error) {
      setBusy(false);
      const failure = actionFailure({ message: error instanceof Error ? error.message : "Unknown error" }, titles.failure);
      lifecycle.error(failure.title, failure.options);
      return false;
    }
  };

  const handleSend = (request: ComposerRequest): Promise<boolean> => {
    if (request.instruction === undefined) {
      if (isResume(routing)) return run(() => executeResume(task.id), resumeTitles(task));
      return Promise.resolve(true);
    }
    const instruction = request.instruction;
    if (routing.type === "reply") {
      return run(() => executeReply(task.id, instruction, task.scope), replyTitles(task));
    }
    if (
      (routing.type === "steer-and-queue" && request.mode === "steer") ||
      (routing.type === "steer" && request.mode === "steer")
    ) {
      return run(() => executeSteer(task.id, instruction), instructionTitles(task));
    }
    if ((routing.type === "steer-and-queue" && request.mode === "primary") || routing.type === "queue") {
      return run(() => executeQueue(task.id, instruction), followUpTitles(task));
    }
    if (routing.type === "resume") {
      return run(() => executeResume(task.id, { instruction, scope: task.scope }), resumeTitles(task));
    }
    if (routing.type === "steer" && request.mode === "primary") {
      return run(() => executeSteer(task.id, instruction), instructionTitles(task));
    }
    return Promise.resolve(true);
  };

  const removeQueued = (index: number) => void run(() => executeRemoveFollowUp(task.id, index), removeFollowUpTitles(task));
  const explanation = task.state === "blocked" || task.state === "failed"
    ? explainBlocked(task.completion, task.scope)
    : undefined;

  return (
    <section className="task-controls" aria-label="Task actions" aria-busy={busy}>
      {explanation && (
        <div className="blocked-task" role="status">
            <strong>{explanation.headline}</strong>
          {explanation.rawReason && (
            <details className="blocked-technical-detail">
              <summary>Technical detail</summary>
              <p>{explanation.rawReason}</p>
            </details>
          )}
          <div className="blocked-task-actions">
            <button className="task-action" type="button" disabled={busy} onClick={() => void run(() => executeComplete(task.id), completeTitles(task))}>
              Mark as completed
            </button>
            {explanation.suggestedScope && (
              <button
                className="task-action task-action-primary"
                type="button"
                disabled={busy}
                title={explanation.deniedPaths.length > 1 ? explanation.deniedPaths.join(", ") : undefined}
                onClick={() => void run(() => executeResume(task.id, { scope: explanation.suggestedScope }), resumeTitles(task))}
              >
                {explanation.deniedPaths.length === 0
                  ? "Continue with wider access"
                  : explanation.deniedPaths.length === 1
                  ? `Continue with access to ${explanation.deniedPaths[0]}`
                  : `Continue with access to ${explanation.deniedPaths[0]} and ${explanation.deniedPaths.length - 1} more`}
              </button>
            )}
            <button className="task-action" type="button" disabled={busy} onClick={() => void run(() => executeResume(task.id), resumeTitles(task))}>
              Continue as-is
            </button>
          </div>
        </div>
      )}
      {pinnedQuestion !== undefined && (
        <div className="pinned-question" ref={pinnedQuestionRef}>
          <p className="pinned-question-label">Waiting for your answer</p>
          <MarkdownContent source={pinnedQuestion} />
        </div>
      )}
      {routing.type !== "none" && (
        <ConversationComposer
          routing={routing}
          scope={task.scope}
          queued={queued}
          onSend={handleSend}
          onRemoveQueued={removeQueued}
          thinkingToggle={thinkingToggle}
        />
      )}
      {routing.type !== "none" && <TaskMetadata task={task} events={events} contextWindow={contextWindow} />}
    </section>
  );
}
