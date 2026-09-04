// Fan-out for the events the shell pushes.
//
// One platform listener per event backs every subscriber that wants it. The
// cost of a pushed event is the subscribers currently mounted, not every
// subscriber a session has ever had.

import { getTransport } from "./transport";
import {
  EVENT_BATCH_EVENT,
  MENU_EVENT,
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

/** Runs `handle` for each paced set of broker changes. */
export function onEventBatch(handle: (batch: EventBatch) => void): Unsubscribe {
  return batches.subscribe(handle);
}

/** Runs `handle` whenever the shell's broker connection changes state. */
export function onBrokerStatus(handle: (status: StreamStatus) => void): Unsubscribe {
  return statuses.subscribe(handle);
}

/** Runs `handle` for every update the shell pushes about the task it is
 * watching. */
export function onTaskDelta(handle: (delta: TaskDelta) => void): Unsubscribe {
  return deltas.subscribe(handle);
}

/** Runs `handle` for every native menu command the shell emits. */
export function onMenuCommand(handle: (command: MenuCommand) => void): Unsubscribe {
  return menuCommands.subscribe(handle);
}

/** How many subscribers a pushed event currently reaches. Test seam only. */
export function liveSubscriptions(): number {
  return batches.size + statuses.size + deltas.size + menuCommands.size;
}

/** Clears feed state between tests that replace the active transport. */
export function resetFeedsForTests(): void {
  batches.reset();
  statuses.reset();
  deltas.reset();
  menuCommands.reset();
}
