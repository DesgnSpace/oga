import type { UsageDay } from "@/bridge/types";

export interface HeatmapCell {
  date: string;
  day: UsageDay | undefined;
  inPeriod: boolean;
}

export interface HeatmapCalendar {
  /** One column per week, seven cells Sunday–Saturday. */
  weeks: HeatmapCell[][];
  /** Short month name at the column where a month starts. */
  monthLabels: Array<string | undefined>;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** Local today as `YYYY-MM-DD`. */
export function localToday(): string {
  const now = new Date();
  return formatDate(now.getFullYear(), now.getMonth() + 1, now.getDate());
}

function formatDate(year: number, month: number, date: number): string {
  return `${String(year).padStart(4, "0")}-${String(month).padStart(2, "0")}-${String(date).padStart(2, "0")}`;
}

function parseLocal(date: string): Date | undefined {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!match) return undefined;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const check = new Date(year, month - 1, day);
  if (check.getFullYear() !== year || check.getMonth() !== month - 1 || check.getDate() !== day) {
    return undefined;
  }
  return check;
}

function toKey(date: Date): string {
  return formatDate(date.getFullYear(), date.getMonth() + 1, date.getDate());
}

/**
 * Full-calendar grid for the heatmap. Spans `start`–`end` (falling back to
 * the active days, then today), stretched to at least seven days ending
 * today, padded out to whole Sunday–Saturday weeks. Date math steps whole
 * calendar days so DST changes cannot shift the grid.
 */
export function buildHeatmapCalendar(
  days: UsageDay[],
  opts?: { start?: string; end?: string; today?: string },
): HeatmapCalendar {
  const today = opts?.today ?? localToday();
  const sorted = [...new Set(days.map((day) => day.date))]
    .filter((date) => parseLocal(date) !== undefined)
    .sort();
  const rawStart = opts?.start ?? sorted[0] ?? today;
  const rawEnd = opts?.end ?? sorted[sorted.length - 1] ?? today;
  // Short periods still reach today so the grid never collapses.
  const endKey = rawEnd >= today ? rawEnd : today;
  const startKey = rawStart <= endKey ? rawStart : endKey;

  const endDate = parseLocal(endKey) ?? parseLocal(today);
  if (endDate === undefined) return { weeks: [], monthLabels: [] };
  const parsedStart = parseLocal(startKey);
  let startDate = parsedStart ?? new Date(endDate);
  const minStart = new Date(endDate);
  minStart.setDate(minStart.getDate() - 6);
  if (startDate > minStart) startDate = minStart;

  // Pad to whole weeks, Sunday first.
  const gridStart = new Date(startDate);
  gridStart.setDate(gridStart.getDate() - gridStart.getDay());
  const gridEnd = new Date(endDate);
  gridEnd.setDate(gridEnd.getDate() + (6 - gridEnd.getDay()));

  const byDate = new Map(days.map((day) => [day.date, day]));
  const weeks: HeatmapCell[][] = [];
  const cursor = new Date(gridStart);
  while (cursor <= gridEnd) {
    const week: HeatmapCell[] = [];
    for (let i = 0; i < 7; i += 1) {
      const key = toKey(cursor);
      week.push({ date: key, day: byDate.get(key), inPeriod: key >= rawStart && key <= rawEnd });
      cursor.setDate(cursor.getDate() + 1);
    }
    weeks.push(week);
  }

  const monthLabels = weeks.map((week) => {
    const first = week.find((cell) => cell.date.slice(8) === "01");
    return first === undefined ? undefined : MONTHS[Number(first.date.slice(5, 7)) - 1];
  });
  return { weeks, monthLabels };
}
