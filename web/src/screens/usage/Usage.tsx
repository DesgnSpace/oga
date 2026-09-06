import { useEffect, useMemo, useState } from "react";
import { broker } from "@/bridge/client";
import type { UsageBreakdown, UsageDay, UsagePeriod, UsageResponse } from "@/bridge/types";
import { formatCost, formatTokenCount } from "@/lib/format";
import { BackArrowIcon, ForwardArrowIcon } from "@/ui/icons";
import {
  addMonths,
  buildHeatmapMonth,
  clampMonth,
  compareMonths,
  heatmapMonthBounds,
  monthLabel,
  type HeatmapCell,
  type MonthKey,
} from "./heatmap";

type UsageTab = "overview" | "models";
type ModelSort = "cost" | "tokens" | "tasks";

const TABS: ReadonlyArray<{ id: UsageTab; label: string }> = [
  { id: "overview", label: "Overview" },
  { id: "models", label: "Models" },
];

export default function UsagePage() {
  const [data, setData] = useState<UsageResponse | undefined>();
  const [error, setError] = useState<string>();
  const [tab, setTab] = useState<UsageTab>("overview");
  const [periodId, setPeriodId] = useState<string>("all");
  const [sort, setSort] = useState<ModelSort>("cost");

  useEffect(() => {
    void broker.usage(-new Date().getTimezoneOffset()).then((result) => {
      if (result.ok) setData(result.value);
      else setError(result.error.message);
    });
  }, []);

  if (error) {
    return (
      <div className="usage-page">
        <UsageHeader />
        <p role="alert">Couldn&apos;t load usage. Try again later.</p>
      </div>
    );
  }
  if (!data) {
    return (
      <div className="usage-page" aria-busy="true" aria-label="Loading usage…">
        <UsageHeader />
        <div className="usage-skeleton" />
      </div>
    );
  }
  const period = data.periods.find((candidate) => candidate.id === periodId) ?? data.periods[0];
  if (!period) {
    return (
      <div className="usage-page">
        <UsageHeader />
        <p className="usage-muted">No task usage yet.</p>
      </div>
    );
  }
  return (
    <div className="usage-page">
      <UsageHeader />
      <div className="usage-topbar">
        <div className="usage-tabs" role="tablist" aria-label="Usage views">
          {TABS.map((entry) => (
            <button
              key={entry.id}
              type="button"
              role="tab"
              id={`usage-tab-${entry.id}`}
              aria-controls={`usage-panel-${entry.id}`}
              aria-selected={tab === entry.id}
              tabIndex={tab === entry.id ? 0 : -1}
              className={`usage-tab${tab === entry.id ? " usage-tab-active" : ""}`}
              onClick={() => setTab(entry.id)}
              onKeyDown={(event) => {
                if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
                event.preventDefault();
                const next: UsageTab = entry.id === "overview" ? "models" : "overview";
                setTab(next);
                document.getElementById(`usage-tab-${next}`)?.focus();
              }}
            >
              {entry.label}
            </button>
          ))}
        </div>
        <div className="usage-period-switch" role="group" aria-label="Period">
          {data.periods.map((candidate) => (
            <button
              key={candidate.id}
              type="button"
              aria-pressed={candidate.id === period.id}
              className={`usage-period-option${candidate.id === period.id ? " usage-period-option-active" : ""}`}
              onClick={() => setPeriodId(candidate.id)}
            >
              {candidate.label}
            </button>
          ))}
        </div>
      </div>
      <div
        id="usage-panel-overview"
        role="tabpanel"
        tabIndex={0}
        aria-labelledby="usage-tab-overview"
        hidden={tab !== "overview"}
        className={tab !== "overview" ? "usage-tab-panel-hidden" : undefined}
      >
        <OverviewPanel
          period={period}
          currentStreakDays={data.currentStreakDays}
          longestStreakDays={data.longestStreakDays}
        />
      </div>
      <div
        id="usage-panel-models"
        role="tabpanel"
        tabIndex={0}
        aria-labelledby="usage-tab-models"
        hidden={tab !== "models"}
        className={tab !== "models" ? "usage-tab-panel-hidden" : undefined}
      >
        <ModelsPanel period={period} sort={sort} onSortChange={setSort} />
      </div>
    </div>
  );
}

