import { StrictMode } from "react";
import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { setTransport, type Transport } from "@/bridge/transport";
import { AppShell } from "./AppShell";

function summaryTask(id: string, preview: string) {
  return { id, profileId: "worker", model: "sonnet", cwd: "/repo", state: "running",
    promptPreview: preview, createdAt: "2026-08-31T10:00:00Z", updatedAt: "2026-08-31T10:00:00Z" };
}

function fullTask(id: string, prompt: string) {
  return { id, profileId: "worker", model: "sonnet", prompt, cwd: "/repo", state: "running",
    createdAt: "2026-08-31T10:00:00Z", updatedAt: "2026-08-31T10:00:00Z", output: "",
    scope: { read: [], write: [] }, allowQuestions: true };
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
      return { profiles: [], tasks: [summaryTask("one", "first task"), summaryTask("two", "second task")],
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
