import { describe, expect, test } from "bun:test";
import { formatCost, formatDuration, formatTokenCount, taskDuration } from "./format";

describe("formatCost", () => {
  test("hides free and unknown cost", () => {
    expect(formatCost(undefined)).toBeUndefined();
    expect(formatCost(0)).toBeUndefined();
  });

  test("shows sub-cent cost as a floor", () => {
    expect(formatCost(0.001)).toBe("<$0.01");
  });

  test("formats cost to two decimals", () => {
    expect(formatCost(0.42)).toBe("$0.42");
    expect(formatCost(12)).toBe("$12.00");
  });

  test("prefixes estimated cost with a tilde", () => {
    expect(formatCost(0.42, true)).toBe("~$0.42");
    expect(formatCost(0.001, true)).toBe("~<$0.01");
    expect(formatCost(undefined, true)).toBeUndefined();
  });
});

describe("formatTokenCount", () => {
  test("keeps small counts exact", () => {
    expect(formatTokenCount(842)).toBe("842");
  });

  test("abbreviates thousands", () => {
    expect(formatTokenCount(15_400)).toBe("15k");
  });

  test("abbreviates millions", () => {
    expect(formatTokenCount(2_300_000)).toBe("2.3M");
  });
});

describe("formatDuration", () => {
  test("floors sub-second durations to 0s", () => {
    expect(formatDuration(400)).toBe("0s");
  });

  test("formats seconds", () => {
    expect(formatDuration(45_000)).toBe("45s");
  });

  test("formats minutes and seconds", () => {
    expect(formatDuration(125_000)).toBe("2m 5s");
  });

  test("formats hours and minutes", () => {
    expect(formatDuration(3 * 3_600_000 + 5 * 60_000)).toBe("3h 5m");
  });
});

describe("taskDuration", () => {
  test("keeps only time already spent in worker runs", () => {
    expect(taskDuration(30_000, undefined, false)).toBe("30s");
  });

  test("adds the active run to completed run time", () => {
    const start = new Date(Date.now() - 5_000).toISOString();
    const result = taskDuration(0, start, true);
    expect(result).toBe("5s");
  });

  test("ignores an invalid active run timestamp", () => {
    expect(taskDuration(30_000, "not-a-date", true)).toBe("30s");
  });
});
