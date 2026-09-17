// Saves one task's rendered rows as a fixture, so the composition tests run
// against what a real provider actually sent rather than a hand-written guess.
//
//   bun scripts/dump-task-events.ts <task id> <fixture name>
//
// Reads from a running Oga daemon; set OGA_URL when it is not on the default
// port.

import type { TaskEventView } from "../src/bridge/types";

const DEFAULT_URL = "http://127.0.0.1:7331";

const [taskId, name] = Bun.argv.slice(2);
if (taskId === undefined || name === undefined) {
  console.error("usage: bun scripts/dump-task-events.ts <task id> <fixture name>");
  process.exit(2);
}

const base = (process.env.OGA_URL ?? DEFAULT_URL).replace(/\/+$/, "");
const response = await fetch(`${base}/api/tasks/${taskId}/events`);
if (!response.ok) {
  console.error(`${base} answered ${response.status} for task ${taskId}`);
  process.exit(1);
}

// SAFETY: the daemon serves this route as the rendered rows themselves.
const events = (await response.json()) as TaskEventView[];
const path = new URL(`../src/domain/activity/fixtures/${name}.json`, import.meta.url);
await Bun.write(path, `${JSON.stringify(events)}\n`);
console.log(`${events.length} rows → ${path.pathname}`);
