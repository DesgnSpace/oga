import { describe, expect, test } from "bun:test";
import { buildInlineDiffRanges } from "./inline-diff";

const lines = (...items: ["removed" | "added" | "context", string][]) => [items.map(([kind, text]) => ({ kind, text }))];

describe("buildInlineDiffRanges", () => {
  test("highlights a simple word change", () => {
    expect(buildInlineDiffRanges(lines(["removed", "const color = red;"], ["added", "const color = blue;"]))[0]).toEqual([
      { before: { start: 14, end: 17 }, after: { start: 14, end: 18 } },
      { before: { start: 14, end: 17 }, after: { start: 14, end: 18 } },
    ]);
  });

  test("highlights whitespace-only changes", () => {
    expect(buildInlineDiffRanges(lines(["removed", "one two"], ["added", "one  two"]))[0]?.[0]).toEqual({
      before: { start: 3, end: 4 }, after: { start: 3, end: 5 },
    });
  });

  test("skips rewrites below the sharing threshold", () => {
    expect(buildInlineDiffRanges(lines(["removed", "old words here"], ["added", "completely new text"]))[0]).toEqual([undefined]);
  });

  test("pairs only the aligned part of uneven runs", () => {
    expect(buildInlineDiffRanges(lines(
      ["removed", "keep old"], ["removed", "another old"], ["added", "keep new"],
    ))[0]).toHaveLength(3);
    expect(buildInlineDiffRanges(lines(
      ["removed", "keep old"], ["removed", "another old"], ["added", "keep new"],
    ))[0]?.[1]).toBeUndefined();
  });
});
