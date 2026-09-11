import { useEffect, useMemo, useState } from "react";
import { broker } from "@/bridge/client";
import type { Provider, UsageBreakdown, UsageDay, UsagePeriod, UsageResponse } from "@/bridge/types";
import { formatCost, formatTokenCount } from "@/lib/format";
import { BackArrowIcon, ForwardArrowIcon } from "@/ui/icons";
import { providerLabel } from "../settings/state";
import { buildDailySeries, topBreakdown, type DailyPoint, type ModelBar } from "./charts";
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
type OverviewView = "grid" | "charts";

const VIEWS: ReadonlyArray<{ id: OverviewView; label: string }> = [
  { id: "grid", label: "Grid" },
  { id: "charts", label: "Charts" },
];

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
  const [loadAttempt, setLoadAttempt] = useState(0);

  useEffect(() => {
    void broker.usage(-new Date().getTimezoneOffset()).then((result) => {
      if (result.ok) setData(result.value);
      else setError(result.error.message);
    });
  }, [loadAttempt]);

  if (error) {
    return (
      <div className="usage-page">
        <UsageHeader />
        <p role="alert">
          Couldn&apos;t load usage. Try again later. <button className="text-button" type="button" onClick={() => {
            setError(undefined);
            setData(undefined);
            setLoadAttempt((attempt) => attempt + 1);
          }}>Try again</button>
        </p>
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
  const [view, setView] = useState<OverviewView>("grid");
  const cards: ReadonlyArray<{ label: string; value: string }> = [
    { label: "Tasks", value: period.tasks.toLocaleString() },
    { label: "Cost", value: formatCost(period.costUsd) ?? "$0.00" },
    { label: "Total tokens", value: formatTokenCount(period.tokens) },
    { label: "Active days", value: period.activeDays.toLocaleString() },
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
            <dd title={card.value}>{card.value}</dd>
          </div>
        ))}
      </dl>
      <div className="usage-period-switch" role="group" aria-label="Activity view">
        {VIEWS.map((entry) => (
          <button
            key={entry.id}
            type="button"
            aria-pressed={view === entry.id}
            className={`usage-period-option${view === entry.id ? " usage-period-option-active" : ""}`}
            onClick={() => setView(entry.id)}
          >
            {entry.label}
          </button>
        ))}
      </div>
      {view === "grid" ? (
        <Heatmap days={period.days} start={period.start} end={period.end} />
      ) : (
        <UsageCharts period={period} />
      )}
    </div>
  );
}

function formatStreak(days: number): string {
  if (days <= 0) return "—";
  return `${days}d`;
}

function formatPeakHour(hour: number): string {
  const date = new Date(2000, 0, 1, hour);
  return date.toLocaleTimeString(undefined, { hour: "numeric" });
}

const WEEKDAY_LABELS: ReadonlyArray<string> = ["", "Mon", "", "Wed", "", "Fri", ""];

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
  const monthTasks = calendar.weeks.flat().reduce((total, cell) => total + (cell.inMonth ? cell.day?.tasks ?? 0 : 0), 0);
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
      <div className="usage-heatmap-body">
        <div className="usage-heatmap-weekdays" aria-hidden="true">
          {WEEKDAY_LABELS.map((weekday, index) => (
            <span key={index} className="usage-heatmap-weekday">
              {weekday}
            </span>
          ))}
        </div>
        <div className={`usage-heatmap-grid usage-heatmap-grid-${calendar.weeks.length}`} role="img" aria-label={`Daily activity for ${label} · ${monthTasks} ${monthTasks === 1 ? "task" : "tasks"}`}>
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
      </div>
      <div className="usage-heatmap-legend" aria-hidden="true">
        <span>Less</span>
        <span className="usage-heatmap-cell" data-level="0" />
        <span className="usage-heatmap-cell" data-level="1" />
        <span className="usage-heatmap-cell" data-level="2" />
        <span className="usage-heatmap-cell" data-level="3" />
        <span className="usage-heatmap-cell" data-level="4" />
        <span>More</span>
      </div>
      {!hasActivity && <p className="usage-heatmap-empty">No activity in {label}.</p>}
    </div>
  );
}

function heatmapTooltip(cell: HeatmapCell): string {
  if (cell.day !== undefined && cell.inMonth) {
    const cost = formatCost(cell.day.costUsd) ?? "$0.00";
    const tasks = `${cell.day.tasks} ${cell.day.tasks === 1 ? "task" : "tasks"}`;
    return `${formatShortDate(cell.date)} · ${cost} · ${formatTokenCount(cell.day.tokens)} tokens · ${tasks}`;
  }
  return `${formatShortDate(cell.date)} · $0.00 · 0 tokens · 0 tasks`;
}

function intensityLevel(cost: number, max: number): number {
  if (cost <= 0 || max <= 0) return 0;
  const ratio = cost / max;
  if (ratio <= 0.25) return 1;
  if (ratio <= 0.5) return 2;
  if (ratio <= 0.75) return 3;
  return 4;
}

const CHART_BREAKDOWN_LIMIT = 8;

