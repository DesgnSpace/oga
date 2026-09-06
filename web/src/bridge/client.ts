// The broker, as the web view sees it. Every method wraps one `BrokerCall`
// and never throws — a failure comes back as a typed `BridgeResult`.

import { getTransport } from "./transport";
import type {
  BrokerCall,
  BrokerCallResult,
  BridgeError,
  BridgeResult,
  CompletionRequest,
  CleanupSettings,
  HandoffRequest,
  McpInstallResult,
  ModelSettingsUpdate,
  ProfileCreate,
  ProfilePatch,
  ProfileView,
  PromptWrite,
  ReplyRequest,
  ResumeRequest,
  StateQuery,
  SteerRequest,
  StreamStatus,
  TaskEventsQuery,
  TaskSnapshot,
  WaitSettings,
} from "./types";

export interface ImagePreview {
  bytes: number[];
  mime: string;
}

async function call<C extends BrokerCall["call"]>(
  request: Extract<BrokerCall, { call: C }>,
): Promise<BridgeResult<BrokerCallResult[C]>> {
  try {
    const value = await getTransport().invoke<BrokerCallResult[C]>("broker_call", {
      call: request,
    });
    return { ok: true, value };
  } catch (error) {
    return { ok: false, error: error as BridgeError };
  }
}

async function command<T>(
  name: string,
  args?: Record<string, unknown>,
): Promise<BridgeResult<T>> {
  try {
    const value = await getTransport().invoke<T>(name, args);
    return { ok: true, value };
  } catch (error) {
    return { ok: false, error: error as BridgeError };
  }
}

export const broker = {
  health: () => call({ call: "health" }),

  summary: (query: StateQuery) => call({ call: "summary", query }),

  usage: (tzOffset: number) => call<"usage">({ call: "usage", tzOffset }),

  task: (taskId: string) => call({ call: "task", taskId }),

  taskEvents: (taskId: string, query: TaskEventsQuery) =>
    call({ call: "taskEvents", taskId, query }),

  taskDiff: (taskId: string) => call({ call: "taskDiff", taskId }),

  projects: () => call({ call: "projects" }),

  memories: (cwd: string) => call({ call: "memories", cwd }),

  prompt: (cwd?: string) => call({ call: "prompt", cwd }),

  putPrompt: (request: PromptWrite) => call({ call: "putPrompt", request }),

  modelSettings: (cwd?: string, refresh = false) => call({ call: "modelSettings", cwd, refresh }),

  cleanup: () => call({ call: "cleanup" }),

  putCleanup: (settings: CleanupSettings) => call({ call: "putCleanup", settings }),

  runCleanup: () => call({ call: "runCleanup" }),

  waiting: () => call({ call: "waiting" }),

  putWaiting: (settings: WaitSettings) => call({ call: "putWaiting", settings }),

  putModelSettings: (request: ModelSettingsUpdate) => call({ call: "putModelSettings", request }),

  resetModelSettings: (cwd?: string, revision?: string) =>
    call({ call: "resetModelSettings", cwd, revision }),

  createProfile: (profile: ProfileCreate) => call({ call: "createProfile", profile }),

  updateProfile: (profileId: string, patch: ProfilePatch) =>
    call({ call: "updateProfile", profileId, patch }),

  deleteProfile: (profileId: string) => call({ call: "deleteProfile", profileId }),

  archiveTask: (taskId: string, archived: boolean, deleteBranch = false) =>
    call<"archiveTask">({ call: "archiveTask", taskId, archived, deleteBranch }),

  cancelTask: (taskId: string) => call({ call: "cancelTask", taskId }),

  resumeTask: (taskId: string, request: ResumeRequest) => call({ call: "resumeTask", taskId, request }),

  replyTask: (taskId: string, request: ReplyRequest) => call({ call: "replyTask", taskId, request }),

  steerTask: (taskId: string, request: SteerRequest) => call({ call: "steerTask", taskId, request }),

  handoffTask: (taskId: string, request: HandoffRequest) => call({ call: "handoffTask", taskId, request }),

  completeTask: (taskId: string, request: CompletionRequest) =>
    call({ call: "completeTask", taskId, request }),

  removeFollowUp: (taskId: string, index: number) => call({ call: "removeFollowUp", taskId, index }),

};

/** The shell's current view of its single broker stream connection. */
export function streamStatus(): Promise<BridgeResult<StreamStatus>> {
  return command<StreamStatus>("broker_stream_status");
}

/**
 * Asks the shell to follow a task, and reads back everything needed to draw
 * it. From then on the shell pushes only what changes, over `onTaskDelta`.
 * Asking again for a task already being followed resynchronises it.
 */
export function watchTask(taskId: string, events: number): Promise<BridgeResult<TaskSnapshot>> {
  return command<TaskSnapshot>("broker_watch_task", { taskId, events });
}

/** Tells the shell to stop following a task. */
export function unwatchTask(taskId: string): Promise<BridgeResult<void>> {
  return command<void>("broker_unwatch_task", { taskId });
}

/** Writes Oga's MCP config into every detected agent host, filesystem-side. */
export function installMcpConfigs(
  profiles: ProfileView[],
): Promise<BridgeResult<McpInstallResult[]>> {
  return command<McpInstallResult[]>("install_mcp_configs", { profiles });
}

export function readImagePreview(path: string): Promise<BridgeResult<ImagePreview>> {
  return command<ImagePreview>("read_image_preview", { path });
}

/** Opens an attachment in its OS default app, same as double-clicking it. */
export function openAttachment(path: string): Promise<BridgeResult<void>> {
  return command<void>("open_attachment", { path });
}

/** Grays out or re-enables a native menu item by its `oga-menu-command` id. */
export function setMenuItemEnabled(id: string, enabled: boolean): Promise<BridgeResult<void>> {
  return command<void>("set_menu_item_enabled", { id, enabled });
}
