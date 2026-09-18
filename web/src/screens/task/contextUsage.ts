// How full the worker's context window is, read off the run itself.
//
// Each provider reports its freshest input size on its usage rows — Claude's
// result, Codex's turn receipt, OpenCode's step receipt — as input tokens plus
// whatever it read back from cache. The newest read is the current fill; the
// oldest read of the current stretch is where growth is measured from. A
// compaction starts a new stretch, so the number drops instead of climbing
// forever. No usable read, or no published window, means no answer — the
// footer shows nothing rather than a guess.

import type { TaskEventView } from "@/bridge/types";

export interface UsageTotals {
  tokensIn: number;
  tokensOut: number;
  tokensCached: number;
}

/**
 * A worker that publishes how full its context window is restates the whole
 * session on every reading, so the freshest one is the answer and adding them
 * up would report the same tokens once per reading. A worker that reports a
 * turn at a time is describing that turn alone, and those still add up.
 */
function reportsRunningTotal(event: TaskEventView): boolean {
  return event.kind === "usage" && event.presentation?.total !== undefined;
}

/** What the run has spent in tokens, however its worker reports them. */
export function usageTotals(events: TaskEventView[]): UsageTotals {
  let tokensIn = 0;
  let tokensOut = 0;
  let tokensCached = 0;
  let running: UsageTotals | undefined;
  for (const event of events) {
    const presentation = event.presentation;
    if (!presentation) continue;
    if (reportsRunningTotal(event)) {
      running = {
        tokensIn: presentation.tokensIn ?? 0,
        tokensOut: presentation.tokensOut ?? 0,
        tokensCached: presentation.tokensCached ?? 0,
      };
      continue;
    }
    tokensIn += presentation.tokensIn ?? 0;
    tokensOut += presentation.tokensOut ?? 0;
    tokensCached += presentation.tokensCached ?? 0;
  }
  return {
    tokensIn: tokensIn + (running?.tokensIn ?? 0),
    tokensOut: tokensOut + (running?.tokensOut ?? 0),
    tokensCached: tokensCached + (running?.tokensCached ?? 0),
  };
}

export interface ContextUsage {
  /** The provider's freshest read of context consumed, in tokens. */
  used: number;
  /** The model's published window, in tokens. */
  window: number;
  /** Fill against the window, rounded — can exceed 100. */
  percent: number;
  /** Fill against the window, rounded and clamped to 100 for display. */
  displayPercent: number;
  /** Growth since the first read of the current stretch, in tokens. */
  delta: number;
  /** Growth against the window, rounded. */
  deltaPercent: number;
  /** True once a compaction has started a new stretch this run. */
  compacted: boolean;
}

/** The reader-facing title of the broker's compaction marker event. */
export const COMPACT_BOUNDARY_TITLE = "Compact boundary";

export function contextUsage(
  events: TaskEventView[],
  contextWindow: number | undefined,
): ContextUsage | undefined {
  if (contextWindow === undefined || contextWindow <= 0) return undefined;
  let segmentStart = 0;
  for (let index = 0; index < events.length; index++) {
    if (events[index]?.title === COMPACT_BOUNDARY_TITLE) segmentStart = index + 1;
  }
  const reads: number[] = [];
  for (const event of events.slice(segmentStart)) {
    const presentation = event.presentation;
    if (!presentation) continue;
    const input = (presentation.tokensIn ?? 0) + (presentation.tokensCached ?? 0);
    if (input > 0) reads.push(input);
  }
  if (reads.length === 0) return undefined;
  const used = reads[reads.length - 1] ?? 0;
  const first = reads[0] ?? used;
  const delta = Math.max(0, used - first);
  const percent = Math.round((used / contextWindow) * 100);
  return {
    used,
    window: contextWindow,
    percent,
    displayPercent: Math.min(percent, 100),
    delta,
    deltaPercent: Math.round((delta / contextWindow) * 100),
    compacted: segmentStart > 0,
  };
}
