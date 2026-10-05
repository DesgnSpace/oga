import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ComposerTray } from "./ComposerTray";

function tray(queued: string[]) {
  const removed: number[] = [];
  const view = render(
    <ComposerTray queued={queued} subagents={[]} onRemoveQueued={(index) => removed.push(index)} />,
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
