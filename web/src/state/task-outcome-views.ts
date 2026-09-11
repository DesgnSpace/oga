import type { TaskState, TaskSummary } from "@/bridge/types";
import { readStorage, writeStorage } from "./storage";

const TASK_OUTCOME_VIEWS_KEY = "taskOutcomeViews";
export const MAX_TASK_OUTCOME_VIEWS = 512;

type TaskOutcomeSource = Pick<TaskSummary, "id" | "state" | "question" | "error" | "completion" | "hold">;

interface StoredTaskOutcomeView {
  outcome: string;
  viewed: boolean;
  touchedAt: number;
}

interface SerializedTaskOutcomeView extends StoredTaskOutcomeView {
  id: string;
}

export type TaskDotTone = "success" | "danger" | "info" | "warning" | "muted";

export function taskOutcomeKey(task: TaskOutcomeSource): string {
  switch (task.state) {
    case "needs_input":
      return JSON.stringify([task.state, task.question ?? ""]);
    case "failed":
      return JSON.stringify([
        task.state,
        task.error ?? "",
        task.completion?.code ?? "",
        task.completion?.reason ?? "",
      ]);
    case "completed":
      return JSON.stringify([task.state, task.completion?.code ?? ""]);
    default:
      return task.state;
  }
}

export function taskDotTone(state: TaskState): TaskDotTone {
  switch (state) {
    case "completed":
      return "success";
    case "failed":
      return "danger";
    case "needs_input":
      return "info";
    case "pending":
    case "blocked":
      return "warning";
    case "queued":
    case "preparing_checkout":
    case "removing_checkout":
    case "running":
    case "answered":
    case "cancelled":
      return "muted";
  }
}

export function isTaskWaiting(task: TaskOutcomeSource): boolean {
  if (task.state === "pending") return task.hold !== undefined;
  return task.state === "blocked" && task.completion?.dependencyBlocked === true;
}

function isSerializedTaskOutcomeView(value: unknown): value is SerializedTaskOutcomeView { // oxlint-disable-line anti-slop/no-unknown-parameters -- localStorage JSON is untrusted input
  // oxlint-disable-next-line anti-slop/no-runtime-typeof -- persisted JSON must be narrowed at this boundary
  if (typeof value !== "object" || value === null) return false;
  // SAFETY: the object check above establishes that this persisted JSON value can be read as a record.
  const entry = value as Partial<SerializedTaskOutcomeView>;
  // oxlint-disable-next-line anti-slop/no-runtime-typeof -- persisted JSON field validation
  return typeof entry.id === "string" &&
    // oxlint-disable-next-line anti-slop/no-runtime-typeof -- persisted JSON field validation
    typeof entry.outcome === "string" &&
    // oxlint-disable-next-line anti-slop/no-runtime-typeof -- persisted JSON field validation
    typeof entry.viewed === "boolean" &&
    // oxlint-disable-next-line anti-slop/no-runtime-typeof -- persisted JSON field validation
    typeof entry.touchedAt === "number" &&
    Number.isFinite(entry.touchedAt);
}

function loadTaskOutcomeViews(): Map<string, StoredTaskOutcomeView> {
  const raw = readStorage(TASK_OUTCOME_VIEWS_KEY);
  if (raw === undefined) return new Map();
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return new Map();
    return new Map(
      parsed
        .filter(isSerializedTaskOutcomeView)
        .map(({ id, outcome, viewed, touchedAt }) => [id, { outcome, viewed, touchedAt }]),
    );
  } catch {
    return new Map();
  }
}

function boundedEntries(entries: Map<string, StoredTaskOutcomeView>): Map<string, StoredTaskOutcomeView> {
  if (entries.size <= MAX_TASK_OUTCOME_VIEWS) return entries;
  return new Map(
    [...entries.entries()]
      .sort(([, left], [, right]) => right.touchedAt - left.touchedAt)
      .slice(0, MAX_TASK_OUTCOME_VIEWS),
  );
}

