// Browser-only transport for `bun run dev` outside the desktop shell.
// Reads go to the local broker through the Vite `/broker` proxy, so UI work
// hot-reloads without a Rust rebuild. Writes are refused: this view is for
// looking, not for changing tasks. There is no event stream, so an open task
// shows the state it had when opened.

import type { Transport } from "./transport";

type Args = Record<string, unknown> | undefined;
type Query = Record<string, unknown> | undefined;

const READ_ONLY = "The browser preview is read-only. Use the desktop app to change tasks.";

// Query fields the broker reads under the same name. Flags it reads as "1"
// (`compact`, `skipSummaryAggregates`, `includeTask`) go in `flags`.
const STATE_KEYS = ["archived", "limit", "state", "createdSince", "idPrefix", "project", "search", "group"];
const EVENT_KEYS = ["after", "waitMs", "last", "before", "limit", "taskUpdatedAt"];

function params(query: Query, keys: string[], flags: string[], base: Record<string, string> = {}): string {
  const search = new URLSearchParams(base);
  for (const key of flags) if (query?.[key]) search.set(key, "1");
  for (const key of keys) {
    const value = query?.[key];
    if (value !== undefined && value !== null) search.set(key, String(value));
  }
  const text = search.toString();
  return text ? `?${text}` : "";
}

async function get<T>(path: string): Promise<T> {
  const response = await fetch(`/broker${path}`);
  if (!response.ok) throw { message: `${response.status} ${path}`, status: response.status };
  return (await response.json()) as T;
}

function enc(value: unknown): string {
  return encodeURIComponent(String(value));
}

async function brokerCall(call: Record<string, unknown>): Promise<unknown> {
  const id = enc(call.taskId);
  const query = call.query as Query;
  switch (call.call) {
    case "health":
      return get("/health");
    case "summary":
      return get(`/api/state${params(query, STATE_KEYS, ["compact", "skipSummaryAggregates"], { view: "summary" })}`);
    case "usage":
      return get(`/api/usage${call.tzOffset === undefined ? "" : `?tzOffset=${enc(call.tzOffset)}`}`);
    case "task":
      return get(`/api/tasks/${id}`);
    case "taskEvents":
      return get(`/api/tasks/${id}/events${params(query, EVENT_KEYS, ["includeTask"])}`);
    case "taskDiff":
      return get(`/api/tasks/${id}/diff${call.against ? `?against=${enc(call.against)}` : ""}`);
    case "taskBranch":
      return get(`/api/tasks/${id}/branch`);
    case "taskBranches":
      return get(`/api/tasks/${id}/branches`);
    case "memories":
      return get(`/api/memories?cwd=${enc(call.cwd)}`);
    case "callerPrompt":
      return get("/api/caller-prompts");
    case "modelSettings":
      return get(`/api/model-settings?refresh=${call.refresh ? "true" : "false"}`);
    case "enabledModels":
      return get("/api/model-settings?enabled=true");
    case "cleanup":
    case "waiting":
    case "attribution":
    case "advisor":
    case "appearance":
      return get(`/api/${call.call}`);
    default:
      throw { message: READ_ONLY };
  }
}

async function watchTask(args: Args): Promise<unknown> {
  const id = enc(args?.taskId);
  const events = Number(args?.events ?? 200);
  const [task, page] = await Promise.all([
    get<unknown>(`/api/tasks/${id}`),
    get<{ events?: unknown[]; cursor?: number; oldestId?: number; hasEarlier?: boolean } | unknown[]>(
      `/api/tasks/${id}/events?last=${events}&limit=${events}`,
    ),
  ]);
  if (Array.isArray(page)) return { task, events: page, cursor: 0, oldestId: null, hasEarlier: false };
  return {
    task,
    events: page.events ?? [],
    cursor: page.cursor ?? 0,
    oldestId: page.oldestId ?? null,
    hasEarlier: page.hasEarlier ?? false,
  };
}

export const devTransport: Transport = {
  async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    switch (command) {
      case "broker_call":
        return (await brokerCall(args?.call as Record<string, unknown>)) as T;
      case "broker_watch_task":
        return (await watchTask(args)) as T;
      case "broker_unwatch_task":
        return undefined as T;
      case "broker_stream_status":
        return { connected: true, cursor: 0, streamFloor: 0, stale: false } as T;
      default:
        throw { message: READ_ONLY };
    }
  },
  listen() {},
};
