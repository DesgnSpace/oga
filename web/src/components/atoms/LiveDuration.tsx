// A task's run time, counting up each second while a run is active.

import { useEffect, useState } from "react";
import { taskDuration } from "@/lib/format";

export function LiveDuration({ durationMs, runningSince, running }: {
  durationMs: number | undefined;
  runningSince: string | undefined;
  running: boolean;
}) {
  const ticking = running && runningSince !== undefined;
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!ticking) return;
    const timer = setInterval(() => setTick((tick) => tick + 1), 1_000);
    return () => clearInterval(timer);
  }, [ticking]);
  return <>{taskDuration(durationMs, runningSince, running)}</>;
}
