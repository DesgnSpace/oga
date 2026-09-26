import { describe, expect, it } from "bun:test";
import type { EventKind, TaskEventView } from "@/bridge/types";
import { ActivityStory, type ReasoningPulse } from "@/domain/activity";
import {
  expansionFromEvent,
  expansionOffers,
  relativePaths,
  TraceRowBuilder,
  traceRowOffersExpansion,
  traceRowWeight,
  withoutThinking,
  type TraceRow,
} from "./index";

function event(id: number, kind: EventKind, title: string): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: `agent.${kind}`,
    kind,
    phase: "completed",
    title,
    createdAt: "2026-07-30T15:00:00Z",
  };
}

function traceRows(events: TaskEventView[], live = false, cwd = "/repo"): TraceRow[] {
  return TraceRowBuilder.rows(ActivityStory.compose(events).blocks, cwd, live);
}

describe("trace rows", () => {
  it("gives command output a single-line preview", () => {
    const commandEvent: TaskEventView = {
      ...event(1, "command", "Bash"),
      presentation: { type: "command", command: "cargo test" },
      rawText: JSON.stringify({ tool_response: { stdout: "clean\n" } }),
    };
    const rows = traceRows([commandEvent], false);
    expect(rows[0].preview).toBe("clean");
    expect(rows[0].children.length === 0 && rows[0].event !== undefined).toBe(true);
  });

  it("uses the command summary instead of an error payload for the collapsed title", () => {
    const command = "cd /tmp && opencode run --format json --model opencode-go/minimax-m2.7";
    const commandEvent: TaskEventView = {
      ...event(1, "command", "Run command"),
      source: "opencode",
      verb: "Ran",
      presentation: {
        type: "command",
        command,
        text: "opencode run --format json --model opencode-go/minimax-m2.7",
      },
      rawText: JSON.stringify({
        type: "tool_use",
        part: {
          tool: "bash",
          state: {
            status: "error",
            input: { command },
            output: '{"type":"error","error":{"data":{"statusCode":401}}}',
          },
        },
      }),
    };

    const rows = traceRows([commandEvent], false);

    expect(rows[0].target).toBe("opencode run --format json --model opencode-go/minimax-m2.7");
    expect(rows[0].expansion?.type).toBe("command");
  });

  it("falls back to the full command when no summary is present", () => {
    const command = "cd /tmp && opencode run --format json";
    const commandEvent: TaskEventView = {
      ...event(1, "command", "Run command"),
      source: "opencode",
      verb: "Ran",
      presentation: { type: "command", command },
    };
    const rows = traceRows([commandEvent], false);
    expect(rows[0].target).toBe(command);
  });

  it("keeps event targets when presentation is missing", () => {
    const toolEvent: TaskEventView = { ...event(1, "tool", "Read"), verb: "Read", detail: "/repo/src/main.rs" };
    const rows = traceRows([toolEvent], false);
    expect(rows[0].target).toBe("src/main.rs");
  });

  it("shows a fenced, line-numbered read result as one clean line", () => {
    const readEvent: TaskEventView = {
      ...event(1, "file", "Read"),
      verb: "Read",
      presentation: { type: "file", path: "/repo/web/src/state/sidebar-preferences.ts" },
      result: "```typescript\n1\t// Sidebar filter/grouping/sort choices and collapsed group ids, persisted.\n2\texport {};\n```",
    };
    const rows = traceRows([readEvent], false);
    expect(rows[0].target).toBe("web/src/state/sidebar-preferences.ts");
    expect(rows[0].result).toBe("// Sidebar filter/grouping/sort choices and collapsed group ids, persisted.");
  });

  it("does not invent a verb when the event has none", () => {
    const rows = traceRows([{ ...event(1, "tool", "Tool call"), detail: "whatever it did" }], false);
    expect(rows[0].verb).toBeUndefined();
  });

  it("keeps a search query, scope, and match count on the collapsed row", () => {
    const searchEvent: TaskEventView = {
      ...event(1, "tool", "Search code"),
      verb: "Searched",
      presentation: { type: "tool", text: "TaskEventView in rust", outcome: "4 matches" },
    };
    const rows = traceRows([searchEvent], false);
    expect(rows[0].target).toBe("TaskEventView in rust");
    expect(rows[0].result).toBe("4 matches");
  });

  // Real shape from `~/.oga/oga.db`: the worker ran `oga query` through Bash,
  // and the hook nests the tool's stdout under `tool_response`.
  it("shows an oga query search's matches on expansion", () => {
    const searchEvent: TaskEventView = {
      ...event(1, "tool", "Search code"),
      verb: "Searched",
      presentation: { type: "tool", text: 'oga query "tool description strings"' },
      rawText: JSON.stringify({
        hook_event_name: "PostToolUse",
        tool_name: "Bash",
        tool_input: { command: 'oga query "tool description strings"' },
        tool_response: { stdout: "web/src/ui/icons.tsx:12#IconTitle (matched: title)", stderr: "" },
      }),
    };
    const expansion = expansionFromEvent(searchEvent);
    expect(expansion).toEqual({
      type: "content",
      hiddenLines: 0,
      language: "plain",
      text: "web/src/ui/icons.tsx:12#IconTitle (matched: title)",
      preview: undefined,
    });
  });

  it("shows an Oga task list as readable bounded content", () => {
    const taskEvent: TaskEventView = {
      ...event(1, "tool", "List tasks"),
      verb: "Listed",
      presentation: { type: "tool", text: "trace presentation", outcome: "2 tasks" },
      rawText: JSON.stringify({
        hook_event_name: "PostToolUse",
        tool_name: "oga_tasks",
        tool_input: { query: "trace presentation" },
        tool_response: {
          content: [{
            type: "text",
            text: JSON.stringify([
              { id: "task-1", title: "Review the trace", state: "running" },
              { id: "task-2", title: "Run the checks", state: "completed" },
            ]),
          }],
        },
      }),
    };

    expect(expansionFromEvent(taskEvent)).toEqual({
      type: "content",
      hiddenLines: 0,
      language: "plain",
      text: "2 tasks\n- Review the trace · in progress\n- Run the checks · complete",
      preview: undefined,
    });
  });

  it("does not expose task IDs in an Oga action result", () => {
    const taskEvent: TaskEventView = {
      ...event(1, "tool", "View task"),
      verb: "Viewed",
      presentation: { type: "tool", text: "selected task" },
      rawText: JSON.stringify({
        tool_name: "oga_inspect",
        tool_input: { taskId: "secret-task-id" },
        tool_response: {
          content: [{
            type: "text",
            text: JSON.stringify({ id: "secret-task-id", state: "completed", output: "finished" }),
          }],
        },
      }),
    };

    const expansion = expansionFromEvent(taskEvent);
    expect(expansion?.type).toBe("content");
    if (expansion?.type !== "content") throw new Error("expected content expansion");
    expect(expansion.text).toBe("Status: complete\nResult: finished");
    expect(expansion.text).not.toContain("secret-task-id");
  });

  it("shows an Oga action error instead of a success message", () => {
    const taskEvent: TaskEventView = {
      ...event(1, "tool", "Confirm task complete"),
      verb: "Confirmed",
      presentation: { type: "tool", text: "selected task" },
      rawText: JSON.stringify({
        tool_name: "oga_complete",
        tool_input: { taskId: "task-1" },
        tool_response: {
          content: [{
            type: "text",
            text: JSON.stringify({ error: { message: "task is already complete" } }),
          }],
        },
      }),
    };

    const expansion = expansionFromEvent(taskEvent);
    expect(expansion?.type).toBe("content");
    if (expansion?.type !== "content") throw new Error("expected content expansion");
    expect(expansion.text).toBe("Error: task is already complete");
  });

  // Claude's own Grep tool reports the same way, without the Bash wrapper.
  it("shows a built-in grep search's matches on expansion", () => {
    const searchEvent: TaskEventView = {
      ...event(1, "tool", "Search code"),
      verb: "Searched",
      presentation: { type: "tool", text: "TaskEventView in rust", outcome: "2 matches" },
      rawText: JSON.stringify({
        hook_event_name: "PostToolUse",
        tool_name: "Grep",
        tool_input: { pattern: "TaskEventView", path: "rust" },
        tool_response: { stdout: "rust/crates/oga-events/src/lib.rs\nrust/crates/oga-http/src/state.rs" },
      }),
    };
    const expansion = expansionFromEvent(searchEvent);
    expect(expansion).toEqual({
      type: "content",
      hiddenLines: 0,
      language: "plain",
      text: "rust/crates/oga-events/src/lib.rs\nrust/crates/oga-http/src/state.rs",
      preview: undefined,
    });
  });

  it("reads a search that matched nothing as a plain empty result, not a blank panel", () => {
    const searchEvent: TaskEventView = {
      ...event(1, "tool", "Search code"),
      verb: "Searched",
      presentation: { type: "tool", text: "nonexistent_symbol" },
      rawText: JSON.stringify({
        hook_event_name: "PostToolUse",
        tool_name: "Bash",
        tool_input: { command: 'rg "nonexistent_symbol"' },
        tool_response: { stdout: "", stderr: "" },
      }),
    };
    const expansion = expansionFromEvent(searchEvent);
    expect(expansion?.type).toBe("content");
    if (expansion?.type === "content") expect(expansion.text).toBe("No matches found.");
  });

  it("reads a file-finding search that found nothing as a plain empty result", () => {
    const findEvent: TaskEventView = {
      ...event(1, "tool", "Find files"),
      verb: "Found",
      presentation: { type: "tool", text: "*.missing" },
      rawText: JSON.stringify({
        hook_event_name: "PostToolUse",
        tool_name: "Glob",
        tool_input: { pattern: "*.missing" },
        tool_response: { stdout: "" },
      }),
    };
    const expansion = expansionFromEvent(findEvent);
    expect(expansion?.type).toBe("content");
    if (expansion?.type === "content") expect(expansion.text).toBe("No files found.");
  });

  it("shows loaded skill instructions as prose without the wrapper", () => {
    const skillEvent: TaskEventView = {
      ...event(1, "tool", "Load skill"),
      presentation: { type: "tool", text: "refactor" },
      rawText: JSON.stringify({
        type: "tool_use",
        part: {
          tool: "skill",
          state: {
            status: "completed",
            input: { name: "refactor" },
            output: '<skill_content name="refactor">\n# Skill: refactor\n\nImprove code structure.\n</skill_content>',
          },
        },
      }),
    };
    const expansion = expansionFromEvent(skillEvent);
    expect(expansion).toEqual({ type: "skill", text: "# Skill: refactor\n\nImprove code structure." });

    const rows = traceRows([skillEvent], false);
    expect(rows[0].target).toBe("refactor");
    expect(rows[0].event?.kind).toBe("tool");
    expect(rows[0].event?.presentation?.type).toBe("tool");
    expect(rows[0].expansion && expansionOffers(rows[0].expansion, skillEvent)).toBe(true);
  });

  it("renders OpenCode todo tool input as a checklist", () => {
    const todoEvent: TaskEventView = {
      ...event(1, "tool", "Todo list"),
      source: "opencode",
      presentation: { type: "todo", completed: 1, total: 3, text: "3 steps" },
      rawText: JSON.stringify({
        type: "tool_use",
        part: {
          type: "tool",
          tool: "todowrite",
          state: {
            status: "completed",
            input: {
              todos: [
                { content: "Read the files", status: "completed", priority: "high" },
                { content: "Fix the palette", status: "in_progress", priority: "medium" },
                { content: "Run the tests", status: "pending", priority: "low" },
              ],
            },
          },
        },
      }),
    };

    expect(expansionFromEvent(todoEvent)).toEqual({
      type: "todo",
      items: [
        { text: "Read the files", status: "completed" },
        { text: "Fix the palette", status: "in_progress" },
        { text: "Run the tests", status: "pending" },
      ],
    });
  });

  it("keeps Codex todo list payloads on the shared checklist path", () => {
    const codexEvent: TaskEventView = {
      ...event(1, "tool", "Todo list"),
      source: "codex",
      presentation: { type: "todo", completed: 1, total: 2, text: "2 steps" },
      rawText: JSON.stringify({
        item: {
          type: "todo_list",
          items: [
            { text: "Read the files", completed: true },
            { text: "Run the tests", completed: false },
          ],
        },
      }),
    };

    expect(expansionFromEvent(codexEvent)).toEqual({
      type: "todo",
      items: [
        { text: "Read the files", status: "completed" },
        { text: "Run the tests", status: "pending" },
      ],
    });
  });

  it("shows only file text from a provider's tagged read output", () => {
    const output = [
      "<path>/Users/malico/desgn/oga/web/src/screens/settings/Settings.tsx</path>",
      "<type>file</type>",
      "<content>",
      "90:     const healthResult = await broker.health();",
      "91:     const health = healthResult.ok ? healthResult.value : undefined;",
      "92:     setState((s) => applyOverview(s, summaryResult.value, projectsResult.value, health));",
    ].join("\n");
    const fileEvent: TaskEventView = {
      ...event(1, "file", "Read file"),
      source: "opencode",
      presentation: { type: "file", path: "/Users/malico/desgn/oga/web/src/screens/settings/Settings.tsx" },
      rawText: JSON.stringify({
        type: "tool_use",
        part: {
          type: "tool",
          tool: "read",
          callID: "call_01a0574dfba97270b64b34c21209c582",
          state: {
            status: "completed",
            input: {
              filePath: "/Users/malico/desgn/oga/web/src/screens/settings/Settings.tsx",
              offset: 90,
              limit: 50,
            },
            output,
          },
        },
      }),
    };

    const expansion = expansionFromEvent(fileEvent);
    expect(expansion?.type).toBe("content");
    if (expansion?.type !== "content") throw new Error("expected content expansion");
    expect(expansion.text).toBe(
      [
        "90:     const healthResult = await broker.health();",
        "91:     const health = healthResult.ok ? healthResult.value : undefined;",
        "92:     setState((s) => applyOverview(s, summaryResult.value, projectsResult.value, health));",
      ].join("\n"),
    );
    expect(expansion.text).not.toMatch(/<\/?[A-Za-z]/);
  });

  it("renders agent text as a message with its text as detail", () => {
    const messageEvent: TaskEventView = { ...event(1, "message", "Agent message"), detail: "Message body" };
    const rows = traceRows([messageEvent], false);
    expect(rows[0].style).toBe("message");
    expect(rows[0].target).toBe("Message body");
  });

  it("offers no expansion for a lifecycle row whose detail is already printed", () => {
    const lifecycleEvent: TaskEventView = {
      ...event(1, "lifecycle", "Something happened"),
      detail: "Short detail already on the row",
    };
    const rows = traceRows([lifecycleEvent], false);
    expect(rows[0].target).toBe("Short detail already on the row");
    expect(rows[0].expansion).toBeUndefined();
    expect(traceRowOffersExpansion(rows[0])).toBe(false);
  });

  it("truncates a long lifecycle detail on the row and keeps the full text for expansion", () => {
    const detail = "line one is quite long on its own\nand keeps going onto a second line entirely";
    const lifecycleEvent: TaskEventView = { ...event(1, "lifecycle", "Something happened"), detail };
    const rows = traceRows([lifecycleEvent], false);
    expect(rows[0].target).toBe(detail.split("\n")[0]);
    expect(rows[0].expansion).toEqual({ type: "detail", text: detail });
    expect(traceRowOffersExpansion(rows[0])).toBe(true);
  });

  it("keeps three reads that share a title as three rows", () => {
    const events = Array.from({ length: 3 }, (_, index) => ({
      ...event(index + 1, "file", "Read file"),
      detail: `src/${index}.rs`,
      verb: "Read",
      actionId: `call_${index}`,
      createdAt: `2026-07-30T15:00:0${index}Z`,
    }));

    expect(traceRows(events, false).map((row) => row.target)).toEqual(["src/0.rs", "src/1.rs", "src/2.rs"]);
  });

  it("marks a started row as running only while the trace is live", () => {
    const started = { ...event(1, "file", "Read file"), phase: "started" as const, detail: "src/app.ts" };

    expect(traceRows([started], true)[0].state).toBe("running");
    expect(traceRows([started], false)[0].state).toBe("done");
  });

  it("keeps a stretch running while one of its calls is still open", () => {
    const narration = { ...event(1, "message", "Agent message"), detail: "Reading the two files." };
    const open = { ...event(2, "file", "Read file"), phase: "started" as const, detail: "a.ts", actionId: "call_a" };
    const closed = { ...open, id: 3, phase: "completed" as const, detail: "b.ts", actionId: "call_b" };

    expect(traceRows([narration, open, closed], true)[0].state).toBe("running");
    expect(traceRows([narration, { ...open, phase: "completed" as const }, closed], true)[0].state).toBe("done");
  });

  it("gives ten thousand events one row each and no more", () => {
    // Every row renames the call it reports. Identity is the call id, so a
    // rename cannot merge two calls, and it cannot split one either.
    const events = Array.from({ length: 10_000 }, (_, index) => {
      const id = index + 1;
      return {
        ...event(id, "file", "Read file"),
        title: `Read file ${id}`,
        detail: `src/${id}.rs`,
        actionId: `call_${Math.floor(index / 2)}`,
      };
    });
    const composition = ActivityStory.compose(events);
    const rows = TraceRowBuilder.rows(composition.blocks, "/repo", false);
    const weight = rows.reduce((sum, row) => sum + traceRowWeight(row), 0);

    expect(rows.length).toBe(5_000);
    expect(weight).toBe(5_000);
  });

  it("does not match longer workspace names when relativizing paths", () => {
    expect(relativePaths("/repo/src /repo2", "/repo")).toBe("src /repo2");
    expect(relativePaths("/repo", "/repo")).toBe(".");
  });

  it("recognizes a file edit nested under part.state.input as a change, not raw payload", () => {
    // Real shape seen across providers: the broker normalizes every tool_use
    // into `part.state.input`, and an edit's arguments arrive camelCased.
    const fileEvent: TaskEventView = {
      ...event(1, "file", "Edit"),
      rawText: JSON.stringify({
        part: {
          tool: "edit",
          state: {
            input: {
              filePath: "/repo/src/app.ts",
              oldString: "const a = 1;",
              newString: "const a = 2;",
            },
          },
        },
      }),
    };
    const expansion = expansionFromEvent(fileEvent);
    expect(expansion?.type).toBe("changes");
    if (expansion?.type === "changes") {
      expect(expansion.change.path).toBe("/repo/src/app.ts");
      expect(expansion.change.blocks[0].some((line) => line.kind === "removed")).toBe(true);
      expect(expansion.change.blocks[0].some((line) => line.kind === "added")).toBe(true);
    }
  });

  it("previews an image read as a data URL, not as text", () => {
    const fileEvent: TaskEventView = {
      ...event(1, "file", "Read"),
      presentation: { type: "file", path: "/repo/assets/logo.png" },
      rawText: JSON.stringify({
        tool_result: {
          content: [{ type: "image", source: { type: "base64", media_type: "image/png", data: "abc123" } }],
        },
      }),
    };
    const expansion = expansionFromEvent(fileEvent);
    expect(expansion?.type).toBe("content");
    if (expansion?.type === "content") {
      expect(expansion.preview).toEqual({ kind: "image", dataUrl: "data:image/png;base64,abc123" });
    }
  });

  // The shape Claude Code actually writes: a PostToolUse hook whose response
  // carries the file's own base64 and mime type, not an API content block.
  it("previews an image read from the hook payload the provider really sends", () => {
    const fileEvent: TaskEventView = {
      ...event(1, "file", "Read"),
      presentation: { type: "file", path: "/repo/.scratch/home.png" },
      rawText: JSON.stringify({
        hook_event_name: "PostToolUse",
        tool_name: "Read",
        tool_input: { file_path: "/repo/.scratch/home.png" },
        tool_response: {
          type: "image",
          file: { base64: "iVBORw0KGgo=", type: "image/png", originalSize: 30925 },
        },
      }),
    };
    const expansion = expansionFromEvent(fileEvent);
    expect(expansion?.type).toBe("content");
    if (expansion?.type === "content") {
      expect(expansion.preview).toEqual({ kind: "image", dataUrl: "data:image/png;base64,iVBORw0KGgo=" });
    }
  });

  it("marks a markdown read for rendered preview, keeping its source as text", () => {
    const fileEvent: TaskEventView = {
      ...event(1, "file", "Read"),
      presentation: { type: "file", path: "/repo/README.md" },
      rawText: JSON.stringify({ content: "# Hello\n\nBody text." }),
    };
    const expansion = expansionFromEvent(fileEvent);
    expect(expansion?.type).toBe("content");
    if (expansion?.type === "content") {
      expect(expansion.preview).toEqual({ kind: "markdown" });
      expect(expansion.text).toBe("# Hello\n\nBody text.");
    }
  });

  it("leaves a plain text read with no special preview", () => {
    const fileEvent: TaskEventView = {
      ...event(1, "file", "Read"),
      presentation: { type: "file", path: "/repo/src/app.ts" },
      rawText: JSON.stringify({ content: "const a = 1;" }),
    };
    const expansion = expansionFromEvent(fileEvent);
    expect(expansion?.type).toBe("content");
    if (expansion?.type === "content") {
      expect(expansion.preview).toBeUndefined();
      expect(expansion.language).toBe("typescript");
    }
  });

  it("falls back to no expansion when a file's contents cannot be loaded", () => {
    const fileEvent: TaskEventView = {
      ...event(1, "file", "Read"),
      presentation: { type: "file", path: "/repo/assets/photo.png" },
      rawText: JSON.stringify({ tool_result: { status: "error", message: "permission denied" } }),
    };
    const expansion = expansionFromEvent(fileEvent);
    expect(expansion).toBeUndefined();
  });

  it("recognizes a Codex apply_patch call as a change", () => {
    const fileEvent: TaskEventView = {
      ...event(1, "file", "Apply patch"),
      rawText: JSON.stringify({
        part: {
          tool: "apply_patch",
          state: {
            input: {
              patchText:
                "*** Begin Patch\n*** Update File: /repo/src/app.ts\n@@\n-old\n+new\n*** End Patch",
            },
          },
        },
      }),
    };
    const expansion = expansionFromEvent(fileEvent);
    expect(expansion?.type).toBe("changes");
    if (expansion?.type === "changes") {
      expect(expansion.change.path).toBe("/repo/src/app.ts");
    }
  });
});

