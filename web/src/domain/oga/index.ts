export type OgaValue = string | number | boolean | null | OgaValue[] | OgaObject;

export interface OgaObject {
  [key: string]: OgaValue;
}

export interface OgaCall {
  operation: string;
  input: OgaObject;
  result?: OgaValue;
}

export interface OgaResultText {
  text: string;
  hiddenLines: number;
}

const EMPTY_INPUT: OgaObject = {};

export function ogaCall(tool: string | undefined, input: OgaObject | undefined, server?: string): OgaCall | undefined {
  const wrapper = isMcpWrapper(tool);
  const nestedServer = firstString(input, ["ServerName", "serverName", "server", "Server"]);
  const operation = ogaOperation(tool, server ?? nestedServer, firstString(input, ["ToolName", "toolName", "tool"]));
  if (operation === undefined) return undefined;
  const callInput = wrapper
    ? firstObject(input, ["Arguments", "arguments", "args"]) ?? input ?? EMPTY_INPUT
    : input ?? EMPTY_INPUT;
  return { operation, input: callInput };
}

export function ogaTitle(operation: string, input: OgaObject = EMPTY_INPUT): string {
  if (operation === "memory") {
    const action = firstString(input, ["action"]);
    if (action === "set") return "Save project note";
    if (action === "remove") return "Remove project note";
    return "Read project notes";
  }
  if (operation === "archive" && input.archived === false) return "Restore task";
  return {
    tasks: "List tasks",
    query: "Find code",
    delegate: "Send work",
    inspect: "View task",
    health: "Check connection",
    models: "Check available models",
    reply: "Answer question",
    resume: "Continue task",
    steer: "Guide task",
    handoff: "Move task",
    cancel: "Stop task",
    complete: "Confirm task complete",
    archive: "Archive task",
    "worktree-remove": "Remove task copy",
  }[operation] ?? humanizeOgaKey(operation);
}

export function ogaVerb(operation: string, input: OgaObject = EMPTY_INPUT, complete: boolean): string {
  const tense = (done: string, open: string) => (complete ? done : open);
  if (operation === "memory") {
    const action = firstString(input, ["action"]);
    if (action === "set") return tense("Saved", "Saving");
    if (action === "remove") return tense("Removed", "Removing");
    return tense("Read", "Reading");
  }
  if (operation === "archive") return input.archived === false ? tense("Restored", "Restoring") : tense("Archived", "Archiving");
  return {
    tasks: tense("Listed", "Listing"),
    query: tense("Searched", "Searching"),
    delegate: tense("Sent", "Sending"),
    inspect: tense("Viewed", "Viewing"),
    health: tense("Checked", "Checking"),
    models: tense("Checked", "Checking"),
    reply: tense("Answered", "Answering"),
    resume: tense("Continued", "Continuing"),
    steer: tense("Guided", "Guiding"),
    handoff: tense("Moved", "Moving"),
    cancel: tense("Stopped", "Stopping"),
    complete: tense("Confirmed", "Confirming"),
    "worktree-remove": tense("Removed", "Removing"),
  }[operation] ?? tense("Ran", "Running");
}

export function ogaSubject(operation: string, input: OgaObject = EMPTY_INPUT): string | undefined {
  const subject = (() => {
    switch (operation) {
      case "tasks":
        return firstString(input, ["query", "q"])
          ?? taskFilterSubject(input)
          ?? (input.archived === true ? "archived tasks" : input.archived === false ? "active tasks" : undefined);
      case "query":
        return firstString(input, ["q", "query"]);
      case "delegate":
        return firstString(input, ["title", "tldr", "description", "prompt"]);
      case "memory":
        return firstString(input, ["key", "cwd"]);
      case "health":
        return "broker";
      case "models":
        return firstString(input, ["query", "profile", "provider"]) ?? "available models";
      case "worktree-remove":
        return firstString(input, ["project"]) ?? taskSubject(input);
      case "inspect":
      case "reply":
      case "resume":
      case "steer":
      case "handoff":
      case "cancel":
      case "complete":
      case "archive":
        return taskSubject(input);
      default:
        return firstString(input, ["query", "pattern", "description", "name", "project"]);
    }
  })();
  return subject === undefined ? undefined : clipOga(subject, 120);
}

