import { StrictMode } from "react";
import { afterEach, describe, expect, it } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { resetFeedsForTests } from "@/bridge/events";
import { setTransport, type Transport } from "@/bridge/transport";
import { EVENT_BATCH_EVENT, type EventBatch, type TaskState } from "@/bridge/types";
import { resetTaskOutcomeViewsForTests } from "@/state/task-outcome-views";
import { AppShell } from "./AppShell";

function summaryTask(id: string, preview: string) {
  return { id, profileId: "worker", model: "sonnet", cwd: "/repo", state: id === "three" ? "preparing_checkout" : "running",
    promptPreview: preview, createdAt: "2026-08-31T10:00:00Z", updatedAt: "2026-08-31T10:00:00Z" };
}

function fullTask(id: string, prompt: string) {
  return { id, profileId: "worker", model: "sonnet", prompt, cwd: "/repo", state: id === "three" ? "preparing_checkout" : "running",
    createdAt: "2026-08-31T10:00:00Z", updatedAt: "2026-08-31T10:00:00Z", output: "",
    scope: { read: [], write: [] } };
}

const PROFILES = [{ id: "worker", label: "Worker", provider: "claude", model: "sonnet", enabled: true, env: {}, capabilities: [] }];

/** Reads rows from the list alone, since the preview repeats the picked task's title. */
async function listRow(text: string) {
  const list = await waitFor(() => {
    const element = document.getElementById("task-list");
    if (!element) throw new Error("the task list has not rendered yet");
    return element;
  });
  return (await within(list).findByText(text)).closest("button")!;
}

async function openFromList(text: string) {
  fireEvent.doubleClick(await listRow(text));
}

/** The task screen's only text box; search is a search box. */
function replyBox() {
  return screen.getByRole("textbox");
}

function showRecent() {
  fireEvent.click(within(screen.getByRole("navigation", { name: "Task views" })).getByRole("button", { name: "Recent" }));
}

const transport: Transport = {
  invoke: async (command, args) => {
    if (command === "broker_watch_task") {
      // SAFETY: broker_watch_task requests in this fixture always include a string taskId.
      const id = args?.taskId as string;
      // SAFETY: this fixture returns the broker_watch_task response shape expected by TaskDetail.
      return { task: fullTask(id, `prompt for ${id}`), events: [], cursor: 0, hasEarlier: false } as never;
    }
    if (command === "broker_stream_status") {
      // SAFETY: this fixture returns the broker_stream_status response shape expected by SidebarController.
      return { connected: true, cursor: 0, streamFloor: 0, stale: false } as never;
    }
    if (command !== "broker_call") throw { message: `no handler for ${command}` };
    // SAFETY: broker_call requests in this fixture always include a typed call object.
    const call = args?.call as { call: string; taskId?: string };
    if (call.call === "summary") {
      // SAFETY: this fixture returns the summary response shape expected by SidebarController.
      return { profiles: PROFILES, tasks: [summaryTask("one", "first task"), summaryTask("two", "second task"), summaryTask("three", "third task")],
        tasksHasMore: false, profileFailures: [], grants: [], memoryProjects: [] } as never;
    }
    if (call.call === "task") {
      // SAFETY: task calls in this fixture always include taskId.
      return fullTask(call.taskId!, `prompt for ${call.taskId}`) as never;
    }
    if (call.call === "taskEvents") {
      // SAFETY: this fixture returns the taskEvents response shape expected by TaskDetail.
      return { events: [], hasEarlier: false } as never;
    }
    throw { message: `no handler for ${call.call}` };
  },
  listen: () => {},
};

