import { StrictMode } from "react";
import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { setTransport, type Transport } from "@/bridge/transport";
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
      return { profiles: [], tasks: [summaryTask("one", "first task"), summaryTask("two", "second task"), summaryTask("three", "third task")],
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

  it("opens the task a sidebar click picks", async () => {
    setTransport(transport);
    window.history.replaceState(null, "", "/");

    render(
      <StrictMode>
        <AppShell />
      </StrictMode>,
    );

    const row = await screen.findByText("second task");
    row.closest("a")!.click();

    await waitFor(() => expect(window.location.pathname).toBe("/tasks/two"));
    await waitFor(() => expect(screen.getAllByText(/prompt for two/).length).toBeGreaterThan(0), { timeout: 5000 });
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "Add a follow-up…" })));
  });

  it("focuses the newly picked task when switching tasks and when picking the open task", async () => {
    setTransport(transport);
    window.history.replaceState(null, "", "/");
    render(<AppShell />);

    const firstRow = (await screen.findByText("first task")).closest("a")!;
    const secondRow = screen.getByText("second task").closest("a")!;
    firstRow.click();
    const firstReply = await screen.findByRole("textbox", { name: "Add a follow-up…" });
    await waitFor(() => expect(document.activeElement).toBe(firstReply));

    secondRow.click();
    const secondReply = await screen.findByRole("textbox", { name: "Add a follow-up…" });
    await waitFor(() => expect(document.activeElement).toBe(secondReply));

    secondRow.click();
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "Add a follow-up…" })));
  });

  it("does not focus the reply box when a task is opened from its URL or browser history", async () => {
    setTransport(transport);
    window.history.replaceState(null, "", "/tasks/one");
    render(<AppShell />);

    const reply = await screen.findByRole("textbox", { name: "Add a follow-up…" });
    expect(document.activeElement).not.toBe(reply);

    const row = screen.getByText("second task").closest("a")!;
    row.click();
    const selectedReply = await screen.findByRole("textbox", { name: "Add a follow-up…" });
    await waitFor(() => expect(document.activeElement).toBe(selectedReply));
    screen.getByText("first task").closest("a")!.focus();
    window.history.back();
    await waitFor(() => expect(window.location.pathname).toBe("/tasks/one"));
    window.history.forward();
    await waitFor(() => expect(window.location.pathname).toBe("/tasks/two"));
    await waitFor(() => expect(document.activeElement).not.toBe(screen.getByRole("textbox", { name: "Add a follow-up…" })));
  });

  it("leaves focus alone when the selected task has no reply box", async () => {
    setTransport(transport);
    window.history.replaceState(null, "", "/");
    render(<AppShell />);

    const row = (await screen.findByText("third task")).closest("a")!;
    row.focus();
    row.click();
    await waitFor(() => expect(window.location.pathname).toBe("/tasks/three"));
    expect(document.activeElement).toBe(row);
    expect(screen.queryByRole("textbox", { name: "Add a follow-up…" })).toBeNull();
  });

  it("keeps the title bar's run time counting while a running task sends nothing", async () => {
    const runningSince = new Date(Date.now() - 5_000).toISOString();
    setTransport({
      ...transport,
      invoke: async (command, args) => {
        if (command !== "broker_watch_task") return transport.invoke(command, args);
        // SAFETY: this fixture returns the broker_watch_task response shape expected by TaskDetail.
        return { task: { ...fullTask("one", "prompt for one"), runningSince }, events: [], cursor: 0, hasEarlier: false } as never;
      },
    });
    window.history.replaceState(null, "", "/tasks/one");
    render(<AppShell />);

    const stats = await screen.findByLabelText("Task status and usage", undefined, { timeout: 5_000 });
    await waitFor(() => expect(stats.textContent).toMatch(/\ds/));
    const shown = stats.textContent;
    await waitFor(() => expect(stats.textContent).not.toBe(shown), { timeout: 3_000 });
  });

  it("hides and shows the task list on Cmd+B, focusing the list when it reopens", async () => {
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
      expect(document.activeElement?.getAttribute("role")).toBe("option");
    } finally {
      globalThis.requestAnimationFrame = originalRaf;
    }
  });
});
