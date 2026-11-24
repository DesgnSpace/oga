import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { setTransport, type Transport } from "@/bridge/transport";
import type { TaskSummary } from "@/bridge/types";
import { AppShell } from "@/shell/AppShell";

function summaryTask(id: string, title: string, state: string, updatedAt: string, extra: Partial<TaskSummary> = {}) {
  return { id, title, state, updatedAt, profileId: "worker", model: "anthropic/sonnet", cwd: "/repo",
    promptPreview: `brief for ${id}`, createdAt: "2026-08-31T09:00:00Z", ...extra };
}

const transport: Transport = {
  invoke: async (command, args) => {
    if (command === "broker_stream_status") {
      // SAFETY: this fixture returns the broker_stream_status response shape expected by SidebarController.
      return { connected: true, cursor: 0, streamFloor: 0, stale: false } as never;
    }
    // SAFETY: broker_call requests in this fixture always include a typed call object.
    const call = args?.call as { call: string } | undefined;
    if (command === "broker_call" && call?.call === "summary") {
      // SAFETY: this fixture returns the summary response shape expected by SidebarController.
      return {
        profiles: [{ id: "worker", label: "Claude work", provider: "claude", model: "sonnet", enabled: true, env: {}, capabilities: [] }],
        tasks: [
          summaryTask("done", "Newer finished task", "completed", "2026-08-31T12:00:00Z", { tldr: "All done." }),
          summaryTask("ask", "Older question", "needs_input", "2026-08-31T10:00:00Z", { question: "Which branch?" }),
        ],
        tasksHasMore: false, profileFailures: [], grants: [], memoryProjects: [],
      } as never;
    }
    throw { message: `no handler for ${command}` };
  },
  listen: () => {},
};

describe("the home screen", () => {
  afterEach(cleanup);

  it("leads with the task that needs you, previews the one picked, and opens it", async () => {
    setTransport(transport);
    window.history.replaceState(null, "", "/");
    render(<AppShell />);

    const list = within(await screen.findByRole("list", { name: "Recent tasks" }));
    const titles = list.getAllByRole("button").map((card) => card.querySelector("strong")?.textContent);
    expect(titles).toEqual(["Older question", "Newer finished task"]);

    const preview = () => within(screen.getByRole("complementary", { name: "Task preview" }));
    expect(preview().getByText("Which branch?")).toBeTruthy();

    fireEvent.click(list.getByText("Newer finished task"));
    await waitFor(() => expect(preview().getByText("All done.")).toBeTruthy());
    expect(preview().queryByText("Which branch?")).toBeNull();

    fireEvent.click(preview().getByRole("button", { name: "Open task" }));
    await waitFor(() => expect(window.location.pathname).toBe("/tasks/done"));
  });
});
