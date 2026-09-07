import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { setTransport, tauriTransport, type Transport } from "@/bridge/transport";
import type { TaskEventView } from "@/bridge/types";
import type { FileChange } from "@/domain/changes";
import type { TraceRow } from "@/domain/trace";
import { expansionFromEvent } from "@/domain/trace";
import { resolvePreviewPath, TraceRows } from "./Trace";

afterEach(cleanup);

function fileEvent(id: number): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "broker",
    type: "agent.tool_use",
    kind: "file",
    phase: "completed",
    title: "Read file",
    createdAt: "2026-01-01T00:00:00.000Z",
  };
}

function row(overrides: Partial<TraceRow>): TraceRow {
  return {
    id: 1,
    style: "work",
    state: "done",
    children: [],
    startsExpanded: false,
    isStepStart: false,
    isTechnical: false,
    ...overrides,
  };
}

function proseRow(text: string, kind: "message" | "reasoning" = "message"): TraceRow {
  return row({
    style: "message",
    target: text,
    event: { ...fileEvent(1), kind, title: kind === "reasoning" ? "Reasoning" : "Agent message", detail: text },
    expansion: { type: "prose", text },
  });
}

class TestIntersectionObserver implements IntersectionObserver {
  readonly root = null;
  readonly rootMargin = "";
  readonly thresholds: readonly number[] = [];

  constructor(_callback: IntersectionObserverCallback, _options?: IntersectionObserverInit) {}

  observe(_target: Element) {}
  disconnect() {}
  unobserve(_target: Element) {}
  takeRecords(): IntersectionObserverEntry[] {
    return [];
  }
}

