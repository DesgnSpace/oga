import { useEffect, useState } from "react";
import type { TaskSummary } from "@/bridge/types";
import { taskStatusLook, type TaskStatusLook } from "@/components/atoms/TaskStatusDot";

/** How long a task stays marked as arrived: its entrance plus a margin. */
const ARRIVAL_MS = 400;
const NONE: ReadonlySet<string> = new Set();

interface SeenTasks {
  looks: ReadonlyMap<string, TaskStatusLook>;
  newestCreatedAt: number;
}

function seenTasks(tasks: readonly TaskSummary[], previous?: SeenTasks): SeenTasks {
  let newestCreatedAt = previous?.newestCreatedAt ?? 0;
  const looks = new Map<string, TaskStatusLook>();
  for (const task of tasks) {
    looks.set(task.id, taskStatusLook(task.state));
    newestCreatedAt = Math.max(newestCreatedAt, Date.parse(task.createdAt) || 0);
  }
  return { looks, newestCreatedAt };
}

function arrivals(seen: SeenTasks, tasks: readonly TaskSummary[]): Set<string> {
  const arrived = new Set<string>();
  for (const task of tasks) {
    const look = seen.looks.get(task.id);
    const isNew = look === undefined && (Date.parse(task.createdAt) || 0) > seen.newestCreatedAt;
    const moved = look !== undefined && look !== taskStatusLook(task.state);
    if (isNew || moved) arrived.add(task.id);
  }
  return arrived;
}

/**
 * Ids of tasks created or moved to another status since the last update.
 * The first load and older pages never count. Each id clears after a moment.
 */
export function useTaskArrivals(tasks: readonly TaskSummary[]): ReadonlySet<string> {
  const [list, setList] = useState(tasks);
  const [seen, setSeen] = useState(() => (tasks.length > 0 ? seenTasks(tasks) : undefined));
  const [arrived, setArrived] = useState(NONE);

  if (tasks !== list) {
    setList(tasks);
    setSeen(seenTasks(tasks, seen));
    const next = seen ? arrivals(seen, tasks) : NONE;
    if (next.size > 0) setArrived(next);
  }

  useEffect(() => {
    if (arrived.size === 0) return;
    const timer = window.setTimeout(() => setArrived(NONE), ARRIVAL_MS);
    return () => window.clearTimeout(timer);
  }, [arrived]);

  return arrived;
}