function UsageHeader() {
  return (
    <header className="usage-header"><div><p className="eyebrow">Spending</p><h1 id="usage-modal-title">Usage</h1><p className="usage-subtitle">Cost and tokens from your tasks.</p></div></header>
  );
}

function OverviewPanel({
  period,
  currentStreakDays,
  longestStreakDays,
}: {
  period: UsagePeriod;
  currentStreakDays: number;
  longestStreakDays: number;
}) {
  const cards: ReadonlyArray<{ label: string; value: string }> = [
    { label: "Tasks", value: period.tasks.toLocaleString("en-US") },
    { label: "Cost", value: formatCost(period.costUsd) ?? "$0.00" },
    { label: "Total tokens", value: formatTokenCount(period.tokens) },
    { label: "Active days", value: period.activeDays.toLocaleString("en-US") },
    { label: "Current streak", value: formatStreak(currentStreakDays) },
    { label: "Longest streak", value: formatStreak(longestStreakDays) },
    { label: "Peak hour", value: period.peakHour === null ? "—" : formatPeakHour(period.peakHour) },
    { label: "Favourite model", value: period.favouriteModel ?? "—" },
  ];
  return (
    <div className="usage-overview">
      <dl className="usage-cards">
        {cards.map((card) => (
          <div className="usage-card" key={card.label}>
            <dt>{card.label}</dt>
            <dd>{card.value}</dd>
          </div>
        ))}
      </dl>
      <Heatmap days={period.days} start={period.start} end={period.end} />
    </div>
  );
}

function formatStreak(days: number): string {
  if (days <= 0) return "—";
  return `${days}d`;
}

function formatPeakHour(hour: number): string {
  const twelve = hour % 12 === 0 ? 12 : hour % 12;
  return `${twelve} ${hour < 12 ? "AM" : "PM"}`;
}

const WEEKDAY_LABELS: ReadonlyArray<string> = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

function Heatmap({
  days,
  start,
  end,
}: {
  days: UsageDay[];
  start?: string;
  end?: string;
}) {
  const bounds = useMemo(() => heatmapMonthBounds(days, { start, end }), [days, start, end]);
  const [month, setMonth] = useState<MonthKey>(bounds.max);
  useEffect(() => {
    setMonth(bounds.max);
  }, [days, start, end]);

  const calendar = useMemo(() => buildHeatmapMonth(days, month), [days, month]);
  const max = Math.max(0, ...days.map((day) => day.costUsd));
  const hasActivity = calendar.weeks.some((week) => week.some((cell) => cell.inMonth && cell.day !== undefined));
  const canGoBack = compareMonths(month, bounds.min) > 0;
  const canGoForward = compareMonths(month, bounds.max) < 0;
  const label = monthLabel(month);

  return (
    <div className="usage-heatmap">
      <div className="usage-heatmap-header">
        <button
          type="button"
          className="icon-button"
          aria-label="Previous month"
          title="Previous month"
          disabled={!canGoBack}
          onClick={() => setMonth((current) => clampMonth(addMonths(current, -1), bounds))}
        >
          <BackArrowIcon />
        </button>
        <span className="usage-heatmap-month-label" aria-live="polite">
          {label}
        </span>
        <button
          type="button"
          className="icon-button"
          aria-label="Next month"
          title="Next month"
          disabled={!canGoForward}
          onClick={() => setMonth((current) => clampMonth(addMonths(current, 1), bounds))}
        >
          <ForwardArrowIcon />
        </button>
      </div>
      <div className="usage-heatmap-weekdays" aria-hidden="true">
        {WEEKDAY_LABELS.map((weekday) => (
          <span key={weekday} className="usage-heatmap-weekday">
            {weekday}
          </span>
        ))}
      </div>
      <div className="usage-heatmap-grid" role="img" aria-label={`Daily activity for ${label}`}>
        {calendar.weeks.flatMap((week) =>
          week.map((cell) => (
            <span
              className="usage-heatmap-cell"
              key={cell.date}
              data-in-month={cell.inMonth}
              data-level={cell.day !== undefined && cell.inMonth ? intensityLevel(cell.day.costUsd, max) : 0}
              title={heatmapTooltip(cell)}
            />
          )),
        )}
      </div>
      {!hasActivity && <p className="usage-heatmap-empty">No activity in {label}.</p>}
    </div>
  );
}

