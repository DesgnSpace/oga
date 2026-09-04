import { describe, expect, it } from "bun:test";
import { ToastStore } from "./toast";

class TestClock {
  time = 0;
  private nextId = 1;
  private timers = new Map<number, { at: number; callback: () => void }>();

  now = () => this.time;

  setTimeout = (callback: () => void, delay: number) => {
    const id = this.nextId++;
    this.timers.set(id, { at: this.time + delay, callback });
    return id as unknown as ReturnType<typeof setTimeout>;
  };

  clearTimeout = (timer: ReturnType<typeof setTimeout>) => {
    this.timers.delete(timer as unknown as number);
  };

  advance(ms: number): void {
    this.time += ms;
    for (const [id, timer] of [...this.timers]) {
      if (timer.at <= this.time) {
        this.timers.delete(id);
        timer.callback();
      }
    }
  }
}

function createStore() {
  const clock = new TestClock();
  return { clock, store: new ToastStore(clock) };
}

describe("toast store", () => {
  it("auto-dismisses success and info after four seconds, but not errors or pending work", () => {
    const { clock, store } = createStore();
    store.success("Saved");
    store.info("Updated");
    store.error("Failed");
    store.pending("Saving");

    clock.advance(3_999);
    expect(store.snapshot.every((toast) => toast.phase === "visible")).toBe(true);
    clock.advance(1);

    expect(store.snapshot.filter((toast) => toast.kind === "success" || toast.kind === "info")).toHaveLength(0);
    expect(store.snapshot.filter((toast) => toast.kind === "error" || toast.kind === "pending")).toHaveLength(2);
  });

  it("caps the stack at five and drops the oldest toast", () => {
    const { store } = createStore();
    for (let index = 1; index <= 6; index += 1) store.info(`Toast ${index}`);

    expect(store.snapshot).toHaveLength(5);
    expect(store.snapshot.map((toast) => toast.title)).toEqual(["Toast 2", "Toast 3", "Toast 4", "Toast 5", "Toast 6"]);
  });

  it("pauses without restarting the remaining countdown", () => {
    const { clock, store } = createStore();
    store.success("Saved");
    clock.advance(1_500);
    store.pause();
    clock.advance(10_000);
    store.resume();
    clock.advance(2_499);
    expect(store.snapshot[0]?.phase).toBe("visible");
    clock.advance(1);
    expect(store.snapshot).toHaveLength(0);
  });

  it("resolves pending work in place", () => {
    const { clock, store } = createStore();
    const handle = store.pending("Saving");
    const id = store.snapshot[0]?.id;
    handle.success("Saved", { description: "Your changes are ready." });

    expect(store.snapshot[0]).toMatchObject({ id, kind: "success", title: "Saved", description: "Your changes are ready." });
    clock.advance(3_999);
    expect(store.snapshot[0]?.phase).toBe("visible");
    clock.advance(1);
    expect(store.snapshot).toHaveLength(0);
  });
});