describe("TraceRows", () => {
  it("mounts only the newest window of a long trace", () => {
    const previous = globalThis.IntersectionObserver;
    globalThis.IntersectionObserver = TestIntersectionObserver;
    try {
      const rows = Array.from({ length: 1_000 }, (_, index) => row({ id: index + 1, target: `step ${index + 1}` }));
      const { container } = render(<TraceRows rows={rows} />);
      expect(container.querySelectorAll(".trace-list-static > .trace-row")).toHaveLength(60);
    } finally {
      globalThis.IntersectionObserver = previous;
    }
  });

  it("resolves relative image paths against the task directory", () => {
    expect(resolvePreviewPath(".look-shots/image.png", "/Users/malico/project/")).toBe(
      "/Users/malico/project/.look-shots/image.png",
    );
  });

  // A run group takes its id from the first member it contains, so a shared
  // expansion key would make the two rows open and close together.
  it("keeps a group open when the member sharing its id is collapsed", () => {
    const group = row({
      id: 7,
      verb: "Read",
      target: "3 files",
      children: [
        row({ id: 7, verb: "Read", target: "first.ts", event: fileEvent(7), expansion: { type: "content", text: "a", hiddenLines: 0, language: "plain" } }),
        row({ id: 8, verb: "Read", target: "second.ts" }),
      ],
    });

    render(<TraceRows rows={[group]} />);
    fireEvent.click(screen.getByText("3 files"));
    expect(screen.getByText("first.ts")).toBeDefined();

    fireEvent.click(screen.getByText("first.ts"));
    expect(screen.getByText("second.ts")).toBeDefined();
  });

  it("leaves a group's members collapsed when it opens", () => {
    const group = row({
      id: 7,
      verb: "Read",
      target: "3 files",
      children: [
        row({ id: 7, verb: "Read", target: "first.ts", event: fileEvent(7), expansion: { type: "content", text: "body", hiddenLines: 0, language: "plain" } }),
      ],
    });

    render(<TraceRows rows={[group]} />);
    fireEvent.click(screen.getByText("3 files"));
    expect(screen.queryByText("body")).toBeNull();
  });

  it("renders an OpenCode todo payload as checklist items with raw details available", () => {
    const event: TaskEventView = {
      ...fileEvent(1),
      source: "opencode",
      kind: "tool",
      title: "Todo list",
      presentation: { type: "todo", completed: 1, total: 3, text: "3 steps" },
      rawText: JSON.stringify({
        type: "tool_use",
        part: {
          tool: "todowrite",
          state: {
            input: {
              todos: [
                { content: "Read the files", status: "completed" },
                { content: "Fix the palette", status: "in_progress" },
                { content: "Run the tests", status: "pending" },
              ],
            },
          },
        },
      }),
    };
    const expansion = expansionFromEvent(event);

    expect(expansion?.type).toBe("todo");
    if (expansion?.type !== "todo") throw new Error("expected todo expansion");
    const { container } = render(<TraceRows rows={[row({ target: "3 steps", event, expansion })]} />);
    fireEvent.click(screen.getByText("3 steps"));

    expect(container.querySelectorAll(".trace-todo-item")).toHaveLength(3);
    expect(screen.getByText("Read the files")).toBeDefined();
    expect(screen.getByText("Fix the palette")).toBeDefined();
    expect(screen.getByText("Run the tests")).toBeDefined();
    expect(container.querySelector(".trace-raw-event")?.hasAttribute("open")).toBe(false);
    fireEvent.click(screen.getByText("Show raw event"));
    expect(container.querySelector(".trace-raw-event")?.hasAttribute("open")).toBe(true);
  });

  it("keeps failed results marked as failed", () => {
    const { container } = render(
      <TraceRows rows={[row({ state: "failed", result: "File not found: Cargo.toml" })]} />,
    );
    const failedRow = container.querySelector(".trace-row");

    expect(failedRow?.classList.contains("trace-state-failed")).toBe(true);
    expect(failedRow?.querySelector(".trace-result")?.textContent).toBe("File not found: Cargo.toml");
  });

  it("marks only running row text for the CSS shimmer", () => {
    const { container } = render(
      <TraceRows rows={[row({ state: "running", target: "cargo test" }), row({ id: 2, target: "done" })]} />,
    );

    expect(container.querySelector('[data-running="true"] .trace-target')?.textContent).toBe("cargo test");
    expect(container.querySelectorAll('[data-running="true"]')).toHaveLength(1);
  });

  it("renders command output as a terminal with an outcome", () => {
    const commandEvent: TaskEventView = {
      ...fileEvent(1),
      kind: "command",
      phase: "failed",
      title: "Run command",
      presentation: { type: "command", command: "cargo test\n--workspace" },
      rawText: JSON.stringify({ tool_response: { stderr: "error: test failed\n" } }),
    };
    const expansion = expansionFromEvent(commandEvent);
    if (expansion?.type !== "command") throw new Error("expected command expansion");

    const { container } = render(<TraceRows rows={[row({ target: "cargo test", event: commandEvent, expansion })]} />);
    fireEvent.click(container.querySelector(".trace-row-main-toggle") as HTMLElement);

    const terminal = container.querySelector(".trace-terminal");
    expect(terminal).toBeDefined();
    expect(terminal?.querySelector(".trace-terminal-label")?.textContent).toBe("shell");
    expect(terminal?.querySelector(".trace-terminal-prompt")?.textContent).toBe("$");
    expect(terminal?.querySelector(".trace-terminal-command code")?.textContent).toBe("cargo test\n--workspace");
    expect(terminal?.querySelector(".trace-terminal-command code")?.getAttribute("data-language")).toBe("shell");
    expect(terminal?.querySelector(".trace-terminal-output")?.textContent).toBe("error: test failed\n");
    expect(terminal?.querySelector(".review-content")).toBeNull();
    expect(terminal?.querySelector(".trace-terminal-outcome")?.textContent).toContain("Failed");
  });

  it("opens output at the bottom and stops following after scrolling up", () => {
    const commandEvent: TaskEventView = {
      ...fileEvent(1),
      kind: "command",
      title: "Run command",
      presentation: { type: "command", command: "cargo test" },
    };
    const expansion = expansionFromEvent(commandEvent);
    if (expansion?.type !== "command") throw new Error("expected command expansion");

    const view = render(
      <TraceRows rows={[row({ target: "cargo test", event: commandEvent, expansion: { ...expansion, output: "first\nsecond" } })]} />,
    );
    fireEvent.click(view.container.querySelector(".trace-row-main-toggle") as HTMLElement);

    const output = view.container.querySelector(".trace-terminal-output") as HTMLPreElement;
    Object.defineProperties(output, {
      clientHeight: { configurable: true, value: 100 },
      scrollHeight: { configurable: true, value: 1000 },
    });
    view.rerender(
      <TraceRows rows={[row({ target: "cargo test", event: commandEvent, expansion: { ...expansion, output: "first\nsecond\nthird" } })]} />,
    );
    expect(output.scrollTop).toBe(1000);

    Object.defineProperty(output, "scrollTop", { configurable: true, writable: true, value: 200 });
    fireEvent.scroll(output);
    view.rerender(
      <TraceRows rows={[row({ target: "cargo test", event: commandEvent, expansion: { ...expansion, output: "first\nsecond\nthird\nfourth" } })]} />,
    );
    expect(output.scrollTop).toBe(200);
  });

  it("formats worker prose labels and list items", () => {
    const { container } = render(<TraceRows rows={[proseRow("- **Tests**: pass\n- **Next**: compare")]} />);

    expect(container.querySelectorAll("ul > li")).toHaveLength(2);
    expect(container.querySelector("strong")?.textContent).toBe("Tests");
    expect(container.querySelectorAll("strong")[1]?.textContent).toBe("Next");
  });

  it("formats reasoning prose through the same path", () => {
    const { container } = render(<TraceRows rows={[proseRow("**Reason**: inspect the result", "reasoning")]} />);

    expect(container.querySelector("strong")?.textContent).toBe("Reason");
  });

  it("keeps fenced code literal inside worker prose", () => {
    const { container } = render(<TraceRows rows={[proseRow("```rust\n**not bold**\n```")]} />);
    const code = container.querySelector("pre");

    expect(code?.getAttribute("data-language")).toBe("rust");
    expect(code?.textContent).toBe("**not bold**");
    expect(code?.querySelector("strong")).toBeNull();
  });

  it("does not parse file diffs as markdown", () => {
    const change: FileChange = {
      blocks: [[{ kind: "removed", text: "- **old**" }, { kind: "added", text: "+ new" }]],
    };
    const diff = row({
      verb: "Edited",
      target: "src/file.ts",
      event: fileEvent(1),
      expansion: { type: "changes", change },
    });

    const { container } = render(<TraceRows rows={[diff]} />);
    fireEvent.click(screen.getByText("src/file.ts"));

    expect(container.querySelector(".trace-diff ul")).toBeNull();
    expect(screen.getByText("- **old**")).toBeDefined();
  });

  it("leaves a plain worker note as the existing trace text", () => {
    const { container } = render(<TraceRows rows={[proseRow("plain note")]} />);
    const target = container.querySelector(".trace-target");

    expect(target?.tagName).toBe("SPAN");
    expect(target?.textContent).toBe("plain note");
    expect(container.querySelector(".markdown-content")).toBeNull();
  });

  it("does not render transport tags from a wrapped file result", () => {
    const output = [
      "<path>/Users/malico/desgn/oga/web/src/oga.css</path>",
      "<type>file</type>",
      "<content>",
      "2001: font-size: 0.75rem;",
    ].join("\n");
    const event = {
      ...fileEvent(1),
      source: "opencode" as const,
      presentation: { type: "file" as const, path: "/Users/malico/desgn/oga/web/src/oga.css" },
      rawText: JSON.stringify({
        type: "tool_use",
        part: {
          type: "tool",
          tool: "read",
          state: { status: "completed", input: { filePath: "/Users/malico/desgn/oga/web/src/oga.css" }, output },
        },
      }),
    };
    const expansion = expansionFromEvent(event);
    expect(expansion?.type).toBe("content");
    if (expansion?.type !== "content") throw new Error("expected content expansion");
    const { container } = render(
      <TraceRows rows={[row({ target: "oga.css", event, expansion })]} />,
    );
    fireEvent.click(screen.getByText("oga.css"));

    expect(container.textContent).toContain("2001: font-size: 0.75rem;");
    expect(container.textContent).not.toMatch(/<\/?(?:path|type|content)>/);
  });

  it("renders an image written on disk through the desktop bridge", async () => {
    const imagePath = "/Users/malico/desgn/inter/logos/export/logo-192.png";
    const invoke = (async (command, args) => {
      if (command !== "read_image_preview") throw new Error(`unexpected command: ${command}`);
      const bytes = Array.from(await Bun.file(String(args?.path)).bytes());
      return { bytes, mime: "image/png" };
    }) as Transport["invoke"];
    setTransport({ invoke, listen: () => undefined });

    try {
      const event = {
        ...fileEvent(1),
        presentation: { type: "file" as const, path: imagePath },
        rawText: JSON.stringify({ tool_result: { output: "image written" } }),
      };
      const expansion = expansionFromEvent(event);
      if (expansion?.type !== "content") throw new Error("expected content expansion");
      const { container } = render(<TraceRows rows={[row({ target: imagePath, event, expansion })]} />);
      fireEvent.click(screen.getByText(imagePath));

      await waitFor(() => expect(container.querySelector(".trace-file-preview-image img")?.getAttribute("src")).toMatch(/^data:image\/png;base64,/));
      const previewButton = container.querySelector(".trace-file-preview-image");
      expect(previewButton).toBeDefined();
      if (!previewButton) throw new Error("expected image preview button");
      fireEvent.click(previewButton);
      await waitFor(() => expect(container.querySelector(".trace-file-preview-modal-image")?.getAttribute("src")).toMatch(/^data:image\/png;base64,/));
    } finally {
      setTransport(tauriTransport);
    }
  });
});
