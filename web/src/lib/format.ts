// Cost, token, and duration formatting shared by the sidebar and task header.

/** Undefined when the cost is unknown or the model is free — never "$0.00".
 * `estimated` prefixes a "~": the provider never reported an amount, so this
 * was priced from public model rates instead. */
export function formatCost(costUsd: number | undefined, estimated?: boolean): string | undefined {
  if (costUsd === undefined || costUsd === 0) return undefined;
  const prefix = estimated ? "~" : "";
  if (costUsd < 0.01) return `${prefix}<$0.01`;
  return `${prefix}$${costUsd.toFixed(2)}`;
}

export function formatTokenCount(value: number): string {
  if (value >= 999_500) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}k`;
  return String(value);
}

export function formatDuration(milliseconds: number): string {
  if (milliseconds < 1_000) return "0s";
  const seconds = Math.floor(milliseconds / 1_000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

/** Time spent in worker runs, with the active run ticking from its start. */
export function taskDuration(durationMs: number | undefined, runningSince: string | undefined, running: boolean): string {
  let total = durationMs ?? 0;
  if (running && runningSince !== undefined) {
    const started = new Date(runningSince).getTime();
    if (!Number.isNaN(started)) total += Math.max(0, Date.now() - started);
  }
  return formatDuration(total);
}