export function ogaResultSummary(operation: string, output: string | undefined): string | undefined {
  if (output === undefined || output.trim() === "") return undefined;
  const trimmed = output.trim();
  const parsed = parseOgaJson(trimmed);
  if (parsed === undefined) return operation === "query" ? queryResultSummary(trimmed) : firstOutputLine(trimmed);
  const value = unwrapJsonString(parsed);
  const error = errorResult(value);
  if (error !== undefined) return error;
  switch (operation) {
    case "tasks":
      return countResult(value, "tasks", "task") ?? firstOutputLineValue(value);
    case "models":
      return countResult(value, "models", "model") ?? firstOutputLineValue(value);
    case "query":
      return ogaStringValue(value) === undefined ? firstOutputLineValue(value) : queryResultSummary(ogaStringValue(value) ?? "");
    case "memory":
      return memoryResultSummary(value);
    case "health": {
      const object = ogaObject(value);
      const ok = object?.ok;
      if (ok === true) return "Connected";
      if (ok === false) return "Unavailable";
      return firstOutputLineValue(value);
    }
    case "delegate":
      return taskStateResult(value, "Work") ?? "Work sent";
    case "inspect":
      return taskStateResult(value, "Task");
    case "reply":
      return "Answer sent";
    case "resume":
      return "Task continued";
    case "steer":
      return "Instruction sent";
    case "handoff":
      return "Task moved";
    case "cancel":
      return actionResultSummary(value, "Task stopped", "tasks", "task");
    case "complete":
      return "Task marked complete";
    case "archive":
      return actionResultSummary(value, "Task archived", "tasks", "task");
    case "worktree-remove":
      return actionResultSummary(value, "Task copy removed", "removed", "task copy");
    default:
      return firstOutputLineValue(value);
  }
}

export function ogaResultText(raw: string): OgaResultText | undefined {
  const value = parseOgaJson(raw);
  if (value === undefined) return undefined;
  const call = findOgaCall(value);
  if (call?.result === undefined) return undefined;
  const text = formatOgaResult(call.operation, call.result);
  if (text === undefined) return undefined;
  return boundOgaResult(text);
}

function findOgaCall(value: OgaValue): OgaCall | undefined {
  const object = ogaObject(value);
  if (object) {
    const info = ogaObject(object.tool_info);
    const state = ogaObject(object.state);
    const parameters = firstObject(
      info,
      ["parameters"],
    )
      ?? firstObject(object, ["parameters", "tool_input", "toolInput", "arguments", "args", "input"])
      ?? firstObject(state, ["input"]);
    const tool = firstString(object, ["tool_name", "toolName", "tool", "name"]);
    const server = firstString(parameters, ["ServerName", "serverName", "server", "Server"])
      ?? firstString(object, ["server"]);
    const call = ogaCall(tool, parameters, server);
    if (call) {
      const directResult = firstResult(object);
      const infoResult = info ? firstResult(info) : undefined;
      const stateResult = state ? firstResult(state) : undefined;
      const result = directResult !== undefined ? directResult : infoResult !== undefined ? infoResult : stateResult;
      return result === undefined ? call : { ...call, result };
    }
    for (const child of Object.values(object)) {
      const found = findOgaCall(child);
      if (found) return found;
    }
  } else if (Array.isArray(value)) {
    for (const entry of value) {
      const found = findOgaCall(entry);
      if (found) return found;
    }
  }
  return undefined;
}

function firstResult(object: OgaObject): OgaValue | undefined {
  for (const key of ["tool_response", "toolResponse", "result", "output", "error"]) {
    if (Object.hasOwn(object, key)) return object[key];
  }
  return undefined;
}

function formatOgaResult(operation: string, raw: OgaValue): string | undefined {
  const rawError = errorResult(raw);
  if (rawError !== undefined) return rawError;
  const value = unwrapOgaResult(raw);
  const valueError = errorResult(value);
  if (valueError !== undefined) return valueError;
  const text = ogaStringValue(value);
  if (text !== undefined) return text.trim() === "" ? undefined : text.trim();
  if (value === null) return operation === "memory" ? "No note found" : "No result reported";
  if (operation === "tasks") return formatOgaTasks(value);
  if (operation === "models") return formatOgaModels(value);
  if (operation === "memory") return formatOgaMemory(value);
  return formatStructuredOgaResult(value);
}

