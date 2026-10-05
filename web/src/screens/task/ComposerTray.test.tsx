import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ActivitySubagent } from "@/domain/activity";
import type { TodoItem, TodoProgress } from "@/domain/trace";
import { ComposerTray } from "./ComposerTray";

function tray(queued: string[]) {
  const removed: number[] = [];
  const view = render(
    <ComposerTray
      queued={queued}
      subagents={[]}
      todos={undefined}
      onRemoveQueued={(index) => removed.push(index)}
      onSelectSubagent={() => {}}
    />,
  );
  return { ...view, removed };
}

function texts(): string[] {
  return screen.getAllByRole("listitem").map((item) => item.textContent ?? "");
}

describe("the queued section of the composer tray", () => {
  afterEach(cleanup);

  it("shows nothing while no message is waiting", () => {
    tray([]);
    expect(screen.queryByText("Queued")).toBeNull();
  });

  it("keeps the messages in the order they will be sent", () => {
    tray(["first", "second", "third"]);
    expect(texts()).toEqual(["first", "second", "third"]);
  });

  it("folds past the third message and opens the rest on request", () => {
    tray(["one", "two", "three", "four", "five"]);
    expect(texts()).toEqual(["one", "two", "three"]);

    fireEvent.click(screen.getByRole("button", { name: "Show all 5" }));
    expect(texts()).toEqual(["one", "two", "three", "four", "five"]);

    fireEvent.click(screen.getByRole("button", { name: "Show fewer" }));
    expect(texts()).toEqual(["one", "two", "three"]);
  });

  it("removes a message by its place in the queue, not its place on screen", () => {
    const { removed } = tray(["one", "two", "three", "four", "five"]);
    fireEvent.click(screen.getByRole("button", { name: "Show all 5" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove queued message 5" }));
    expect(removed).toEqual([4]);
  });
});

function subagent(id: string, label?: string): ActivitySubagent {
  return { id, label, events: [], nodes: [], status: "running" };
}

function agents(onSelectSubagent: (nodeId: string) => void, subagents: ActivitySubagent[]) {
  return render(
    <ComposerTray
      queued={[]}
      subagents={subagents}
      todos={undefined}
      onRemoveQueued={() => {}}
      onSelectSubagent={onSelectSubagent}
    />,
  );
}

describe("the subagents section of the composer tray", () => {
  afterEach(cleanup);

  it("stays hidden while no subagent is running", () => {
    tray([]);
    expect(screen.queryByText("Subagents")).toBeNull();
  });

  it("names each running subagent after what it is doing", () => {
    agents(() => {}, [subagent("subagent:one", "Survey crates"), subagent("subagent:two", "Read notes")]);
    expect(
      screen.getByRole("button", { name: "Show Survey crates in the transcript" }),
    ).toBeDefined();
    expect(screen.getByRole("button", { name: "Show Read notes in the transcript" })).toBeDefined();
  });

  it("names a subagent that says nothing about itself", () => {
    agents(() => {}, [subagent("subagent:one")]);
    expect(screen.getByRole("button", { name: "Show Subagent in the transcript" })).toBeDefined();
  });

  it("targets the clicked subagent, not its neighbor", () => {
    const selected: string[] = [];
    agents((nodeId) => selected.push(nodeId), [
      subagent("subagent:one", "Survey crates"),
      subagent("subagent:two", "Read notes"),
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Show Read notes in the transcript" }));
    expect(selected).toEqual(["subagent:two"]);
  });
});

function plan(todos: TodoProgress | undefined) {
  return render(
    <ComposerTray
      queued={[]}
      subagents={[]}
      todos={todos}
      onRemoveQueued={() => {}}
      onSelectSubagent={() => {}}
    />,
  );
}

describe("the plan section of the composer tray", () => {
  afterEach(cleanup);

  function progress(overrides: Partial<TodoProgress> = {}): TodoProgress {
    return {
      items: [
        { text: "Read the files", status: "completed" },
        { text: "Fix the palette", status: "in_progress" },
        { text: "Run the tests", status: "pending" },
      ],
      done: 1,
      total: 3,
      current: "Fix the palette",
      ...overrides,
    };
  }

  it("stays hidden while the run keeps no plan", () => {
    plan(undefined);
    expect(screen.queryByText("Plan")).toBeNull();
  });

  it("folds to the step in hand and the count, then opens every step in order", () => {
    plan(progress());

    expect(screen.queryAllByRole("listitem")).toHaveLength(0);
    expect(screen.getByText("Fix the palette")).toBeDefined();
    expect(screen.getByText("1 of 3")).toBeDefined();
    expect(screen.queryByText("Read the files")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Show all 3" }));

    expect(texts()).toEqual(["Read the files", "Fix the palette", "Run the tests"]);
    expect(screen.queryByText("1 of 3")).toBeNull();
  });

  it("says every step is done once the last one lands while the run carries on", () => {
    const items: TodoItem[] = [
      { text: "Read the files", status: "completed" },
      { text: "Fix the palette", status: "completed" },
    ];
    plan({ items, done: 2, total: 2 });

    expect(screen.getByText("Every step is done")).toBeDefined();
    expect(screen.getByText("2 of 2")).toBeDefined();
  });
});
