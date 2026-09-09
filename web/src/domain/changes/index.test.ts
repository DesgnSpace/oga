// Ported from rust/crates/oga-ui/src/changes/mod.rs `#[cfg(test)] mod tests`.

import { describe, expect, it } from "bun:test";
import type { EventKind, TaskEventView } from "@/bridge/types";
import {
  collectRunChanges,
  RunChangeProjection,
  fileChangeAdded,
  fileChangeFromRaw,
  fileChangeMayContainEdit,
  fileChangeRemoved,
  diffLines,
  relativePath,
  RUN_CHANGES_LINE_LIMIT,
  runChangeSetIsEmpty,
} from "./index";

function event(id: number, kind: EventKind, raw: string): TaskEventView {
  return {
    id,
    taskId: "task",
    source: "claude",
    type: "agent.tool",
    kind,
    phase: "completed",
    title: "Edit",
    rawText: raw,
    createdAt: "2026-08-07T00:00:00Z",
  };
}

function edit(path: string, oldText: string, newText: string): string {
  return JSON.stringify({
    tool_name: "Edit",
    tool_input: { file_path: path, old_string: oldText, new_string: newText },
  });
}

describe("fileChangeFromRaw", () => {
  it("recovers nested edit and diff", () => {
    const change = fileChangeFromRaw(edit("/repo/app.ts", "old", "new"));
    expect(change?.path).toBe("/repo/app.ts");
    expect(fileChangeAdded(change!)).toBe(1);
    expect(fileChangeRemoved(change!)).toBe(1);
  });

  it("keeps each replacement in a multi-edit in order", () => {
    const raw = JSON.stringify({
      tool_name: "MultiEdit",
      tool_input: {
        file_path: "/repo/app.ts",
        edits: [
          { old_string: "let a = 1;", new_string: "let a = 2;" },
          { old_string: "debugger;", new_string: "" },
        ],
      },
    });
    const change = fileChangeFromRaw(raw);
    expect(change?.path).toBe("/repo/app.ts");
    expect(change?.blocks.length).toBe(2);
    expect(fileChangeAdded(change!)).toBe(1);
    expect(fileChangeRemoved(change!)).toBe(2);
  });

  it("treats writes as added and reads as not changes", () => {
    const write = JSON.stringify({ tool_input: { file_path: "new.ts", content: "one\ntwo\n" } });
    const change = fileChangeFromRaw(write);
    expect(fileChangeAdded(change!)).toBe(2);
    const read = JSON.stringify({ tool_input: { file_path: "old.ts" }, tool_response: { content: "one" } });
    expect(fileChangeFromRaw(read)).toBeUndefined();
  });

  it("uses structuredPatch when edit arguments are missing", () => {
    const raw = JSON.stringify({
      tool_response: {
        filePath: "/repo/app.ts",
        structuredPatch: [{ lines: [" const a = 1;", "-const b = 2;", "+const b = 3;", " const c = 4;"] }],
      },
    });
    const change = fileChangeFromRaw(raw);
    expect(change?.path).toBe("/repo/app.ts");
    expect(change?.blocks[0][0].kind).toBe("context");
    expect(change?.blocks[0][1].kind).toBe("removed");
    expect(change?.blocks[0][2].kind).toBe("added");
  });

  it("recovers an edit nested under part.state.input", () => {
    // Shape stored for every provider's tool_use events: the broker normalizes
    // tool calls into `part.state.input` regardless of which CLI produced them.
    const raw = JSON.stringify({
      type: "tool_use",
      part: {
        type: "tool",
        tool: "edit",
        state: {
          status: "completed",
          input: {
            filePath: "/repo/web/src/screens/settings/Settings.tsx",
            oldString: "const enabled = e.target.checked;",
            newString: "const enabled = e.currentTarget.checked;",
          },
        },
      },
    });
    const change = fileChangeFromRaw(raw);
    expect(change?.path).toBe("/repo/web/src/screens/settings/Settings.tsx");
    expect(fileChangeAdded(change!)).toBe(1);
    expect(fileChangeRemoved(change!)).toBe(1);
  });

  it("recovers a Codex apply_patch call from patchText", () => {
    const raw = JSON.stringify({
      type: "tool_use",
      part: {
        type: "tool",
        tool: "apply_patch",
        state: {
          status: "completed",
          input: {
            patchText:
              "*** Begin Patch\n" +
              "*** Update File: /repo/rust/crates/oga-ui/src/state/mod.rs\n" +
              "@@\n" +
              "-#[derive(Debug, Clone, Default, PartialEq, Eq)]\n" +
              "+#[derive(Debug, Clone, PartialEq, Eq)]\n" +
              " pub struct SidebarPreferences {\n" +
              "*** End Patch",
          },
        },
      },
    });
    const change = fileChangeFromRaw(raw);
    expect(change?.path).toBe("/repo/rust/crates/oga-ui/src/state/mod.rs");
    expect(fileChangeRemoved(change!)).toBe(1);
    expect(fileChangeAdded(change!)).toBe(1);
    expect(change?.blocks[0].some((line) => line.kind === "context")).toBe(true);
  });

  it("recovers a Codex apply_patch file addition from patchText", () => {
    const raw = JSON.stringify({
      part: {
        tool: "apply_patch",
        state: {
          input: {
            patchText:
              "*** Begin Patch\n*** Add File: /repo/landing/public/styles.css\n+:root {\n+  --ink: #161616;\n+}\n*** End Patch",
          },
        },
      },
    });
    const change = fileChangeFromRaw(raw);
    expect(change?.path).toBe("/repo/landing/public/styles.css");
    expect(fileChangeAdded(change!)).toBe(3);
    expect(fileChangeRemoved(change!)).toBe(0);
  });

  it("recovers a unified patch from nested result details", () => {
    const raw = JSON.stringify({
      result: {
        details: {
          patch:
            "--- .oga-test/pi-diff.txt\n+++ .oga-test/pi-diff.txt\n@@ -1,1 +1,1 @@\n-status: before\n+status: after\n",
        },
      },
    });
    const change = fileChangeFromRaw(raw);
    expect(change?.path).toBe(".oga-test/pi-diff.txt");
    expect(fileChangeAdded(change!)).toBe(1);
    expect(fileChangeRemoved(change!)).toBe(1);
  });
});