export class TaskOutcomeViewStore {
  private entries = boundedEntries(loadTaskOutcomeViews());
  private activeTaskId: string | undefined;
  private pinnedTaskId: string | undefined;
  private pinnedOutcome: string | undefined;
  private activeNeedsDecision = false;
  private version = 0;
  private readonly listeners = new Set<() => void>();

  get snapshot(): number {
    return this.version;
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  isViewed(task: TaskOutcomeSource): boolean {
    if (this.activeTaskId === task.id) return true;
    return this.storedViewed(task);
  }

  /**
   * Whether a task counts as unread for list ordering. Opening an outcome
   * marks its dot read at once, but the row keeps its unread position until
   * the open task changes, so the selected row never moves underneath the
   * pointer. Genuinely new unread outcomes still order first immediately.
   */
  isOrderingUnread(task: TaskOutcomeSource): boolean {
    const outcome = taskOutcomeKey(task);
    if (this.pinnedTaskId === task.id && this.pinnedOutcome === outcome) return true;
    if (task.id === this.activeTaskId) {
      if (this.activeNeedsDecision) return !this.storedViewed(task);
      return false;
    }
    return !this.isViewed(task);
  }

  observeTasks(tasks: readonly TaskOutcomeSource[]): void {
    let changed = false;
    for (const task of tasks) {
      if (this.observe(task, this.activeTaskId === task.id)) changed = true;
    }
    if (changed) this.commit();
  }

  markViewed(task: TaskOutcomeSource): void {
    if (this.observe(task, true)) this.commit();
  }

  setActiveTask(taskId: string | undefined): void {
    if (this.activeTaskId === taskId) return;
    this.activeTaskId = taskId;
    this.pinnedTaskId = undefined;
    this.pinnedOutcome = undefined;
    this.activeNeedsDecision = taskId !== undefined;
    this.notify();
  }

  clearActiveTask(taskId: string): void {
    if (this.activeTaskId !== taskId) return;
    this.activeTaskId = undefined;
    this.pinnedTaskId = undefined;
    this.pinnedOutcome = undefined;
    this.activeNeedsDecision = false;
    this.notify();
  }

  resetForTests(): void {
    this.entries = new Map();
    this.activeTaskId = undefined;
    this.pinnedTaskId = undefined;
    this.pinnedOutcome = undefined;
    this.activeNeedsDecision = false;
    this.version = 0;
    writeStorage(TASK_OUTCOME_VIEWS_KEY, "[]");
    this.notify();
  }

  private storedViewed(task: TaskOutcomeSource): boolean {
    const entry = this.entries.get(task.id);
    return entry?.outcome === taskOutcomeKey(task) && entry.viewed;
  }

  private observe(task: TaskOutcomeSource, viewed: boolean): boolean {
    const outcome = taskOutcomeKey(task);
    const current = this.entries.get(task.id);
    if (task.id === this.activeTaskId && viewed && this.activeNeedsDecision) {
      const wasUnread = current?.outcome !== outcome || !current.viewed;
      if (wasUnread) {
        this.pinnedTaskId = task.id;
        this.pinnedOutcome = outcome;
      }
      this.activeNeedsDecision = false;
    }
    if (current?.outcome === outcome && (!viewed || current.viewed)) return false;
    this.entries.set(task.id, {
      outcome,
      viewed: current?.outcome === outcome ? current.viewed || viewed : viewed,
      touchedAt: Date.now(),
    });
    return true;
  }

  private commit(): void {
    this.entries = boundedEntries(this.entries);
    const serialized: SerializedTaskOutcomeView[] = [...this.entries].map(([id, entry]) => ({ id, ...entry }));
    writeStorage(TASK_OUTCOME_VIEWS_KEY, JSON.stringify(serialized));
    this.notify();
  }

  private notify(): void {
    this.version += 1;
    for (const listener of this.listeners) listener();
  }
}

export const taskOutcomeViews = new TaskOutcomeViewStore();

export function resetTaskOutcomeViewsForTests(): void {
  taskOutcomeViews.resetForTests();
}
