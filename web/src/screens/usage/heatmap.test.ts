import { describe, expect, test } from "bun:test";
import { addMonths, buildHeatmapMonth, compareMonths, heatmapMonthBounds, monthLabel } from "./heatmap";
import type { UsageDay } from "@/bridge/types";

function day(date: string): UsageDay {
  return { date, costUsd: 1, tokens: 10, tasks: 1 };
}

describe("buildHeatmapMonth", () => {
  test("draws every day of the month padded to whole Sunday–Saturday weeks", () => {
    const calendar = buildHeatmapMonth([], { year: 2026, month: 9 });

    for (const week of calendar.weeks) expect(week.length).toBe(7);
    expect(calendar.weeks[0][0].date).toBe("2026-08-30");
    expect(calendar.weeks.at(-1)?.[6].date).toBe("2026-10-03");
    const inMonth = calendar.weeks.flat().filter((cell) => cell.inMonth).map((cell) => cell.date);
    expect(inMonth[0]).toBe("2026-09-01");
    expect(inMonth.at(-1)).toBe("2026-09-30");
    expect(inMonth.length).toBe(30);
  });

  test("draws a full empty grid when no day in the month has activity", () => {
    const calendar = buildHeatmapMonth([], { year: 2026, month: 9 });

    const inMonthCells = calendar.weeks.flat().filter((cell) => cell.inMonth);
    expect(inMonthCells.every((cell) => cell.day === undefined)).toBe(true);
  });

  test("attaches activity to the matching day, leaves padding days undefined", () => {
    const calendar = buildHeatmapMonth([day("2026-09-02")], { year: 2026, month: 9 });

    const active = calendar.weeks.flat().find((cell) => cell.date === "2026-09-02");
    expect(active?.day).toEqual(day("2026-09-02"));
    const padding = calendar.weeks.flat().find((cell) => cell.date === "2026-08-30");
    expect(padding?.day).toBeUndefined();
    expect(padding?.inMonth).toBe(false);
  });
});

describe("heatmapMonthBounds", () => {
  test("spans from the earliest activity through today", () => {
    const bounds = heatmapMonthBounds([day("2026-07-15")], { today: "2026-09-03" });

    expect(bounds.min).toEqual({ year: 2026, month: 7 });
    expect(bounds.max).toEqual({ year: 2026, month: 9 });
  });

  test("falls back to the current month with no activity or explicit range", () => {
    const bounds = heatmapMonthBounds([], { today: "2026-09-03" });

    expect(bounds.min).toEqual({ year: 2026, month: 9 });
    expect(bounds.max).toEqual({ year: 2026, month: 9 });
  });

  test("never extends the max past today, even if a stale end predates it", () => {
    const bounds = heatmapMonthBounds([], { start: "2026-07-01", end: "2026-07-15", today: "2026-09-03" });

    expect(bounds.max).toEqual({ year: 2026, month: 9 });
  });
});

describe("month arithmetic", () => {
  test("addMonths steps across year boundaries", () => {
    expect(addMonths({ year: 2026, month: 1 }, -1)).toEqual({ year: 2025, month: 12 });
    expect(addMonths({ year: 2026, month: 12 }, 1)).toEqual({ year: 2027, month: 1 });
  });

  test("compareMonths orders chronologically", () => {
    expect(compareMonths({ year: 2026, month: 9 }, { year: 2026, month: 10 })).toBeLessThan(0);
    expect(compareMonths({ year: 2026, month: 9 }, { year: 2026, month: 9 })).toBe(0);
  });

  test("monthLabel names the month and year", () => {
    expect(monthLabel({ year: 2026, month: 9 })).toBe("September 2026");
  });
});
