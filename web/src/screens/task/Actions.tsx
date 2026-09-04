// Task mutations, the header actions menu, and blocked-task explanations.
// Ported from rust/crates/oga-ui/src/actions/mod.rs — keep behavior and copy identical.

import * as React from "react";
import { broker } from "@/bridge/client";
import type {
  BridgeError,
  CompletionCode,
  ModelSettingsSnapshot,
  ProfileView,
  Task,
  TaskCompletion,
  TaskScope,
  TaskState,
} from "@/bridge/types";
import { MenuPanel, type MenuAction } from "@/components/menu/Menu";
import { Modal } from "@/components/primitives/Modal";
import { ArchiveIcon, CancelIcon, CheckIcon, MoreIcon, RestoreIcon } from "@/ui/icons";
import { MarkdownContent } from "@/domain/markdown";
import { ComposerRequest, ConversationComposer, isResume, routingForState } from "./Composer";
import { isUnattendedWait, nextTryLabel } from "./format";
import { TaskMetadata } from "./TaskMetadata";
import { toast } from "@/state/toast";

/** Any task-like value with just the state a menu needs to gate on. */
export type TaskLike = { state: TaskState; archivedAt?: string };

export function executeArchive(taskId: string, archived: boolean) {
  return broker.archiveTask(taskId, archived);
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
  return (["queued", "pending", "running", "needs_input", "blocked"] as string[]).includes(task.state);
}

