import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { TaskEventView } from "@/bridge/types";
import { ActivityStory, type ActivityComposition } from "@/domain/activity";
import { Transcript } from "./Transcript";
import type { TranscriptItem } from "./transcriptModel";

afterEach(cleanup);

describe("Transcript", () => {
  it("renders the final worker report as markdown", () => {
    const { container } = render(
      <Transcript items={[{ type: "response", block: { id: 1, text: "**Done**\n\n- checked", error: false } }]} />,
    );

    expect(container.querySelector("strong")?.textContent).toBe("Done");
    expect(container.querySelectorAll("li")).toHaveLength(1);
  });

  function work(id: number, startsExpanded: boolean): TranscriptItem {
    const event: TaskEventView = {
      id,
      taskId: "task",
      source: "claude",
      type: "agent.tool_use",
      kind: "command",
      phase: "completed",
      title: "Run command",
      detail: `step ${id}`,
      createdAt: `2026-07-30T15:00:0${id}Z`,
    };
    return {
      type: "work",
      segment: {
        id,
        composition: ActivityStory.compose([event]),
        cwd: "/repo",
        live: startsExpanded,
        startsExpanded,
        durationMs: 2_000,
      },
    };
  }

  it("collapses earlier work and opens only the current block", () => {
    render(<Transcript items={[work(1, false), work(2, true)]} />);

    expect(screen.getAllByRole("button").map((button) => button.getAttribute("aria-expanded"))).toEqual(["false", "true"]);
    expect(screen.getByText(/Working for 2 seconds/).textContent).toContain("1 action");
    expect(screen.getByText(/Working for 2 seconds/).textContent).not.toContain("step 2");
  });

  it("keeps a manual toggle through a live rerender", () => {
    const item = work(1, true);
    const view = render(<Transcript items={[item]} />);
    const button = screen.getByRole("button");

    fireEvent.click(button);
    expect(button.getAttribute("aria-expanded")).toBe("false");

    if (item.type !== "work") throw new Error("expected work item");
    view.rerender(<Transcript items={[{ ...item, segment: { ...item.segment, startsExpanded: true } }]} />);
    expect(screen.getByRole("button").getAttribute("aria-expanded")).toBe("false");
  });

  it("uses an action count when a turn title only repeats a child title", () => {
    const event: TaskEventView = {
      id: 1,
      taskId: "task",
      source: "claude",
      type: "agent.tool_use",
      kind: "command",
      phase: "completed",
      title: "Searched grep -n pattern",
      detail: "pattern",
      createdAt: "2026-07-30T15:00:01Z",
    };
    const composition: ActivityComposition = {
      blocks: [{
        type: "chapter",
        id: 1,
        rows: [{
          type: "group",
          group: {
            kind: "turn",
            anchor: event,
            children: [{ type: "work", event }],
            members: [],
            runLabel: "",
            turnTitle: event.title,
            hidden: [event],
          },
        }],
      }],
      technical: [],
    };

    render(<Transcript items={[{
      type: "work",
      segment: {
        id: 1,
        composition,
        cwd: "/repo",
        live: false,
        startsExpanded: false,
        durationMs: 38_000,
      },
    }]} />);

    expect(screen.getByRole("button").textContent).toContain("1 action");
    expect(screen.getByRole("button").textContent).not.toContain(event.title);
  });

  it("keeps thinking out of the trace until the reader asks for it", () => {
    const thinking: TaskEventView = {
      id: 1,
      taskId: "task",
      source: "claude",
      type: "agent.assistant",
      kind: "reasoning",
      phase: "info",
      title: "Thinking",
      detail: "Weighing two options",
      createdAt: "2026-07-30T15:00:01Z",
    };
    const item: TranscriptItem = {
      type: "work",
      segment: {
        id: 1,
        composition: ActivityStory.compose([thinking]),
        cwd: "/repo",
        live: false,
        startsExpanded: true,
        durationMs: 2_000,
      },
    };

    const view = render(<Transcript items={[item]} />);
    expect(screen.queryByText("Weighing two options")).toBeNull();

    view.rerender(<Transcript items={[item]} showThinking={true} />);
    expect(screen.getByText("Weighing two options")).toBeTruthy();
  });

  // happy-dom reports no layout, so pretend every bubble overflows its preview.
  function withOverflowingBubble(run: () => void): void {
    const scrollHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollHeight");
    const clientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
    Object.defineProperty(HTMLElement.prototype, "scrollHeight", { configurable: true, get() { return 200; } });
    Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get() { return 50; } });
    try {
      run();
    } finally {
      if (scrollHeight) Object.defineProperty(HTMLElement.prototype, "scrollHeight", scrollHeight);
      if (clientHeight) Object.defineProperty(HTMLElement.prototype, "clientHeight", clientHeight);
    }
  }

  function goalBubble(): TranscriptItem {
    return {
      type: "bubble",
      bubble: {
        id: -1,
        text: "Add MongoDB as a new adapter.\n\n## Context\n\nPluk already supports Postgres.",
        at: "2026-07-30T15:00:00Z",
      },
    };
  }

  it("keeps the goal heading visible behind a toggle that expands", () => {
    withOverflowingBubble(() => {
      render(<Transcript items={[goalBubble()]} />);

      expect(screen.getByText("Context")).toBeTruthy();
      const toggle = screen.getByRole("button", { name: "Show more" });
      expect(toggle.getAttribute("aria-expanded")).toBe("false");

      fireEvent.click(toggle);
      expect(screen.getByRole("button", { name: "Show less" }).getAttribute("aria-expanded")).toBe("true");
      expect(screen.getByText("Pluk already supports Postgres.")).toBeTruthy();
    });
  });

  it("expands a long goal when its preview is clicked", () => {
    withOverflowingBubble(() => {
      const { container } = render(<Transcript items={[goalBubble()]} />);

      const preview = container.querySelector(".transcript-bubble-preview");
      if (!preview) throw new Error("expected a bubble preview");
      fireEvent.click(preview);
      expect(screen.getByRole("button", { name: "Show less" })).toBeTruthy();
    });
  });
});
