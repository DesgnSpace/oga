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
  return within(document.getElementById("task-list")!);
}

function preview() {
  return within(screen.getByRole("complementary", { name: "Task preview" }));
}

function showView(name: string) {
  fireEvent.click(within(screen.getByRole("navigation", { name: "Task views" })).getByRole("button", { name: new RegExp(`^${name}`) }));
}

describe("the home screen", () => {
  beforeEach(() => {
    resetFeedsForTests();
    resetTaskOutcomeViewsForTests();
    window.localStorage.clear();
    window.history.replaceState(null, "", "/");
  });
  afterEach(cleanup);

  it("leads with what needs you, previews the pick once, and opens it", async () => {
    const tasks = [
      summaryTask("done", "Newer finished task", "completed", "2026-08-31T12:00:00Z", {
        tldr: "Ship the `formatPrice` fix.",
        promptPreview: "Goal: ship the `formatPrice` fix.\n\nKnown:\n- the tests in `money.test.ts`",
      }),
      summaryTask("ask", "Older question", "needs_input", "2026-08-31T10:00:00Z", { question: "Which branch?" }),
    ];
    setTransport(broker(() => ({ tasks })).transport);
    render(<AppShell />);

    await waitFor(() => expect(list().getAllByRole("heading").map((heading) => heading.textContent)).toEqual(["Needs you", "Earlier"]));
    const titles = () => list().getAllByRole("button").flatMap((row) => row.querySelector("strong")?.textContent ?? []);
    expect(titles()).toEqual(["Older question", "Newer finished task"]);
    expect(preview().getByText("Which branch?")).toBeTruthy();

    fireEvent.click(list().getByText("Newer finished task"));
    await waitFor(() => expect(preview().getByText("formatPrice")).toBeTruthy());
    // The summary shows once, as text: no raw backticks and no second copy of the brief.
    expect(preview().queryByText(/`/)).toBeNull();
    expect(preview().queryByText(/Known/)).toBeNull();
    expect(preview().queryByText("Which branch?")).toBeNull();

    showView("Needs you");
    await waitFor(() => expect(titles()).toEqual(["Older question"]));

    fireEvent.click(preview().getByRole("button", { name: "Open task to answer" }));
    await waitFor(() => expect(window.location.pathname).toBe("/tasks/ask"));
  });

  it("keeps loading older tasks in Needs you until every task it counts is listed", async () => {
    const recent = Array.from({ length: 50 }, (_, index) =>
      summaryTask(`done-${index}`, `Finished ${index}`, "completed", `2026-08-31T11:${String(index).padStart(2, "0")}:00Z`));
    const old = summaryTask("old-failure", "Old failure", "failed", "2026-08-01T10:00:00Z", { error: "rate limited" });
    setTransport(broker((query) => {
      const counts = { completed: 50, failed: 1 };
      if (query.limit === 1) return { tasks: [], groupCounts: query.group === "status" ? counts : { "/repo": 51 } };
      const all = [...recent, old];
      return { tasks: all.slice(0, query.limit), tasksHasMore: (query.limit ?? 50) < all.length, groupCounts: counts };
    }).transport);
    render(<AppShell />);

    await waitFor(() => expect(list().getByText("Finished 0")).toBeTruthy());
    showView("Needs you");

    await waitFor(() => expect(list().getByText("Old failure")).toBeTruthy());
    expect(screen.queryByText("Nothing needs you right now.")).toBeNull();
  });

  it("archives a task from its row menu and reads the list again", async () => {
    let tasks = [summaryTask("done", "Finished work", "completed", "2026-08-31T12:00:00Z")];
    const { transport, calls } = broker(() => ({ tasks }));
    setTransport(transport);
    render(<AppShell />);

    fireEvent.click(await screen.findByRole("button", { name: "Task actions for Finished work" }));
    const reads = calls.filter((call) => call.call === "summary").length;
    tasks = [];
    fireEvent.click(await screen.findByRole("menuitem", { name: "Archive" }));

    await waitFor(() => expect(calls).toContainEqual({ call: "archiveTask", taskId: "done", archived: true, deleteBranch: false }));
    await waitFor(() => expect(calls.filter((call) => call.call === "summary").length).toBeGreaterThan(reads));
    await waitFor(() => expect(document.getElementById("task-list")).toBeNull());
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
