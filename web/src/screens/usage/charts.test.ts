import { describe, expect, test } from "bun:test";
import { buildDailySeries, topBreakdown } from "./charts";
import type { UsageBreakdown, UsageDay } from "@/bridge/types";

function day(date: string, costUsd: number): UsageDay {
  return { date, costUsd, tokens: costUsd * 100, tasks: 1 };
}

function row(profile: string, costUsd: number): UsageBreakdown {
  return { provider: "anthropic", profile, model: "claude", costUsd, tokens: costUsd * 100, tasks: 1 };
}

describe("buildDailySeries", () => {
  test("zero-fills days with no activity across the range", () => {
    const series = buildDailySeries([day("2026-09-02", 5)], "2026-09-01", "2026-09-03");

    expect(series.map((point) => point.date)).toEqual(["2026-09-01", "2026-09-02", "2026-09-03"]);
    expect(series[0].costUsd).toBe(0);
    expect(series[1].costUsd).toBe(5);
    expect(series[2].costUsd).toBe(0);
  });

  test("falls back to today with no bounds or activity", () => {
    const series = buildDailySeries([]);

    expect(series.length).toBe(1);
    expect(series[0].costUsd).toBe(0);
  });
});

describe("topBreakdown", () => {
  test("folds rows past the limit into one Other bar", () => {
    const bars = topBreakdown([row("a", 3), row("b", 5), row("c", 1)], 2);

    expect(bars).toEqual([
      { label: "b", costUsd: 5 },
      { label: "a", costUsd: 3 },
      { label: "Other", costUsd: 1 },
    ]);
  });

  test("returns every row untouched when under the limit", () => {
    const bars = topBreakdown([row("a", 3)], 8);

    expect(bars).toEqual([{ label: "a", costUsd: 3 }]);
  });
});
