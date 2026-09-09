// Ported from rust/crates/oga-ui/src/task_detail/mod.rs `#[cfg(test)] mod tests`.

import { describe, expect, it } from "bun:test";
import type { Task, TaskDelta, TaskEventPage, TaskEventView, TaskSnapshot } from "@/bridge/types";
import {
  absorbPage,
  adopt,
  applyConnection,
  applyDelta,
  defaultTaskDetailState,
  mergeEvents,
  type TaskDetailState,
} from "./state";

function event(id: number): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.file",
    kind: "file",
    phase: "completed",
    title: `Read file ${id}`,
    createdAt: "2026-07-30T15:00:00Z",
  };
}

function task(state: Task["state"] = "running"): Task {
  return {
    id: "task",
    profileId: "worker",
    model: "sonnet",
    prompt: "do the thing",
    cwd: "/repo",
    state,
    createdAt: "2026-07-30T15:00:00Z",
    updatedAt: "2026-07-30T15:00:00Z",
    output: "",
    scope: { read: [], write: [] },
    allowQuestions: true,
  canDelegate: false,
  };
}

function delta(fromCursor: number, ids: number[]): TaskDelta {
  const events = ids.map(event);
  return {
    taskId: "task",
    fromCursor,
    cursor: events.length > 0 ? events[events.length - 1].id : fromCursor,
    events,
  };
}

/** A state/events pair holding `ids`, as if the shell had answered a watch. */
function loaded(ids: number[]): { state: TaskDetailState; events: TaskEventView[] } {
  const events = ids.map(event);
  const snapshot: TaskSnapshot = {
    task: task(),
    events,
    cursor: events.length > 0 ? events[events.length - 1].id : 0,
    oldestId: events.length > 0 ? events[0].id : undefined,
    hasEarlier: false,
  };
  const result = adopt([], defaultTaskDetailState(), snapshot);
  return { state: result.state, events: result.events };
}

function heldIds(events: TaskEventView[]): number[] {
  return events.map((e) => e.id);
}

describe("absorbing a page", () => {
  it("folds in without duplicating what is held", () => {
    let events: TaskEventView[] = [];
    let state = defaultTaskDetailState();

    const first: TaskEventPage = {
      events: [event(1), event(2)],
      cursor: 2,
      oldestId: 1,
      hasEarlier: true,
    };
    ({ events, state } = absorbPage(events, state, first));

    const second: TaskEventPage = {
      events: [event(2), event(3)],
      cursor: 12,
      oldestId: 2,
      hasEarlier: false,
    };
    ({ events, state } = absorbPage(events, state, second));

    expect(state.cursor).toBe(12);
    expect(state.oldestId).toBe(1);
    expect(state.hasEarlier).toBe(false);
    expect(heldIds(events)).toEqual([1, 2, 3]);
  });
});