function UsageCharts({ period }: { period: UsagePeriod }) {
  const series = useMemo(() => buildDailySeries(period.days, period.start, period.end), [period.days, period.start, period.end]);
  const bars = useMemo(() => topBreakdown(period.breakdown, CHART_BREAKDOWN_LIMIT), [period.breakdown]);
  return (
    <div className="usage-charts">
      <DailyBarChart label="Cost over time" series={series} value={(point) => point.costUsd} formatValue={(value) => formatCost(value) ?? "$0.00"} />
      <DailyBarChart label="Tokens over time" series={series} value={(point) => point.tokens} formatValue={formatTokenCount} />
      <ModelBarChart bars={bars} />
    </div>
  );
}

const CHART_WIDTH = 600;
const CHART_HEIGHT = 160;
const CHART_PAD_LEFT = 46;
const CHART_PAD_TOP = 12;
const CHART_PAD_BOTTOM = 20;
const CHART_PLOT_WIDTH = CHART_WIDTH - CHART_PAD_LEFT;
const CHART_PLOT_HEIGHT = CHART_HEIGHT - CHART_PAD_TOP - CHART_PAD_BOTTOM;
const CHART_BASELINE_Y = CHART_PAD_TOP + CHART_PLOT_HEIGHT;
const CHART_BAR_MIN_WIDTH = 1;
const CHART_BAR_MAX_WIDTH = 14;
const CHART_BAR_RADIUS = 2;

function DailyBarChart({
  label,
  series,
  value,
  formatValue,
}: {
  label: string;
  series: DailyPoint[];
  value: (point: DailyPoint) => number;
  formatValue: (value: number) => string;
}) {
  const max = Math.max(0, ...series.map(value));
  const isEmpty = max <= 0;
  const slot = series.length > 0 ? CHART_PLOT_WIDTH / series.length : CHART_PLOT_WIDTH;
  const barWidth = Math.min(CHART_BAR_MAX_WIDTH, Math.max(CHART_BAR_MIN_WIDTH, slot * 0.6));
  const bars = series.map((point, index) => {
    const height = max <= 0 ? 0 : (value(point) / max) * CHART_PLOT_HEIGHT;
    return {
      x: CHART_PAD_LEFT + slot * index + (slot - barWidth) / 2,
      y: CHART_BASELINE_Y - height,
      height,
      point,
    };
  });
  return (
    <div className="usage-chart">
      <h3 className="usage-chart-title">{label}</h3>
      <svg
        className="usage-chart-svg"
        viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`}
        role="img"
        aria-label={isEmpty ? `${label}, no activity in this period` : `${label}, peak ${formatValue(max)}`}
      >
        <line x1={CHART_PAD_LEFT} y1={CHART_BASELINE_Y} x2={CHART_WIDTH} y2={CHART_BASELINE_Y} className="usage-chart-axis" />
        <text x={CHART_PAD_LEFT - 6} y={CHART_BASELINE_Y} className="usage-chart-axis-label" textAnchor="end">
          {formatValue(0)}
        </text>
        {!isEmpty && (
          <>
            <line x1={CHART_PAD_LEFT} y1={CHART_PAD_TOP} x2={CHART_WIDTH} y2={CHART_PAD_TOP} className="usage-chart-gridline" />
            <text x={CHART_PAD_LEFT - 6} y={CHART_PAD_TOP + 4} className="usage-chart-axis-label" textAnchor="end">
              {formatValue(max)}
            </text>
            {bars.map(({ x, y, height, point }) => (
              <rect key={point.date} x={x} y={y} width={barWidth} height={height} rx={Math.min(CHART_BAR_RADIUS, barWidth / 2)} className="usage-chart-bar">
                <title>{`${point.date} · ${formatValue(value(point))}`}</title>
              </rect>
            ))}
          </>
        )}
        {series.length > 0 && (
          <>
            <text x={CHART_PAD_LEFT} y={CHART_HEIGHT - 4} className="usage-chart-axis-label" textAnchor="start">
              {formatShortDate(series[0].date)}
            </text>
            <text x={CHART_WIDTH} y={CHART_HEIGHT - 4} className="usage-chart-axis-label" textAnchor="end">
              {formatShortDate(series[series.length - 1].date)}
            </text>
          </>
        )}
      </svg>
      {isEmpty && <p className="usage-chart-empty">No activity in this period.</p>}
    </div>
  );
}

function formatShortDate(date: string): string {
  return new Date(`${date}T00:00:00`).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function ModelBarChart({ bars }: { bars: ReadonlyArray<ModelBar> }) {
  const max = Math.max(0, ...bars.map((bar) => bar.costUsd));
  const isEmpty = bars.length === 0 || max <= 0;
  return (
    <div className="usage-chart">
      <h3 className="usage-chart-title">Cost by worker</h3>
      {isEmpty ? (
        <p className="usage-chart-empty">No usage by worker in this period.</p>
      ) : (
        <ul className="usage-bar-chart">
          {bars.map((bar) => (
            <li className="usage-bar-row" key={bar.label}>
              <span className="usage-bar-label">{bar.label}</span>
              <span className="usage-bar-track">
                <span className="usage-bar-fill" style={{ width: `${(bar.costUsd / max) * 100}%` }} />
              </span>
              <span className="usage-bar-value">{formatCost(bar.costUsd) ?? "$0.00"}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
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
            <option value="cost">Highest cost</option>
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
            <span className="usage-row-name"><strong>{row.profile}</strong><small>{providerLabel(row.provider as Provider)} · <span title={row.model}>{row.model.slice(row.model.lastIndexOf("/") + 1)}</span></small></span>
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
