// One platform listener per event backs every current subscriber.

import { getTransport } from "./transport";
import {
  EVENT_BATCH_EVENT,
  MENU_EVENT,
  OPEN_TASK_EVENT,
  STATUS_EVENT,
  TASK_DELTA_EVENT,
  type MenuCommand,
  type EventBatch,
  type StreamStatus,
  type TaskDelta,
} from "./types";

type Unsubscribe = () => void;

class Feed<T> {
  private nextId = 0;
  private attached = false;
  private readonly handlers = new Map<number, (payload: T) => void>();

  constructor(private readonly event: string) {}

  subscribe(handle: (payload: T) => void): Unsubscribe {
    const id = this.nextId++;
    this.handlers.set(id, handle);
    if (!this.attached) {
      this.attached = true;
      getTransport().listen<T>(this.event, (payload) => this.broadcast(payload));
    }
    return () => {
      this.handlers.delete(id);
    };
  }

  /** Cloned first: a handler is free to subscribe or unsubscribe on its own. */
  private broadcast(payload: T): void {
    for (const handle of Array.from(this.handlers.values())) {
      handle(payload);
    }
  }

  get size(): number {
    return this.handlers.size;
  }

  reset(): void {
    this.nextId = 0;
    this.attached = false;
    this.handlers.clear();
  }
}

const batches = new Feed<EventBatch>(EVENT_BATCH_EVENT);
const statuses = new Feed<StreamStatus>(STATUS_EVENT);
const deltas = new Feed<TaskDelta>(TASK_DELTA_EVENT);
const menuCommands = new Feed<MenuCommand>(MENU_EVENT);
const openedTasks = new Feed<string>(OPEN_TASK_EVENT);

export function onEventBatch(handle: (batch: EventBatch) => void): Unsubscribe {
  return batches.subscribe(handle);
}

export function onBrokerStatus(handle: (status: StreamStatus) => void): Unsubscribe {
  return statuses.subscribe(handle);
}

export function onTaskDelta(handle: (delta: TaskDelta) => void): Unsubscribe {
  return deltas.subscribe(handle);
}

export function onMenuCommand(handle: (command: MenuCommand) => void): Unsubscribe {
  return menuCommands.subscribe(handle);
}

export function onOpenTask(handle: (taskId: string) => void): Unsubscribe {
  return openedTasks.subscribe(handle);
}

export function liveSubscriptions(): number {
  return batches.size + statuses.size + deltas.size + menuCommands.size + openedTasks.size;
}

export function resetFeedsForTests(): void {
  batches.reset();
  statuses.reset();
  deltas.reset();
  menuCommands.reset();
  openedTasks.reset();
}
