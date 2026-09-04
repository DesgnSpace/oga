import { describe, expect, test } from "bun:test";
import { buildHeatmapCalendar } from "./heatmap";
import type { UsageDay } from "@/bridge/types";

function day(date: string): UsageDay {
  return { date, costUsd: 1, tokens: 10, tasks: 1 };
}

describe("buildHeatmapCalendar", () => {
  test("pads a single-day period out to whole Sunday–Saturday weeks", () => {
    const calendar = buildHeatmapCalendar([], {
      start: "2026-09-03",
      end: "2026-09-03",
      today: "2026-09-03",
    });

    expect(calendar.weeks.length).toBe(2);
    for (const week of calendar.weeks) expect(week.length).toBe(7);
    expect(calendar.weeks[0][0].date).toBe("2026-08-23");
    expect(calendar.weeks[1][6].date).toBe("2026-09-05");
    // Only the period day itself counts as in-period.
    const inPeriod = calendar.weeks.flat().filter((cell) => cell.inPeriod).map((cell) => cell.date);
    expect(inPeriod).toEqual(["2026-09-03"]);
  });

  test("stretches short periods to seven days ending today", () => {
    const calendar = buildHeatmapCalendar([day("2026-09-02")], {
      start: "2026-09-02",
      end: "2026-09-02",
      today: "2026-09-03",
    });

    const dates = calendar.weeks.flat().map((cell) => cell.date);
    expect(dates).toContain("2026-09-03");
    expect(dates.length).toBeGreaterThanOrEqual(7);
    const active = calendar.weeks.flat().find((cell) => cell.date === "2026-09-02");
    expect(active?.day).toEqual(day("2026-09-02"));
    expect(active?.inPeriod).toBe(true);
    // Today is outside the period but still renders as an empty cell.
    const todayCell = calendar.weeks.flat().find((cell) => cell.date === "2026-09-03");
    expect(todayCell?.day).toBeUndefined();
    expect(todayCell?.inPeriod).toBe(false);
  });

  test("spans a long period from its first day with Sunday columns", () => {
    const calendar = buildHeatmapCalendar([day("2026-07-15"), day("2026-09-02")], {
      start: "2026-07-15",
      end: "2026-09-03",
      today: "2026-09-03",
    });

    expect(calendar.weeks[0][0].date).toBe("2026-07-12");
    for (const week of calendar.weeks) {
      expect(new Date(`${week[0].date}T00:00:00`).getDay()).toBe(0);
      expect(week.length).toBe(7);
    }
    const dates = calendar.weeks.flat().map((cell) => cell.date);
    expect(dates).toContain("2026-07-15");
    expect(dates).toContain("2026-09-03");
  });

  test("labels the column where a month starts", () => {
    const calendar = buildHeatmapCalendar([], {
      start: "2026-09-03",
      end: "2026-09-03",
      today: "2026-09-03",
    });

    expect(calendar.monthLabels).toEqual([undefined, "Sep"]);
  });

  test("falls back to active days and today without bounds", () => {
    const calendar = buildHeatmapCalendar([day("2026-09-01")], { today: "2026-09-03" });

    const dates = calendar.weeks.flat().map((cell) => cell.date);
    expect(dates).toContain("2026-09-01");
    expect(dates).toContain("2026-09-03");
    expect(dates.length).toBeGreaterThanOrEqual(7);
  });
});
