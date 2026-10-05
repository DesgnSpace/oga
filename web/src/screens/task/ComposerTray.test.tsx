import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ActivitySubagent } from "@/domain/activity";
import { ComposerTray } from "./ComposerTray";

function tray(queued: string[]) {
  const removed: number[] = [];
  const view = render(
    <ComposerTray
      queued={queued}
      subagents={[]}
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
