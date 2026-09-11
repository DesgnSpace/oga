import type { UsageBreakdown, UsageDay } from "@/bridge/types";
import { localToday, parseLocal, toKey } from "./heatmap";

export interface DailyPoint {
  date: string;
  costUsd: number;
  tokens: number;
}

/** Every day from `start` to `end` inclusive, zero-filled where no activity exists. */
export function buildDailySeries(days: UsageDay[], start?: string, end?: string): DailyPoint[] {
  const byDate = new Map(days.map((day) => [day.date, day]));
  const sorted = [...new Set(days.map((day) => day.date))]
    .filter((date) => parseLocal(date) !== undefined)
    .sort();
  const today = localToday();
  const startDate = parseLocal(start ?? sorted[0] ?? today) ?? parseLocal(today);
  const endDate = parseLocal(end ?? sorted[sorted.length - 1] ?? today) ?? parseLocal(today);
  if (startDate === undefined || endDate === undefined) return [];

  const points: DailyPoint[] = [];
  const cursor = new Date(startDate);
  while (cursor <= endDate) {
    const key = toKey(cursor);
    const day = byDate.get(key);
    points.push({ date: key, costUsd: day?.costUsd ?? 0, tokens: day?.tokens ?? 0 });
    cursor.setDate(cursor.getDate() + 1);
  }
  return points;
}

export interface ModelBar {
  label: string;
  costUsd: number;
}

/** The top `limit` workers by cost, with the remainder folded into one "Other" bar. */
export function topBreakdown(rows: UsageBreakdown[], limit: number): ModelBar[] {
  const sorted = [...rows].sort((a, b) => b.costUsd - a.costUsd);
  const head = sorted.slice(0, limit).map((row) => ({ label: row.profile, costUsd: row.costUsd }));
  const rest = sorted.slice(limit);
  if (rest.length === 0) return head;
  const otherCost = rest.reduce((sum, row) => sum + row.costUsd, 0);
  return [...head, { label: "Other", costUsd: otherCost }];
}