function unwrapOgaResult(value: OgaValue): OgaValue {
  const text = ogaStringValue(value);
  if (text !== undefined) {
    const parsed = parseOgaJson(text);
    return parsed === undefined ? value : unwrapOgaResult(parsed);
  }
  if (Array.isArray(value)) {
    const textBlock = value.find((entry) => ogaObject(entry)?.type === "text");
    const textValue = textBlock === undefined ? undefined : ogaObject(textBlock)?.text;
    return textValue === undefined ? value : unwrapOgaResult(textValue);
  }
  const object = ogaObject(value);
  if (!object) return value;
  if (Object.hasOwn(object, "content")) return unwrapOgaResult(object.content);
  if (object.type === "text" && Object.hasOwn(object, "text")) return unwrapOgaResult(object.text);
  const transportKeys = ["stdout", "output", "result", "response"];
  if (Object.keys(object).every((key) => transportKeys.includes(key) || key === "type")) {
    for (const key of transportKeys) {
      if (Object.hasOwn(object, key)) return unwrapOgaResult(object[key]);
    }
  }
  return object;
}

function formatOgaTasks(value: OgaValue): string {
  const entries = Array.isArray(value) ? value : ogaObject(value)?.tasks;
  if (!Array.isArray(entries)) return formatStructuredOgaResult(value);
  const lines = [`${entries.length} task${entries.length === 1 ? "" : "s"}`];
  for (const entry of entries) {
    const object = ogaObject(entry);
    if (!object) {
      lines.push(`- ${formatOgaInline(entry)}`);
      continue;
    }
    const title = firstString(object, ["title", "tldr", "promptPreview"]) ?? "Task";
    const state = ogaStateLabel(firstString(object, ["state"]));
    lines.push(`- ${[singleLineOga(title), state].filter((part): part is string => part !== undefined).join(" · ")}`);
  }
  return lines.join("\n");
}

function formatOgaModels(value: OgaValue): string {
  const models = ogaObject(value)?.models;
  if (!Array.isArray(models)) return formatStructuredOgaResult(value);
  const lines = [`${models.length} model${models.length === 1 ? "" : "s"}`];
  for (const entry of models) {
    const object = ogaObject(entry);
    if (!object) {
      lines.push(`- ${formatOgaInline(entry)}`);
      continue;
    }
    const name = firstString(object, ["model", "id"]) ?? "Model";
    const profile = firstString(object, ["profileId", "profile"]);
    const status = object.enabled === false ? "off" : object.preferred === true ? "preferred" : undefined;
    lines.push(`- ${[profile, name, status].filter((part): part is string => part !== undefined).join(" · ")}`);
  }
  return lines.join("\n");
}

function formatOgaMemory(value: OgaValue): string {
  if (!Array.isArray(value)) return formatStructuredOgaResult(value);
  if (value.length === 0) return "No saved notes";
  const lines = [`${value.length} saved note${value.length === 1 ? "" : "s"}`];
  for (const entry of value) {
    const object = ogaObject(entry);
    if (!object) {
      lines.push(`- ${formatOgaInline(entry)}`);
      continue;
    }
    const key = firstString(object, ["key"]) ?? "Note";
    const text = firstString(object, ["value"]);
    lines.push(`- ${[key, text && singleLineOga(text)].filter((part): part is string => part !== undefined).join(": ")}`);
  }
  return lines.join("\n");
}

function formatStructuredOgaResult(value: OgaValue, indent = 0): string {
  if (Array.isArray(value)) {
    if (value.length === 0) return "No items";
    return value.map((entry) => `${" ".repeat(indent)}- ${formatOgaInline(entry)}`).join("\n");
  }
  const object = ogaObject(value);
  if (!object) return formatOgaInline(value);
  const lines: string[] = [];
  for (const [key, entry] of Object.entries(object).filter(([key]) => !isOgaIdentifierKey(key))) {
    const label = ogaResultFieldLabel(key);
    if (Array.isArray(entry) || ogaObject(entry)) {
      lines.push(`${" ".repeat(indent)}${label}:`);
      lines.push(formatStructuredOgaResult(entry, indent + 2));
    } else {
      lines.push(`${" ".repeat(indent)}${label}: ${formatOgaFieldValue(key, entry)}`);
    }
  }
  return lines.length > 0 ? lines.join("\n") : "No details";
}

function formatOgaInline(value: OgaValue): string {
  if (value === null) return "none";
  if (Array.isArray(value)) return `${value.length} item${value.length === 1 ? "" : "s"}`;
  const tag = Object.prototype.toString.call(value);
  if (tag === "[object String]") return singleLineOga(String(value));
  if (tag === "[object Boolean]") return value ? "yes" : "no";
  if (tag === "[object Number]") return String(value);
  const object = ogaObject(value);
  return object ? `${Object.keys(object).length} field${Object.keys(object).length === 1 ? "" : "s"}` : "none";
}