export function canComplete(task: TaskLike): boolean {
  return (
    (["queued", "pending", "running", "needs_input", "answered", "blocked", "failed", "cancelled"] as string[]).includes(
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
  return task.state !== "completed";
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

export interface BlockedExplanation {
  headline: string;
  rawReason?: string;
  deniedPaths: string[];
  suggestedScope?: TaskScope;
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

/** The header's ellipsis menu: cancel, archive, mark completed. */
export function TaskHeaderActions({ task, onChanged }: { task: Task; onChanged: () => void }) {
  const [busy, setBusy] = React.useState(false);
  const [menuOpen, setMenuOpen] = React.useState(false);
  const [confirmingCancel, setConfirmingCancel] = React.useState(false);
  const [handoffOpen, setHandoffOpen] = React.useState(false);
  const menuRef = React.useRef<HTMLDivElement>(null);
  const cancellable = canCancel(task);
  const completable = canComplete(task) && task.state !== "blocked";
  const handoffable = canHandoff(task);
  const archived = task.archivedAt !== undefined;

  React.useEffect(() => {
    if (!menuOpen) return;
    const closeOnPointer = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (target && menuRef.current?.contains(target)) return;
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

  const run = async (
    action: () => Promise<{ ok: true; value: unknown } | { ok: false; error: BridgeError }>,
    pendingTitle: string,
    successTitle: string,
  ) => {
    if (busy) return;
    setBusy(true);
    const lifecycle = toast.pending(pendingTitle);
    try {
      const result = await action();
      setBusy(false);
      if (result.ok) {
        lifecycle.success(successTitle);
        onChanged();
      } else {
        const failure = actionFailure(result.error, "update this task");
        lifecycle.error(failure.title, failure.options);
      }
    } catch (error) {
      setBusy(false);
      const failure = actionFailure({ message: error instanceof Error ? error.message : "Unknown error" }, "update this task");
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
          void run(() => executeArchive(task.id, !archived), archived ? "Restoring task" : "Archiving task", archived ? "Task restored" : "Task archived");
        },
      },
      ...(completable
        ? [
            {
              key: "complete",
              label: "Mark as completed",
              icon: <CheckIcon />,
              disabled: busy,
              onSelect: () => {
                setMenuOpen(false);
                void run(() => executeComplete(task.id), "Completing task", "Task marked completed");
              },
            },
          ]
        : []),
    ],
  ];

  return (
    <div className="task-detail-header-actions">
      <div className="task-action-menu" ref={menuRef}>
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
        {menuOpen && (
          <div className="task-action-menu-items">
            <MenuPanel sections={sections} onClose={() => setMenuOpen(false)} />
          </div>
        )}
      </div>
      <Modal open={confirmingCancel} onClose={() => setConfirmingCancel(false)} labelledBy="cancel-dialog-title" className="modal-dialog-cancel">
        <h2 id="cancel-dialog-title">Stop this task?</h2>
        <p>The worker stops. You can resume it later.</p>
        <div className="handoff-actions">
          <button className="settings-button" type="button" onClick={() => setConfirmingCancel(false)}>
            Don&apos;t stop
          </button>
          <button
            className="settings-button settings-button-danger"
            type="button"
            onClick={() => {
              setConfirmingCancel(false);
              void run(() => executeCancel(task.id), "Stopping task", "Task stopped");
            }}
          >
            Stop task
          </button>
        </div>
      </Modal>
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
  const [currentLabels, setCurrentLabels] = React.useState({ profile: task.profileId, model: task.model });

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
        profile: currentProfile?.label ?? task.profileId,
        model: currentWorker?.models.find((model) => model.id === task.model)?.label ?? task.model,
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
    const lifecycle = toast.pending("Moving task");
    try {
      const result = await executeHandoff(task.id, workerId, modelId);
      setBusy(false);
      if (result.ok) {
        lifecycle.success("Task moved to another worker");
        onChanged();
      } else {
        const failure = actionFailure(result.error, "move this task");
        lifecycle.error(failure.title, failure.options);
      }
    } catch (error) {
      setBusy(false);
      const failure = actionFailure({ message: error instanceof Error ? error.message : "Unknown error" }, "move this task");
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
          <p className="handoff-description">This task has no other worker and model to move to.</p>
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
            <p className="handoff-current">Current: {currentLabels.profile} / {currentLabels.model}</p>
          </>
        ) : null}
        <div className="handoff-actions">
          <button className="settings-button" type="button" onClick={onClose}>Cancel</button>
          <button className="settings-button settings-button-primary" type="submit" disabled={!canSubmit}>
            {busy ? "Moving…" : "Move task"}
          </button>
        </div>
      </form>
    </Modal>
  );
}

/**
 * What a task that stopped for a reason nobody chose is waiting for, and the
 * two ways out of the wait: start it now, or stop waiting altogether.
 */
export function WaitNotice({ task, onChanged }: { task: Task; onChanged: () => void }) {
  const [busy, setBusy] = React.useState(false);
  if (!isUnattendedWait(task.hold) || !task.hold) return null;
  const nextTry = nextTryLabel(task.hold);

  const run = async (
    action: () => Promise<{ ok: true; value: unknown } | { ok: false; error: BridgeError }>,
    pendingTitle: string,
    successTitle: string,
  ) => {
    if (busy) return;
    setBusy(true);
    const lifecycle = toast.pending(pendingTitle);
    const result = await action();
    setBusy(false);
    if (result.ok) {
      lifecycle.success(successTitle);
      onChanged();
      return;
    }
    const failure = actionFailure(result.error, "update this task");
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
          onClick={() => void run(() => executeResume(task.id), "Resuming task", "Task resumed")}
        >
          Continue now
        </button>
        <button
          className="task-action"
          type="button"
          disabled={busy}
          onClick={() => void run(() => executeCancel(task.id), "Stopping task", "Task stopped")}
        >
          Stop task
        </button>
      </div>
    </div>
  );
}

/** The composer plus the blocked-task explanation, in the footer of the transcript. */
export function TaskControls({ task, onChanged }: { task: Task; onChanged: () => void }) {
  const [busy, setBusy] = React.useState(false);
  const routing = routingForState(task.state, false, task.question);
  const queued = task.queuedFollowUpItems ?? [];
  const pinnedQuestionRef = React.useRef<HTMLDivElement>(null);
  const pinnedQuestion = routing.type === "reply" ? routing.question : undefined;

  React.useEffect(() => {
    if (pinnedQuestion !== undefined) pinnedQuestionRef.current?.scrollIntoView({ block: "nearest" });
  }, [pinnedQuestion]);

  const run = async (
    action: () => Promise<{ ok: true; value: unknown } | { ok: false; error: BridgeError }>,
    pendingTitle: string,
    successTitle: string,
  ) => {
    if (busy) return;
    setBusy(true);
    const lifecycle = toast.pending(pendingTitle);
    try {
      const result = await action();
      setBusy(false);
      if (result.ok) {
        lifecycle.success(successTitle);
        onChanged();
      } else {
        const failure = actionFailure(result.error, "update this task");
        lifecycle.error(failure.title, failure.options);
      }
    } catch (error) {
      setBusy(false);
      const failure = actionFailure({ message: error instanceof Error ? error.message : "Unknown error" }, "update this task");
      lifecycle.error(failure.title, failure.options);
    }
  };

  const handleSend = (request: ComposerRequest) => {
    if (request.instruction === undefined) {
       if (isResume(routing)) void run(() => executeResume(task.id), "Resuming task", "Task resumed");
      return;
    }
    const instruction = request.instruction;
    if (routing.type === "reply") {
      void run(() => executeReply(task.id, instruction, task.scope), "Sending reply", "Reply sent");
      return;
    }
    if (
      (routing.type === "steer-and-queue" && request.mode === "steer") ||
      (routing.type === "steer" && request.mode === "steer")
    ) {
      void run(() => executeSteer(task.id, instruction), "Sending instruction", "Instruction sent");
      return;
    }
    if ((routing.type === "steer-and-queue" && request.mode === "primary") || routing.type === "queue") {
      void run(() => executeQueue(task.id, instruction), "Queueing follow-up", "Follow-up queued");
      return;
    }
    if (routing.type === "resume") {
      void run(() => executeResume(task.id, { instruction, scope: task.scope }), "Resuming task", "Task resumed");
      return;
    }
    if (routing.type === "steer" && request.mode === "primary") {
      void run(() => executeSteer(task.id, instruction), "Sending instruction", "Instruction sent");
    }
  };

  const removeQueued = (index: number) => void run(() => executeRemoveFollowUp(task.id, index), "Removing follow-up", "Follow-up removed");
  const explanation = task.state === "blocked" || task.state === "failed"
    ? explainBlocked(task.completion, task.scope)
    : undefined;

  return (
    <section className="task-controls" aria-label="Task actions" aria-busy={busy}>
      {explanation && (
        <div className="blocked-task" role="status">
            <strong>{task.state === "blocked" ? "Blocked: " : "Failed: "}{explanation.headline}</strong>
          {explanation.rawReason && (
            <details className="blocked-technical-detail">
              <summary>Technical detail</summary>
              <p>{explanation.rawReason}</p>
            </details>
          )}
          <div className="blocked-task-actions">
            <button className="task-action" type="button" disabled={busy} onClick={() => void run(() => executeComplete(task.id), "Completing task", "Task marked completed")}>
              Mark as completed
            </button>
            {explanation.suggestedScope && (
              <button
                className="task-action task-action-primary"
                type="button"
                onClick={() => void run(() => executeResume(task.id, { scope: explanation.suggestedScope }), "Resuming task", "Task resumed")}
              >
                {explanation.deniedPaths.length > 0
                  ? `Continue with access to ${explanation.deniedPaths.join(", ")}`
                  : "Continue with wider access"}
              </button>
            )}
            <button className="task-action" type="button" onClick={() => void run(() => executeResume(task.id), "Resuming task", "Task resumed")}>
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
        <ConversationComposer routing={routing} scope={task.scope} queued={queued} onSend={handleSend} onRemoveQueued={removeQueued} />
      )}
      {routing.type !== "none" && <TaskMetadata task={task} />}
    </section>
  );
}