function heatmapTooltip(cell: HeatmapCell): string {
  if (cell.day !== undefined && cell.inMonth) {
    const cost = formatCost(cell.day.costUsd) ?? "$0.00";
    const tasks = `${cell.day.tasks} ${cell.day.tasks === 1 ? "task" : "tasks"}`;
    return `${cell.date} · ${cost} · ${formatTokenCount(cell.day.tokens)} tokens · ${tasks}`;
  }
  return `${cell.date} · $0.00 · 0 tokens · 0 tasks`;
}

function intensityLevel(cost: number, max: number): number {
  if (cost <= 0 || max <= 0) return 0;
  const ratio = cost / max;
  if (ratio <= 0.25) return 1;
  if (ratio <= 0.5) return 2;
  if (ratio <= 0.75) return 3;
  return 4;
}

function ModelsPanel({
  period,
  sort,
  onSortChange,
}: {
  period: UsagePeriod;
  sort: ModelSort;
  onSortChange: (sort: ModelSort) => void;
}) {
  const rows = useMemo(() => sortBreakdown(period.breakdown, sort), [period.breakdown, sort]);
  const handleSortChange = (value: string) => {
    // SAFETY: the select below only offers the three ModelSort option values.
    onSortChange(value as ModelSort);
  };
  return (
    <div className="usage-models">
      <div className="usage-models-toolbar">
        <span className="usage-muted">
          {period.tasks} {period.tasks === 1 ? "task" : "tasks"} · {formatCost(period.costUsd) ?? "$0.00"} · {formatTokenCount(period.tokens)} tokens
        </span>
        <label className="usage-sort">
          <span>Sort by</span>
          <select value={sort} onChange={(event) => handleSortChange(event.target.value)}>
            <option value="cost">Most used</option>
            <option value="tokens">Most tokens</option>
            <option value="tasks">Most tasks</option>
          </select>
        </label>
      </div>
      {rows.length === 0 ? (
        <p className="usage-muted">No task usage in this period.</p>
      ) : (
        <div className="usage-breakdown">{rows.map((row) => (
          <div className="usage-row" key={`${row.provider}:${row.profile}:${row.model}`}>
            <span className="usage-row-name"><strong>{row.profile}</strong><small>{row.provider} · {row.model}</small></span>
            <span className="usage-row-cost">{formatCost(row.costUsd) ?? "$0.00"}</span>
            <span className="usage-row-tokens">{formatTokenCount(row.tokens)} tokens</span>
            <span className="usage-row-tasks">{row.tasks} {row.tasks === 1 ? "task" : "tasks"}</span>
          </div>
        ))}</div>
      )}
    </div>
  );
}

function sortBreakdown(rows: UsageBreakdown[], sort: ModelSort): UsageBreakdown[] {
  return [...rows].sort((a, b) => {
    if (sort === "tokens") return b.tokens - a.tokens || a.model.localeCompare(b.model);
    if (sort === "tasks") return b.tasks - a.tasks || a.model.localeCompare(b.model);
    if (b.costUsd !== a.costUsd) return b.costUsd - a.costUsd;
    return a.model.localeCompare(b.model);
  });
}