function singleLineOga(value: string): string {
  const compact = value.split(/\s+/).filter((part) => part !== "").join(" ");
  return compact.length > 180 ? `${compact.slice(0, 177)}...` : compact;
}

function humanizeOgaKey(value: string): string {
  const label = value.replace(/([a-z])([A-Z])/g, "$1 $2").replace(/[_-]/g, " ");
  return label.charAt(0).toUpperCase() + label.slice(1);
}

function ogaResultFieldLabel(key: string): string {
  switch (key.toLowerCase()) {
    case "state":
      return "Status";
    case "output":
    case "result":
      return "Result";
    default:
      return humanizeOgaKey(key);
  }
}

function formatOgaFieldValue(key: string, value: OgaValue): string {
  if (key.toLowerCase() === "state") {
    const state = ogaStringValue(value);
    if (state !== undefined) return ogaStateLabel(state) ?? "status unavailable";
  }
  return formatOgaInline(value);
}

function ogaStateLabel(value: string | undefined): string | undefined {
  if (value === undefined) return undefined;
  return {
    queued: "waiting to start",
    pending: "waiting to start",
    running: "in progress",
    answered: "in progress",
    needs_input: "waiting for an answer",
    completed: "complete",
    failed: "failed",
    blocked: "blocked",
    cancelled: "stopped",
  }[value.toLowerCase()] ?? "status unavailable";
}

function taskSubject(input: OgaObject): string | undefined {
  const taskId = input.taskId;
  if (Array.isArray(taskId)) return formatCounted(taskId.length, "task");
  return ogaStringValue(taskId) === undefined ? undefined : "selected task";
}

function taskFilterSubject(input: OgaObject): string | undefined {
  const state = firstString(input, ["state"]);
  return state === undefined ? undefined : `${ogaStateLabel(state) ?? "selected"} tasks`;
}

function countResult(value: OgaValue, key: string, noun: string): string | undefined {
  const count = Array.isArray(value) ? value.length : ogaObject(value)?.[key];
  return Array.isArray(count) ? formatCounted(count.length, noun) : undefined;
}

function formatCounted(count: number, noun: string): string {
  const plural = noun === "match" ? "matches" : noun === "task copy" ? "task copies" : `${noun}s`;
  return `${count} ${count === 1 ? noun : plural}`;
}

function firstOutputLine(output: string): string | undefined {
  return output.split("\n").find((line) => line.trim() !== "")?.trim().slice(0, 120);
}

function firstOutputLineValue(value: OgaValue): string | undefined {
  const text = ogaStringValue(value);
  if (text !== undefined) return firstOutputLine(text);
  if (value === null) return undefined;
  if (Array.isArray(value)) return countResult(value, "items", "item");
  const tag = Object.prototype.toString.call(value);
  if (tag === "[object Boolean]" || tag === "[object Number]") return String(value);
  const object = ogaObject(value);
  return object === undefined ? undefined : `${Object.keys(object).length} field${Object.keys(object).length === 1 ? "" : "s"}`;
}

function queryResultSummary(text: string): string | undefined {
  const first = text.split("\n").find((line) => line.trim() !== "");
  if (first === undefined) return undefined;
  if (first.toLowerCase().includes("no confident match")) return "No matching code";
  const count = text.split("\n").filter(queryAnchorLine).length;
  return count > 0 ? formatCounted(count, "match") : firstOutputLine(text);
}

function queryAnchorLine(line: string): boolean {
  const trimmed = line.trim();
  if (trimmed.includes("(matched:")) return true;
  const separator = trimmed.indexOf(":");
  if (separator === -1) return false;
  const path = trimmed.slice(0, separator);
  const rest = trimmed.slice(separator + 1);
  if (!path.includes("/") && !path.includes(".")) return false;
  return /^\d/.test(rest);
}

function memoryResultSummary(value: OgaValue): string | undefined {
  if (Array.isArray(value)) return formatCounted(value.length, "note");
  if (value === null) return "No note found";
  const object = ogaObject(value);
  if (object === undefined) return firstOutputLineValue(value);
  if (object.removed === true) return "Note removed";
  if (object.removed === false) return "No note removed";
  if (object.key !== undefined && object.value !== undefined) return "Note found";
  return object.version === undefined ? firstOutputLineValue(value) : "Note saved";
}

