import type { UsageDay } from "@/bridge/types";

export interface HeatmapCell {
  date: string;
  day: UsageDay | undefined;
  inMonth: boolean;
}

export interface HeatmapMonth {
  year: number;
  month: number; // 1-12
  /** One row per week, seven cells Sunday–Saturday. */
  weeks: HeatmapCell[][];
}

export interface MonthKey {
  year: number;
  month: number; // 1-12
}

export interface MonthRange {
  min: MonthKey;
  max: MonthKey;
}

const MONTH_NAMES = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
];

export function monthLabel(month: MonthKey): string {
  return `${MONTH_NAMES[month.month - 1]} ${month.year}`;
}

/** Local today as `YYYY-MM-DD`. */
export function localToday(): string {
  const now = new Date();
  return formatDate(now.getFullYear(), now.getMonth() + 1, now.getDate());
}

function formatDate(year: number, month: number, date: number): string {
  return `${String(year).padStart(4, "0")}-${String(month).padStart(2, "0")}-${String(date).padStart(2, "0")}`;
}

export function parseLocal(date: string): Date | undefined {
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

export function toKey(date: Date): string {
  return formatDate(date.getFullYear(), date.getMonth() + 1, date.getDate());
}

function monthOf(date: Date): MonthKey {
  return { year: date.getFullYear(), month: date.getMonth() + 1 };
}

export function addMonths(month: MonthKey, delta: number): MonthKey {
  const total = month.year * 12 + (month.month - 1) + delta;
  return { year: Math.floor(total / 12), month: (((total % 12) + 12) % 12) + 1 };
}

export function compareMonths(a: MonthKey, b: MonthKey): number {
  return (a.year * 12 + a.month) - (b.year * 12 + b.month);
}

/**
 * Months a person can navigate to: from the earliest activity (or `start`)
 * through today (or `end`, whichever is later), so paging can never reach
 * an always-empty future month.
 */
export function heatmapMonthBounds(
  days: UsageDay[],
  opts?: { start?: string; end?: string; today?: string },
): MonthRange {
  const today = opts?.today ?? localToday();
  const sorted = [...new Set(days.map((day) => day.date))]
    .filter((date) => parseLocal(date) !== undefined)
    .sort();
  const rawStart = opts?.start ?? sorted[0] ?? today;
  const rawEnd = opts?.end ?? sorted[sorted.length - 1] ?? today;
  const endKey = rawEnd >= today ? rawEnd : today;
  const startKey = rawStart <= endKey ? rawStart : endKey;

  const endDate = parseLocal(endKey) ?? parseLocal(today);
  const startDate = parseLocal(startKey) ?? endDate;
  if (endDate === undefined || startDate === undefined) {
    const fallback = monthOf(new Date());
    return { min: fallback, max: fallback };
  }
  return { min: monthOf(startDate), max: monthOf(endDate) };
}

export function clampMonth(month: MonthKey, bounds: MonthRange): MonthKey {
  if (compareMonths(month, bounds.min) < 0) return bounds.min;
  if (compareMonths(month, bounds.max) > 0) return bounds.max;
  return month;
}

/**
 * Full calendar grid for one month: every day of the month plus the
 * leading/trailing days needed to pad out to whole Sunday–Saturday weeks.
 */
export function buildHeatmapMonth(days: UsageDay[], month: MonthKey): HeatmapMonth {
  const byDate = new Map(days.map((day) => [day.date, day]));
  const firstOfMonth = new Date(month.year, month.month - 1, 1);
  const lastOfMonth = new Date(month.year, month.month, 0);

  const gridStart = new Date(firstOfMonth);
  gridStart.setDate(gridStart.getDate() - gridStart.getDay());
  const gridEnd = new Date(lastOfMonth);
  gridEnd.setDate(gridEnd.getDate() + (6 - gridEnd.getDay()));

  const weeks: HeatmapCell[][] = [];
  const cursor = new Date(gridStart);
  while (cursor <= gridEnd) {
    const week: HeatmapCell[] = [];
    for (let i = 0; i < 7; i += 1) {
      const key = toKey(cursor);
      week.push({ date: key, day: byDate.get(key), inMonth: cursor.getMonth() === month.month - 1 });
      cursor.setDate(cursor.getDate() + 1);
    }
    weeks.push(week);
  }
  return { year: month.year, month: month.month, weeks };
}