describe("the app shell", () => {
  afterEach(cleanup);

  it("opens the task a list row opens, with its reply box focused", async () => {
    setTransport(transport);
    window.history.replaceState(null, "", "/");

    render(
      <StrictMode>
        <AppShell />
      </StrictMode>,
    );

    await openFromList("second task");

    await waitFor(() => expect(window.location.pathname).toBe("/tasks/two"));
    await waitFor(() => expect(screen.getAllByText(/prompt for two/).length).toBeGreaterThan(0), { timeout: 5000 });
    await waitFor(() => expect(document.activeElement).toBe(replyBox()));
  });

  it("focuses the reply box of each task opened from the list, the same one again included", async () => {
    setTransport(transport);
    window.history.replaceState(null, "", "/");
    render(<AppShell />);

    await openFromList("first task");
    const firstReply = await screen.findByRole("textbox");
    await waitFor(() => expect(document.activeElement).toBe(firstReply));

    showRecent();
    await openFromList("second task");
    await waitFor(() => expect(window.location.pathname).toBe("/tasks/two"));
    await waitFor(() => expect(document.activeElement).toBe(replyBox()));

    showRecent();
    await openFromList("second task");
    await waitFor(() => expect(document.activeElement).toBe(replyBox()));
  });

  it("does not focus the reply box when a task is opened from its URL or browser history", async () => {
    setTransport(transport);
    window.history.replaceState(null, "", "/tasks/one");
    render(<AppShell />);

    const reply = await screen.findByRole("textbox");
    expect(document.activeElement).not.toBe(reply);

    showRecent();
    await openFromList("second task");
    await waitFor(() => expect(document.activeElement).toBe(replyBox()));
    replyBox().blur();
    window.history.back();
    await waitFor(() => expect(window.location.pathname).toBe("/"));
    window.history.back();
    await waitFor(() => expect(window.location.pathname).toBe("/tasks/one"));
    await screen.findByRole("textbox");
    expect(document.activeElement).not.toBe(replyBox());
  });

  it("hides and shows the sidebar on Cmd+B, focusing the view on screen when it reopens", async () => {
    setTransport(transport);
    window.history.replaceState(null, "", "/");

    // jsdom has no rendering loop, so this test's own rAF stands in for the
    // browser's; the production code only needs it to run after this turn.
    const originalRaf = globalThis.requestAnimationFrame;
    // SAFETY: this stub only needs to accept and invoke a FrameRequestCallback, matching the real signature's call shape.
    globalThis.requestAnimationFrame = ((callback: FrameRequestCallback) => {
      callback(0);
      return 0;
    }) as typeof requestAnimationFrame;

    try {
      render(
        <StrictMode>
          <AppShell />
        </StrictMode>,
      );
      await screen.findByText("second task");
      const sidebar = document.querySelector(".task-sidebar")!;
      expect(sidebar.className).not.toContain("task-sidebar-collapsed");

      fireEvent.keyDown(window, { key: "b", metaKey: true });
      await waitFor(() => expect(sidebar.className).toContain("task-sidebar-collapsed"));

      fireEvent.keyDown(window, { key: "b", metaKey: true });
      await waitFor(() => expect(sidebar.className).not.toContain("task-sidebar-collapsed"));
      expect(document.activeElement?.getAttribute("aria-current")).toBe("page");
      expect(document.activeElement?.textContent?.trim()).toBe("Recent");
    } finally {
      globalThis.requestAnimationFrame = originalRaf;
    }
  });

  it("keeps a finished task read after opening it, until it finishes again", async () => {
    const finishedAt = "2026-08-31T10:05:00Z";
    const finished = (updatedAt: string) => ({ state: "completed", updatedAt, completion: { blocked: false, code: "completed" } });
    const idle = summaryTask("idle", "idle task");
    let listed = [idle, summaryTask("finishing", "finishing task")];
    const opened = { ...fullTask("finishing", "prompt for finishing"), ...finished(finishedAt) };
    let listReads = 0;
    const listeners = new Map<string, (payload: EventBatch) => void>();
    const liveTransport: Transport = {
      invoke: async (command, args) => {
        // SAFETY: broker_call requests always carry a typed call object.
        const call = args?.call as { call: string; taskId?: string; query?: { archived?: string; limit?: number } } | undefined;
        if (call?.call === "summary") {
          // The task screen reads the summary too, for worker profiles; only the list's reads filter by archive.
          if (call.query?.archived !== undefined && call.query.limit !== 1) listReads += 1;
          // SAFETY: this fixture returns the summary response shape expected by SidebarController.
          return { profiles: PROFILES, tasks: listed, tasksHasMore: false, profileFailures: [], grants: [], memoryProjects: [] } as never;
        }
        if (command === "broker_watch_task" && args?.taskId === "finishing") {
          // SAFETY: this fixture returns the broker_watch_task response shape expected by TaskDetail.
          return { task: opened, events: [], cursor: 1, hasEarlier: false } as never;
        }
        if (call?.call === "task" && call.taskId === "finishing") {
          // SAFETY: `opened` is a full task, the shape a task call returns.
          return opened as never;
        }
        return transport.invoke(command, args);
      },
      // SAFETY: the only feed this test drives is the event batch feed.
      listen: (event, handle) => listeners.set(event, handle as (payload: EventBatch) => void),
    };
    const deliver = (cursor: number, state: TaskState, at: string) => act(() =>
      listeners.get(EVENT_BATCH_EVENT)!({ cursor, streamFloor: 0, stale: false, pointers: [
        { id: cursor, cursor, taskId: "finishing", type: state, kind: "lifecycle", state, at, title: "", summary: "" },
      ] }));
    const reread = async (tasks: typeof listed) => {
      listed = tasks;
      const before = listReads;
      act(() => listeners.get(EVENT_BATCH_EVENT)!({ cursor: 0, streamFloor: 0, stale: true, pointers: [] }));
      await waitFor(() => expect(listReads).toBe(before + 1));
      await act(() => new Promise((resolve) => setTimeout(resolve, 0)));
    };
    const finishingRow = () => within(document.getElementById("task-list")!).getByText("finishing task").closest("button")!;

    resetFeedsForTests();
    resetTaskOutcomeViewsForTests();
    setTransport(liveTransport);
    window.history.replaceState(null, "", "/");
    render(<AppShell />);
    await screen.findByText("finishing task");

    deliver(1, "completed", finishedAt);
    await waitFor(() => expect(finishingRow().textContent).toContain("New update"));

    fireEvent.doubleClick(finishingRow());
    await waitFor(() => expect(screen.getAllByText(/prompt for finishing/).length).toBeGreaterThan(0));
    showRecent();
    await waitFor(() => expect(window.location.pathname).toBe("/"));
    expect(finishingRow().textContent).toContain("Viewed");

    await reread([idle, { ...summaryTask("finishing", "finishing task"), ...finished(finishedAt) }]);
    expect(finishingRow().textContent).toContain("Viewed");

    const againAt = "2026-08-31T10:09:00Z";
    deliver(2, "running", "2026-08-31T10:07:00Z");
    deliver(3, "completed", againAt);
    await waitFor(() => expect(finishingRow().textContent).toContain("New update"));
    await reread([idle, { ...summaryTask("finishing", "finishing task"), ...finished(againAt) }]);
    expect(finishingRow().textContent).toContain("New update");
  });
});
