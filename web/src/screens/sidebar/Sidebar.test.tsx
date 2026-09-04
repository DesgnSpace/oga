import { afterEach, describe, expect, it, mock } from "bun:test";
import { cleanup, render, screen } from "@testing-library/react";
import { setTransport, type Transport } from "@/bridge/transport";
import { SidebarController } from "@/state";
import Sidebar from "./Sidebar";

function task(id: string, preview: string) {
  return {
    id,
    profileId: "worker",
    model: "sonnet",
    cwd: "/repo",
    state: "running",
    promptPreview: preview,
    createdAt: "2026-08-31T10:00:00Z",
    updatedAt: "2026-08-31T10:00:00Z",
  };
}

function transport(
  options: { profiles?: Array<{ id: string; label: string }> } = {},
): Transport {
  return {
    invoke: async (command: string, args?: Record<string, unknown>) => {
      if (command !== "broker_call") throw { message: `no handler for ${command}` };
      const call = (args?.call as { call: string }).call;
      if (call === "summary") {
        return {
          profiles: options.profiles ?? [],
          tasks: [task("one", "first task"), task("two", "second task")],
          tasksHasMore: false,
          profileFailures: [],
          grants: [],
          memoryProjects: [],
        } as never;
      }
      throw { message: `no handler for ${call}` };
    },
    listen: () => {},
  };
}

describe("the sidebar", () => {
  afterEach(cleanup);

  it("hands a clicked task to the shell", async () => {
    setTransport(transport());
    const selected = mock();
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={selected} />);

    const row = await screen.findByText("second task");
    row.closest("a")!.click();

    expect(selected).toHaveBeenCalledWith("two");
  });

  it("names the worker that ran the task in the subtitle", async () => {
    setTransport(transport({ profiles: [{ id: "worker", label: "Night Shift" }] }));
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    await screen.findByText("second task");
    expect(screen.getAllByText(/^Night Shift/)).toHaveLength(2);
  });

  it("falls back to a sensible subtitle when no worker is recorded", async () => {
    setTransport(transport({ profiles: [] }));
    const controller = new SidebarController();

    render(<Sidebar sidebarController={controller} onSelectTask={mock()} />);

    await screen.findByText("second task");
    expect(screen.getAllByText(/^Unknown worker/)).toHaveLength(2);
  });
});