describe("delta application", () => {
  // The whole point of the delta: what an update costs is what arrived, not
  // what the task has been through. A hundred events already held must not
  // be touched to add one more.
  it("an in-order delta folds an append in, touching only what arrived", () => {
    let { state, events } = loaded(Array.from({ length: 200 }, (_, i) => i + 1));

    for (let id = 201; id <= 300; id++) {
      const result = applyDelta(events, state, "task", delta(id - 1, [id]));
      expect(result.outcome).toBe("applied");
      events = result.events;
      state = result.state;
    }

    expect(heldIds(events)).toHaveLength(300);
    expect(state.cursor).toBe(300);
  });

  it("an out-of-order delta is merged by id rather than appended", () => {
    const { state, events } = loaded([1, 2, 3, 4, 5]);

    // Two events that continue from the held cursor, but arrive with the
    // higher id first — the append fast path must not apply, and the merge
    // must still land them in id order.
    const result = applyDelta(events, state, "task", {
      taskId: "task",
      fromCursor: 5,
      cursor: 7,
      events: [event(7), event(6)],
    });

    expect(result.outcome).toBe("applied");
    expect(heldIds(result.events)).toEqual([1, 2, 3, 4, 5, 6, 7]);
    expect(result.state.cursor).toBe(7);
  });

  it("an update already held changes nothing", () => {
    const { state, events } = loaded([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);

    const result = applyDelta(events, state, "task", delta(5, [6, 7]));

    expect(result.outcome).toBe("ignored");
    expect(result.state).toEqual(state);
    expect(heldIds(result.events)).toHaveLength(10);
  });

  it("an update for another task changes nothing", () => {
    const { state, events } = loaded([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    const stray = { ...delta(10, [11]), taskId: "other" };

    const result = applyDelta(events, state, "task", stray);

    expect(result.outcome).toBe("ignored");
    expect(heldIds(result.events)).toHaveLength(10);
  });

  // An update that does not continue from the cursor held means a frame went
  // missing. Applying it anyway would leave a hole nothing repairs — the
  // view has to ask for the task again instead.
  it("a delta that skips ahead forces a refetch (gap)", () => {
    const { state, events } = loaded([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);

    const result = applyDelta(events, state, "task", delta(40, [41]));

    expect(result.outcome).toBe("gap");
    expect(heldIds(result.events)).toHaveLength(10);
    expect(result.state.cursor).toBe(10);
  });

  it("an update before the task is loaded changes nothing", () => {
    const state = defaultTaskDetailState();

    const result = applyDelta([], state, "task", delta(0, [1]));

    expect(result.outcome).toBe("ignored");
    expect(result.events).toHaveLength(0);
  });
});

describe("reading the task again after a gap or resync", () => {
  it("keeps activity that joins up with what arrived", () => {
    const { state, events } = loaded([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);

    const result = adopt(events, state, {
      task: task(),
      events: [event(11), event(12)],
      cursor: 12,
      oldestId: 11,
      hasEarlier: true,
    });

    expect(heldIds(result.events)).toHaveLength(12);
    expect(result.state.cursor).toBe(12);
  });

  it("drops activity that does not join up, rather than leaving a hole", () => {
    const { state, events } = loaded([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);

    const result = adopt(events, state, {
      task: task(),
      events: [event(400), event(401)],
      cursor: 401,
      oldestId: 400,
      hasEarlier: true,
    });

    expect(heldIds(result.events)).toEqual([400, 401]);
    expect(result.state.hasEarlier).toBe(true);
  });

  it("does not move the cursor backward when an older snapshot returns late", () => {
    const { state, events } = loaded([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);

    const result = adopt(events, state, {
      task: task(),
      events: [event(1)],
      cursor: 9,
      oldestId: 1,
      hasEarlier: false,
    });

    expect(result.state.cursor).toBe(10);
    expect(heldIds(result.events)).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
  });
});

describe("connection state", () => {
  it("a dropped shell stream stops claiming live updates", () => {
    const [state, returning] = applyConnection(defaultTaskDetailState(), {
      connected: false,
      cursor: 0,
      streamFloor: 0,
      stale: false,
      error: "broker restarting",
    });

    expect(returning).toBe(false);
    expect(state.connection).toBe("offline");
    expect(state.error).toBe("broker restarting");
  });

  // A stream that comes back may have missed frames while it was away, so
  // the view reads the task again rather than trusting what it holds.
  it("a stream that comes back reports returning so the view resyncs", () => {
    const [offline] = applyConnection(defaultTaskDetailState(), {
      connected: false,
      cursor: 0,
      streamFloor: 0,
      stale: false,
    });

    const [live, returning] = applyConnection(offline, {
      connected: true,
      cursor: 9,
      streamFloor: 0,
      stale: false,
    });

    expect(returning).toBe(true);
    expect(live.connection).toBe("live");

    // A stream that never dropped does not keep asking.
    const [, returningAgain] = applyConnection(live, {
      connected: true,
      cursor: 10,
      streamFloor: 0,
      stale: false,
    });
    expect(returningAgain).toBe(false);
  });

  // Matches the Rust controller exactly: `streamFloor`/`stale` are carried
  // on the wire type but not consulted here, so a stale cursor below the
  // stream floor is not a separate branch — it resyncs through the same
  // "connection returned" path as any other reconnect.
  it("a stale cursor below the stream floor still resyncs via the reconnect path, not a special case", () => {
    const [offline] = applyConnection(defaultTaskDetailState(), {
      connected: false,
      cursor: 0,
      streamFloor: 0,
      stale: false,
    });

    const [live, returning] = applyConnection(offline, {
      connected: true,
      cursor: 50,
      streamFloor: 41,
      stale: true,
    });

    expect(returning).toBe(true);
    expect(live.connection).toBe("live");
    expect(live.error).toBeUndefined();
  });
});

describe("merging events", () => {
  it("keeps ordering stable when arriving activity is a mixed page", () => {
    const held = [event(1), event(3)];
    const merged = mergeEvents(held, [event(2), event(3)]);
    expect(heldIds(merged)).toEqual([1, 2, 3]);
  });
});