describe("ACP results", () => {
  const block = (text: string) => JSON.stringify({ content: [{ type: "content", content: { type: "text", text } }] });

  it("shows a command's output from the content block it arrived in", () => {
    const run: TaskEventView = {
      ...event(1, "command", "Run command"),
      presentation: { type: "command", command: "git status" },
      rawText: block("```console\nOn branch main\nnothing to commit\n```"),
    };
    expect(expansionFromEvent(run)).toEqual({ type: "command", command: "git status", output: "On branch main\nnothing to commit" });
  });

  it("shows a read file's lines from the content block it arrived in", () => {
    const read: TaskEventView = {
      ...event(2, "file", "Read file"),
      presentation: { type: "file", path: "src/main.rs" },
      rawText: block("```\n1\tfn main() {}\n```"),
    };
    const expansion = expansionFromEvent(read);
    expect(expansion?.type).toBe("content");
    expect(expansion?.type === "content" ? expansion.text : undefined).toBe("1\tfn main() {}");
  });
});

describe("stretches the worker opened with its own words", () => {
  const narration = (id: number, text: string): TaskEventView => ({
    ...event(id, "message", "Agent message"),
    detail: text,
  });

  it("collapses the work under one heading with its call count and duration", () => {
    const [row] = traceRows([
      narration(1, "Running focused tests, type checks, and the configured linter"),
      { ...event(2, "file", "Read file"), actionId: "call_a", presentation: { type: "file", path: "/repo/a.ts" } },
      {
        ...event(3, "command", "Run command"),
        actionId: "call_b",
        createdAt: "2026-07-30T15:00:40Z",
        presentation: { type: "command", command: "bun test" },
      },
    ]);

    expect(row.target).toBe("Running focused tests, type checks, and the configured linter");
    expect(row.result).toBe("2 calls · 40s");
    expect(row.startsExpanded).toBe(false);
    expect(row.children.length).toBe(2);
  });

  it("opens a new stretch each time the worker speaks again", () => {
    const rows = traceRows([
      narration(1, "Reading the toast layer"),
      { ...event(2, "file", "Read file"), actionId: "call_a", presentation: { type: "file", path: "/repo/Toast.tsx" } },
      narration(3, "Raising it above every current overlay"),
      { ...event(4, "file", "Edit file"), actionId: "call_b", presentation: { type: "file", path: "/repo/Toast.tsx" } },
    ]);

    expect(rows.map((row) => row.target)).toEqual([
      "Reading the toast layer",
      "Raising it above every current overlay",
    ]);
    expect(rows.map((row) => row.children.length)).toEqual([1, 1]);
  });
});

