import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { resetFeedsForTests } from "@/bridge/events";
import { setTransport, type Transport } from "@/bridge/transport";
import type { StateQuery, TaskState, TaskSummary } from "@/bridge/types";
import { resetTaskOutcomeViewsForTests } from "@/state/task-outcome-views";
import { AppShell } from "@/shell/AppShell";

function summaryTask(id: string, title: string, state: TaskState, updatedAt: string, extra: Partial<TaskSummary> = {}): TaskSummary {
  return { id, title, state, updatedAt, profileId: "worker", model: "anthropic/sonnet", cwd: "/repo",
    promptPreview: `brief for ${id}`, createdAt: "2026-08-31T09:00:00Z", ...extra };
}

const WORKER = { id: "worker", label: "Claude work", provider: "claude", model: "sonnet", enabled: true, env: {}, capabilities: [] };

interface RecordedCall {
  call: string;
  query?: StateQuery;
  taskId?: string;
  archived?: boolean;
  deleteBranch?: boolean;
}

interface Broker {
  transport: Transport;
  calls: RecordedCall[];
}

/** A broker that answers list reads with `pages(query)` and records every call. */
function broker(pages: (query: StateQuery) => { tasks: TaskSummary[]; tasksHasMore?: boolean; groupCounts?: Record<string, number> }): Broker {
  const calls: Broker["calls"] = [];
  const transport: Transport = {
    invoke: async (command, args) => {
      if (command === "broker_stream_status") {
        // SAFETY: this fixture returns the StreamStatus shape the sidebar reads.
        return { connected: true, cursor: 0, streamFloor: 0, stale: false } as never;
      }
      if (command !== "broker_call") throw { message: `no handler for ${command}` };
      // SAFETY: broker_call requests always carry a call object.
      const call = args?.call as RecordedCall;
      calls.push(call);
      if (call.call === "summary") {
        const page = pages(call.query!);
        // SAFETY: this fixture returns the BrokerSummaryState shape SidebarController reads.
        return { profiles: [WORKER], tasksHasMore: false, profileFailures: [], grants: [], memoryProjects: [], ...page } as never;
      }
      if (call.call === "archiveTask") {
        // SAFETY: the archive call's caller reads only whether it succeeded.
        return { archived: true } as never;
      }
      throw { message: `no handler for ${call.call}` };
    },
    listen: () => {},
  };
  return { transport, calls };
}

function list() {
  return within(document.getElementById("home-tasks")!);
}

function titles() {
  return list().getAllByRole("button").flatMap((row) => row.querySelector("strong")?.textContent ?? []);
}

describe("the home screen", () => {
  beforeEach(() => {
    resetFeedsForTests();
    resetTaskOutcomeViewsForTests();
    window.localStorage.clear();
    window.history.replaceState(null, "", "/");
  });
  afterEach(cleanup);

  it("leads with what needs you, then what runs, then finished work by day, and a click opens the task", async () => {
    const tasks = [
      summaryTask("done", "Newer finished task", "completed", "2026-08-31T12:00:00Z"),
      summaryTask("run", "Running task", "running", "2026-08-31T11:00:00Z"),
      summaryTask("ask", "Older question", "needs_input", "2026-08-31T10:00:00Z", { question: "Which branch?" }),
    ];
    setTransport(broker(() => ({ tasks, groupCounts: { completed: 1, running: 1, needs_input: 1 } })).transport);
    render(<AppShell />);

    await waitFor(() => expect(list().getAllByRole("heading").map((heading) => heading.textContent)).toEqual(["Needs you", "In progress", "Earlier"]));
    expect(titles()).toEqual(["Older question", "Running task", "Newer finished task"]);
    expect(screen.getByText("3 tasks")).toBeTruthy();

    fireEvent.click(list().getByText("Newer finished task"));
    await waitFor(() => expect(window.location.pathname).toBe("/tasks/done"));
  });

  it("lists only what the sidebar search finds", async () => {
    const tasks = [
      summaryTask("ship", "Ship the price fix", "completed", "2026-08-31T12:00:00Z"),
      summaryTask("docs", "Write the docs", "completed", "2026-08-31T11:00:00Z"),
    ];
    setTransport(broker(() => ({ tasks })).transport);
    render(<AppShell />);

    await waitFor(() => expect(titles()).toEqual(["Ship the price fix", "Write the docs"]));
    fireEvent.click(screen.getByRole("button", { name: "Search tasks" }));
    fireEvent.change(await screen.findByRole("searchbox", { name: "Search tasks" }), { target: { value: "price" } });

    await waitFor(() => expect(titles()).toEqual(["Ship the price fix"]));
    expect(screen.getByText("Titles matching “price”")).toBeTruthy();
  });

  it("archives a task from its row menu and reads the list again", async () => {
    let tasks = [summaryTask("done", "Finished work", "completed", "2026-08-31T12:00:00Z")];
    const { transport, calls } = broker(() => ({ tasks }));
    setTransport(transport);
    render(<AppShell />);

    fireEvent.click(await waitFor(() => list().getByRole("button", { name: "Task actions for Finished work" })));
    const reads = calls.filter((call) => call.call === "summary").length;
    tasks = [];
    fireEvent.click(await screen.findByRole("menuitem", { name: "Archive" }));

    await waitFor(() => expect(calls).toContainEqual({ call: "archiveTask", taskId: "done", archived: true, deleteBranch: false }));
    await waitFor(() => expect(calls.filter((call) => call.call === "summary").length).toBeGreaterThan(reads));
    await waitFor(() => expect(document.getElementById("home-tasks")).toBeNull());
  });

  it("says what a held task waits for, and reads a blocked one as a problem", async () => {
    const held = summaryTask("held", "Held task", "pending", "2026-08-31T12:00:00Z", {
      hold: { kind: "time", until: "2026-09-01T09:00:00Z", note: "Starts later", expiresAt: "2026-09-01T09:00:00Z" },
    });
    const blocked = summaryTask("blocked", "Blocked task", "blocked", "2026-08-31T11:00:00Z", {
      completion: { blocked: true, code: "cancelled", dependencyBlocked: true },
    });
    setTransport(broker(() => ({ tasks: [held, blocked] })).transport);
    render(<AppShell />);

    const heldRow = (await waitFor(() => list().getByText("Held task"))).closest("button")!;
    expect(heldRow.querySelector(".task-dot")!.className).toContain("task-dot-look-waiting");
    expect(heldRow.textContent).toContain("Starts");

    const blockedRow = list().getByText("Blocked task").closest("button")!;
    expect(blockedRow.querySelector(".task-dot")!.className).toContain("task-dot-look-problem");
    expect(blockedRow.textContent).toContain("Blocked");
  });
});
