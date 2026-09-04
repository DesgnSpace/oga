type TimeValue = Date | string | number;

function asDate(value: TimeValue): Date | undefined {
  const date = value instanceof Date ? new Date(value) : new Date(value);
  return Number.isNaN(date.getTime()) ? undefined : date;
}

const relativeFormatter = new Intl.RelativeTimeFormat(undefined, { numeric: "always", style: "short" });
const shortDateFormatter = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const absoluteFormatter = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

function shortRelative(value: number, unit: Intl.RelativeTimeFormatUnit): string {
  return relativeFormatter
    .format(value, unit)
    .replace("min.", "min")
    .replace("hr.", "h")
    .replace(/\bday(s)?\b/, "d");
}

export function absoluteTime(value: TimeValue): string {
  const date = asDate(value);
  return date ? absoluteFormatter.format(date) : String(value);
}

export function relativeTime(value: TimeValue, now: TimeValue = new Date()): string {
  const date = asDate(value);
  const current = asDate(now);
  if (!date || !current) return String(value);

  const seconds = Math.max(Math.floor((current.getTime() - date.getTime()) / 1000), 0);
  if (seconds < 60) return "just now";
  if (seconds < 3_600) return shortRelative(-Math.floor(seconds / 60), "minute");
  if (seconds < 86_400) return shortRelative(-Math.floor(seconds / 3_600), "hour");
  if (seconds < 604_800) return shortRelative(-Math.floor(seconds / 86_400), "day");
  return shortDateFormatter.format(date);
}