describe("receipt rows", () => {
  it("labels and groups token counts with locale separators", () => {
    const [row] = TraceRowBuilder.rows(
      [{
        type: "receipt",
        event: { ...event(1, "message", "Run complete"), presentation: { type: "usage", tokensOut: 1234 } },
        thinkingTokens: 56,
      }],
      "/repo",
      false,
    );

    expect(row.target).toBe("Run complete · 1,234 tokens out · ~56 thinking");
  });
});

describe("thinking rows", () => {
  const rows = (pulse: ReasoningPulse) =>
    TraceRowBuilder.nodeRows({ type: "thinking", id: `thinking:${pulse.id}`, pulse }, "/repo", false);

  it("previews the first line and keeps the rest behind the row", () => {
    const [row] = rows({ id: 1, seconds: 12, text: "First the shape.\n\nThen the cost." });
    expect(row.target).toBe("Thought for 12s");
    expect(row.preview).toBe("First the shape.");
    expect(row.expansion).toEqual({ type: "thinking", text: "First the shape.\n\nThen the cost." });
    expect(traceRowOffersExpansion(row)).toBe(true);
  });

  it("still says the worker thought when it cannot say what about", () => {
    const [row] = rows({ id: 1, seconds: 4, tokens: 1_200 });
    expect(row.target).toBe("Thought for 4s");
    expect(row.result).toBe("~1k tokens");
    expect(row.expansion).toBeUndefined();
  });

  it("drops thinking rows wherever they sit when the reader has not asked for them", () => {
    const built = traceRows([
      { ...event(1, "reasoning", "Thinking"), phase: "info", detail: "Weighing two options" },
      { ...event(2, "message", "Agent message"), detail: "Rewriting the stylesheet" },
      { ...event(3, "reasoning", "Thinking"), phase: "info", detail: "Then the cost" },
      { ...event(4, "file", "Edit file"), actionId: "call_a", presentation: { type: "file" as const, path: "/repo/a.ts" } },
    ]);

    expect(built.some((row) => row.isThinking === true)).toBe(true);
    expect(withoutThinking(built).flatMap(flatIds)).toEqual([2, 4]);
  });
});

function flatIds(row: TraceRow): number[] {
  return [row.id, ...row.children.flatMap(flatIds)];
}
