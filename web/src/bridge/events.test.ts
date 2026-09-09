import { describe, expect, it, mock } from "bun:test";
import type { Transport } from "./transport";
import { EVENT_BATCH_EVENT, STATUS_EVENT, TASK_DELTA_EVENT, type EventBatch } from "./types";

function fakeTransport() {
  const listeners = new Map<string, Array<(payload: unknown) => void>>();
  const transport: Transport = {
    invoke: mock(),
    listen: mock((event, handle) => {
      const bucket = listeners.get(event) ?? [];
      bucket.push(handle as (payload: unknown) => void);
      listeners.set(event, bucket);
    }),
  };
  return {
    transport,
    emit(event: string, payload: unknown) {
      for (const handle of listeners.get(event) ?? []) handle(payload);
    },
  };
}

// Each Feed is a module-level singleton that attaches to whatever transport
// is active the first time it gets a subscriber, so every test needs its own
// fresh module instance (transport included) rather than sharing state
// through the import cache.
async function freshEvents(transport: Transport) {
  const { resetFeedsForTests } = await import("./events");
  resetFeedsForTests();
  const { setTransport } = await import("./transport");
  setTransport(transport);
  return import("./events");
}

describe("event subscription fan-out", () => {
  it("attaches one platform listener no matter how many subscribers join", async () => {
    const fake = fakeTransport();
    const { onEventBatch } = await freshEvents(fake.transport);

    onEventBatch(() => {});
    onEventBatch(() => {});
    onEventBatch(() => {});

    expect(fake.transport.listen).toHaveBeenCalledTimes(1);
    expect(fake.transport.listen).toHaveBeenCalledWith(EVENT_BATCH_EVENT, expect.any(Function));
  });

  it("reaches every live subscriber when the shell forwards an event", async () => {
    const fake = fakeTransport();
    const { onEventBatch } = await freshEvents(fake.transport);

    const seen: EventBatch[] = [];
    onEventBatch((batch) => seen.push(batch));
    onEventBatch((batch) => seen.push(batch));

    fake.emit(EVENT_BATCH_EVENT, { cursor: 1, streamFloor: 0, stale: false, pointers: [] });

    expect(seen).toHaveLength(2);
  });

  it("stops reaching a subscriber once it unsubscribes", async () => {
    const fake = fakeTransport();
    const { onTaskDelta } = await freshEvents(fake.transport);

    const runs = mock();
    const unsubscribe = onTaskDelta(runs);
    unsubscribe();
    fake.emit(TASK_DELTA_EVENT, { taskId: "t", fromCursor: 0, cursor: 1, events: [] });

    expect(runs).not.toHaveBeenCalled();
  });

  it("costs nothing for subscribers that already left", async () => {
    const fake = fakeTransport();
    const { onEventBatch } = await freshEvents(fake.transport);

    for (let i = 0; i < 10; i++) {
      const unsubscribe = onEventBatch(() => {});
      unsubscribe();
    }
    const runs = mock();
    onEventBatch(runs);

    fake.emit(EVENT_BATCH_EVENT, { cursor: 1, streamFloor: 0, stale: false, pointers: [] });

    expect(runs).toHaveBeenCalledTimes(1);
  });

  it("keeps each event's feed independent", async () => {
    const fake = fakeTransport();
    const { onBrokerStatus, onMenuCommand } = await freshEvents(fake.transport);

    const status = mock();
    const menu = mock();
    onBrokerStatus(status);
    onMenuCommand(menu);

    fake.emit(STATUS_EVENT, { connected: true, cursor: 0, streamFloor: 0, stale: false });

    expect(status).toHaveBeenCalledTimes(1);
    expect(menu).not.toHaveBeenCalled();
  });

  it("tracks live subscription count across feeds", async () => {
    const fake = fakeTransport();
    const { onEventBatch, onBrokerStatus, liveSubscriptions } = await freshEvents(fake.transport);

    expect(liveSubscriptions()).toBe(0);
    const unsubscribeFrame = onEventBatch(() => {});
    const unsubscribeStatus = onBrokerStatus(() => {});
    expect(liveSubscriptions()).toBe(2);

    unsubscribeFrame();
    unsubscribeStatus();
    expect(liveSubscriptions()).toBe(0);
  });
});