function taskStateResult(value: OgaValue, subject: string): string | undefined {
  const state = ogaStringValue(ogaObject(value)?.state);
  if (state === undefined) return undefined;
  const label = ogaStateLabel(state) ?? "status unavailable";
  return `${subject} is ${label}`;
}

function actionResultSummary(value: OgaValue, fallback: string, key: string, noun: string): string {
  return countResult(value, key, noun) ?? errorResult(value) ?? fallback;
}

function errorResult(value: OgaValue): string | undefined {
  const errorValue = ogaObject(value)?.error;
  const error = ogaStringValue(errorValue) ?? firstString(ogaObject(errorValue), ["message", "detail", "error"]);
  return error === undefined ? undefined : `Error: ${error.slice(0, 120)}`;
}

function isOgaIdentifierKey(key: string): boolean {
  return ["id", "taskid", "parenttaskid", "orchestratorid", "grantid", "sessionid", "tooluseid", "callid", "sourceid"]
    .includes(key.replace(/[_-]/g, "").toLowerCase());
}

function parseOgaJson(raw: string): OgaValue | undefined {
  try {
    // SAFETY: JSON.parse returns only JSON primitives, arrays, and objects.
    return JSON.parse(raw) as OgaValue;
  } catch {
    return undefined;
  }
}

function unwrapJsonString(value: OgaValue): OgaValue {
  const text = ogaStringValue(value);
  if (text === undefined) return value;
  const parsed = parseOgaJson(text);
  return parsed === undefined ? value : parsed;
}

function ogaObject(value: OgaValue | undefined): OgaObject | undefined {
  if (value === undefined || value === null || Array.isArray(value)) return undefined;
  if (Object.prototype.toString.call(value) !== "[object Object]") return undefined;
  // SAFETY: the object tag excludes JSON primitives and arrays.
  return value as OgaObject;
}

function firstObject(object: OgaObject | undefined, keys: string[]): OgaObject | undefined {
  if (object === undefined) return undefined;
  for (const key of keys) {
    const value = ogaObject(object[key]);
    if (value !== undefined) return value;
  }
  return undefined;
}

function firstString(object: OgaObject | undefined, keys: string[]): string | undefined {
  if (object === undefined) return undefined;
  for (const key of keys) {
    const value = ogaStringValue(object[key]);
    if (value !== undefined) return value;
  }
  return undefined;
}

function ogaStringValue(value: OgaValue | undefined): string | undefined {
  if (value === undefined || Object.prototype.toString.call(value) !== "[object String]") return undefined;
  const text = String(value);
  return text === "" ? undefined : text;
}

function ogaOperation(tool: string | undefined, server: string | undefined, nestedTool: string | undefined): string | undefined {
  const normalized = tool?.trim().toLowerCase();
  if (normalized === undefined) return undefined;
  if (isMcpWrapper(normalized)) return isOgaServer(server) ? canonicalOgaOperation(nestedTool) : undefined;
  if (normalized.startsWith("mcp__")) {
    const parts = normalized.slice("mcp__".length).split("__");
    return isOgaServer(parts[0]) ? canonicalOgaOperation(parts.at(-1)) : undefined;
  }
  if (normalized.startsWith("oga_") || normalized.startsWith("oga-")) return canonicalOgaOperation(normalized);
  return isOgaServer(server) ? canonicalOgaOperation(normalized) : undefined;
}

function canonicalOgaOperation(value: string | undefined): string | undefined {
  if (value === undefined) return undefined;
  const operation = value.trim().toLowerCase().replace(/^oga[_-]/, "").replace(/_/g, "-");
  return operation === "" ? undefined : operation;
}

function isMcpWrapper(tool: string | undefined): boolean {
  return tool !== undefined && ["call_mcp_tool", "call_mcp", "mcp_tool"].includes(tool.toLowerCase());
}

function isOgaServer(value: string | undefined): boolean {
  return value !== undefined && ["oga", "oga-mcp"].includes(value.trim().toLowerCase());
}

function clipOga(value: string, limit: number): string {
  return value.length > limit ? `${value.slice(0, limit - 3)}...` : value;
}

function boundOgaResult(value: string): OgaResultText {
  const lines = value.split("\n");
  const hiddenLines = Math.max(lines.length - 200, 0);
  let text = lines.slice(0, 200).join("\n");
  if (Array.from(text).length > 12_000) text = `${Array.from(text).slice(0, 11_997).join("")}...`;
  return { text, hiddenLines };
}
