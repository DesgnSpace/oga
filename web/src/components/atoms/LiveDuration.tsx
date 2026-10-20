// A task's run time, counting up each second while a run is active.

import { useEffect, useState } from "react";
import { taskDuration } from "@/lib/format";
import { useWindowVisible } from "@/lib/window-visibility";

export function LiveDuration({ durationMs, runningSince, running }: {
  durationMs: number | undefined;
  runningSince: string | undefined;
  running: boolean;
}) {
  const ticking = running && runningSince !== undefined;
  const visible = useWindowVisible();
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!ticking || !visible) return;
    const timer = setInterval(() => setTick((tick) => tick + 1), 1_000);
    return () => clearInterval(timer);
  }, [ticking, visible]);
  return <>{taskDuration(durationMs, runningSince, running)}</>;
}