describe("fileChangeMayContainEdit", () => {
  it("matches supported payloads", () => {
    expect(fileChangeMayContainEdit('{"tool_input":{"old_string":"a","new_string":"b"}}')).toBe(true);
    expect(fileChangeMayContainEdit('{"patch":"@@ -1,1 +1,1 @@"}')).toBe(true);
    expect(fileChangeMayContainEdit('{"patchText":"*** Begin Patch\\n*** Update File: a\\n@@\\n-a\\n+b\\n*** End Patch"}')).toBe(
      true,
    );
    expect(fileChangeMayContainEdit('{"tool_input":{"file_path":"/repo/app.ts"}}')).toBe(false);
  });
});

describe("diffLines", () => {
  it("groups removals before additions", () => {
    expect(diffLines("a\nb\nkeep", "x\ny\nkeep").map((line) => line.kind)).toEqual([
      "removed",
      "removed",
      "added",
      "added",
      "context",
    ]);
  });
});

describe("collectRunChanges", () => {
  it("shares a file across repeated edits and ignores duplicate payloads", () => {
    const first = edit("src/a.ts", "one", "ONE");
    const set = collectRunChanges([event(1, "tool", first), event(2, "tool", first)], "/repo");
    expect(set.files.length).toBe(1);
    expect(set.files[0].edits).toBe(1);
    expect(set.files[0].path).toBe("src/a.ts");
  });

  it("merges absolute and relative paths into one file", () => {
    const set = collectRunChanges(
      [
        event(1, "tool", edit("/repo/src/a.ts", "one", "ONE")),
        event(2, "tool", edit("src/a.ts", "two", "TWO")),
      ],
      "/repo",
    );
    expect(set.files[0].path).toBe("src/a.ts");
    expect(set.files[0].edits).toBe(2);
  });

  it("keeps files in first-touch order", () => {
    const set = collectRunChanges(
      [
        event(1, "tool", edit("z.ts", "a", "b")),
        event(2, "tool", edit("a.ts", "c", "d")),
        event(3, "tool", edit("z.ts", "e", "f")),
      ],
      "/repo",
    );
    expect(set.files.map((file) => file.path)).toEqual(["z.ts", "a.ts"]);
  });

  it("does not count retry payloads", () => {
    const raw = edit("src/a.ts", "one", "ONE");
    expect(runChangeSetIsEmpty(collectRunChanges([event(1, "retry", raw)], "/repo"))).toBe(true);
  });

  it("counts changes without paths as unmatched", () => {
    const set = collectRunChanges(
      [
        event(1, "tool", JSON.stringify({ tool_input: { old_string: "x", new_string: "y" } })),
        event(2, "tool", edit("a.ts", "one", "ONE")),
      ],
      "/repo",
    );
    expect(set.unmatched).toBe(1);
    expect(set.files[0].path).toBe("a.ts");
  });

  it("treats whole-file writes as all added", () => {
    const raw = JSON.stringify({ tool_name: "Write", tool_input: { file_path: "src/new.ts", content: "one\ntwo" } });
    const set = collectRunChanges([event(1, "file", raw)], "/repo");
    expect(set.files[0].path).toBe("src/new.ts");
    expect(set.files[0].added).toBe(2);
    expect(set.files[0].removed).toBe(0);
  });

  it("treats deleting all content as all removed", () => {
    const set = collectRunChanges([event(1, "tool", edit("src/gone.ts", "one\ntwo", ""))], "/repo");
    expect(set.files[0].added).toBe(0);
    expect(set.files[0].removed).toBe(2);
  });

  it("flags shortened payloads", () => {
    const raw = edit("src/huge.ts", "before", "after …[truncated: kept 6 of 900000 bytes]");
    const set = collectRunChanges([event(1, "tool", raw)], "/repo");
    expect(set.files[0].shortened).toBe(true);
  });

  it("keeps whole blocks and counts hidden lines for long stacks", () => {
    const events = Array.from({ length: 40 }, (_, id) => {
      const old = Array.from({ length: 20 }, (_, line) => `old ${id}-${line}`).join("\n");
      const next = Array.from({ length: 20 }, (_, line) => `new ${id}-${line}`).join("\n");
      return event(id, "tool", edit("src/big.ts", old, next));
    });
    const set = collectRunChanges(events, "/repo");
    const file = set.files[0];
    const drawn = file.change.blocks.reduce((sum, block) => sum + block.length, 0);
    expect(file.added).toBe(800);
    expect(file.removed).toBe(800);
    expect(drawn + file.hiddenLines).toBe(1600);
    expect(drawn).toBeGreaterThanOrEqual(RUN_CHANGES_LINE_LIMIT);
    expect(drawn).toBeLessThan(RUN_CHANGES_LINE_LIMIT + 40);
  });
});

describe("RunChangeProjection", () => {
  it("matches a full rebuild after an append and keeps the file view", () => {
    const events = [event(1, "tool", edit("src/a.ts", "one", "ONE"))];
    const projection = new RunChangeProjection();
    const first = projection.update(events, "/repo");
    events.push(event(2, "tool", edit("src/a.ts", "two", "TWO")));

    const incremental = projection.update(events, "/repo");
    expect(incremental).toEqual(collectRunChanges(events, "/repo"));
    expect(incremental.files[0]).toBe(first.files[0]);
  });

  it("rebuilds after an earlier page replaces the event array", () => {
    const recent = [event(2, "tool", edit("src/a.ts", "two", "TWO"))];
    const projection = new RunChangeProjection();
    projection.update(recent, "/repo");

    const complete = [event(1, "tool", edit("src/a.ts", "one", "ONE")), ...recent];
    expect(projection.update(complete, "/repo")).toEqual(collectRunChanges(complete, "/repo"));
  });
});

describe("relativePath", () => {
  it("does not confuse sibling directories", () => {
    expect(relativePath("/repo-backup/a", "/repo")).toBe("/repo-backup/a");
    expect(relativePath("/repo/a", "/repo/")).toBe("a");
  });
});
