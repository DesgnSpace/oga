// Ported from rust/crates/oga-ui/src/settings/state.rs — keep behavior identical.
// Local to the settings screen; not part of the shared store.

import type {
  BrokerSummaryState,
  CleanupResult,
  CleanupSnapshot,
  HealthReport,
  McpInstallResult,
  MemoryEntry,
  MemoryProject,
  ModelSettingsModel,
  ModelSettingsSnapshot,
  ModelSettingsUpdate,
  ProfileView,
  ProjectList,
  PromptConfig,
  PromptWrite,
  Provider,
  WorkerSettings,
} from "@/bridge/types";

export type SettingsTab = "workers" | "connections" | "memories" | "prompts" | "storage" | "about";

export const SETTINGS_TABS: SettingsTab[] = ["workers", "connections", "memories", "prompts", "storage", "about"];

export function tabLabel(tab: SettingsTab): string {
  switch (tab) {
    case "workers":
      return "Workers";
    case "connections":
      return "Connections";
    case "memories":
      return "Memories";
    case "prompts":
      return "Prompts";
    case "storage":
      return "Storage";
    case "about":
      return "About";
  }
}

export type ProjectSettingsScope = { kind: "global" } | { kind: "project"; path: string };

export function scopeKey(scope: ProjectSettingsScope): string {
  return scope.kind === "global" ? "global" : scope.path;
}

export function scopeFromKey(raw: string): ProjectSettingsScope {
  return raw === "global" ? { kind: "global" } : { kind: "project", path: raw };
}

export function scopeCwd(scope: ProjectSettingsScope, projects: ProjectList | undefined): string | undefined {
  if (!projects) return undefined;
  return scope.kind === "global" ? projects.global : scope.path;
}

export type LoadState = "idle" | "loading" | "ready" | "error";


export function modelRowKey(profileId: string, modelId?: string): string {
  return modelId ? `${profileId}/${modelId}` : profileId;
}

export interface ModelSettingsStore {
  snapshot: ModelSettingsSnapshot | undefined;
  loading: boolean;
  loadError: string | undefined;
  saveError: string | undefined;
  pending: Set<string>;
  // Previous snapshot for rollback on conflict, per pending key.
  optimisticSnapshot: ModelSettingsSnapshot | undefined;
}

export function defaultModelSettingsStore(): ModelSettingsStore {
  return {
    snapshot: undefined,
    loading: false,
    loadError: undefined,
    saveError: undefined,
    pending: new Set(),
    optimisticSnapshot: undefined,
  };
}

export function beginModelLoad(store: ModelSettingsStore): ModelSettingsStore {
  return { ...store, loading: true, loadError: undefined, saveError: undefined };
}

export function applyModelSnapshot(store: ModelSettingsStore, snapshot: ModelSettingsSnapshot): ModelSettingsStore {
  return {
    ...store,
    snapshot,
    loading: false,
    loadError: undefined,
    saveError: undefined,
    pending: new Set(),
    optimisticSnapshot: undefined,
  };
}

export function applyModelLoadError(store: ModelSettingsStore, message: string): ModelSettingsStore {
  return { ...store, loading: false, loadError: message, snapshot: undefined };
}

export function beginModelUpdate(store: ModelSettingsStore, key: string): ModelSettingsStore {
  const pending = new Set(store.pending);
  pending.add(key);
  return {
    ...store,
    pending,
    saveError: undefined,
    optimisticSnapshot: store.optimisticSnapshot ?? store.snapshot,
  };
}

export function finishModelUpdate(
  store: ModelSettingsStore,
  key: string,
  result: { ok: true; snapshot: ModelSettingsSnapshot } | { ok: false; status?: number },
): ModelSettingsStore {
  const pending = new Set(store.pending);
  pending.delete(key);
  if (result.ok) {
    return {
      ...store,
      snapshot: result.snapshot,
      pending,
      saveError: undefined,
      optimisticSnapshot: pending.size === 0 ? undefined : store.optimisticSnapshot,
    };
  }
  // Roll back optimistic snapshot on conflict (409), otherwise keep optimistic
  // values but surface the error. When every pending clears, restore snapshot.
  if (result.status === 409 && store.optimisticSnapshot) {
    return {
      ...store,
      snapshot: store.optimisticSnapshot,
      pending,
      saveError: "This file changed outside Oga. Reload it before saving.",
      optimisticSnapshot: pending.size === 0 ? undefined : store.optimisticSnapshot,
    };
  }
  return {
    ...store,
    pending,
    saveError: "Couldn't save the model choice. Try again.",
    optimisticSnapshot: pending.size === 0 ? undefined : store.optimisticSnapshot,
  };
}

/**
 * Apply an optimistic patch to the snapshot before the server responds.
 * Used for allow/prefer toggles so the row updates immediately.
 */
export function applyOptimisticModelUpdate(
  store: ModelSettingsStore,
  workerId: string,
  modelId: string | undefined,
  patch: { enabled?: boolean | null; preferred?: boolean | null; capabilities?: string[] | null },
): ModelSettingsStore {
  if (!store.snapshot) return store;
  const snapshot: ModelSettingsSnapshot = {
    ...store.snapshot,
    workers: store.snapshot.workers.map((worker) => {
      if (worker.id !== workerId) return worker;
      const models = worker.models.map((model) => {
        if (modelId && model.id !== modelId) return model;
        if (!modelId && modelId !== undefined) return model;
        let next: ModelSettingsModel = { ...model };
        if (patch.enabled !== undefined) {
          if (patch.enabled === null) {
            next = { ...next, enabled: next.inheritedEnabled, hasEnabledOverride: false };
          } else {
            next = { ...next, enabled: patch.enabled, hasEnabledOverride: true };
          }
        }
        if (patch.preferred !== undefined) {
          if (patch.preferred === null) {
            next = { ...next, preferred: next.inheritedPreferred, hasPreferredOverride: false };
          } else {
            next = { ...next, preferred: patch.preferred, hasPreferredOverride: true };
          }
        }
        if (patch.capabilities !== undefined) {
          if (patch.capabilities === null) {
            next = {
              ...next,
              capabilities: [...next.inheritedCapabilities],
              hasCapabilitiesOverride: false,
            };
          } else {
            next = { ...next, capabilities: patch.capabilities, hasCapabilitiesOverride: true };
          }
        }
        return next;
      });
      // Update worker-level enabled counts derived from models, if needed — not stored separately.
      return { ...worker, models } as WorkerSettings;
    }),
  };
  return { ...store, snapshot };
}

export function modelRevision(store: ModelSettingsStore): string | undefined {
  return store.snapshot?.revision;
}


export type PromptsSaveError = { kind: "message"; message: string } | { kind: "unreachable" };

export interface PromptsModel {
  cwd: string;
  scope: string;
  written: boolean;
  text: string;
  inherited: string;
  configPath: string | undefined;
  loaded: boolean;
  loadError: string | undefined;
  saving: boolean;
  saveError: PromptsSaveError | undefined;
  saved: boolean;
  baselineWritten: boolean;
  baselineText: string;
}

export function defaultPromptsModel(): PromptsModel {
  return {
    cwd: "",
    scope: "global",
    written: false,
    text: "",
    inherited: "",
    configPath: undefined,
    loaded: false,
    loadError: undefined,
    saving: false,
    saveError: undefined,
    saved: false,
    baselineWritten: false,
    baselineText: "",
  };
}

export function applyPromptConfig(model: PromptsModel, snapshot: PromptConfig): PromptsModel {
  return {
    ...model,
    cwd: snapshot.cwd,
    scope: snapshot.scope,
    written: snapshot.written,
    text: snapshot.value,
    inherited: snapshot.inherited,
    configPath: snapshot.configPath,
    baselineWritten: snapshot.written,
    baselineText: snapshot.value,
    loaded: true,
    loadError: undefined,
    saveError: undefined,
    saved: false,
  };
}

export function updatePromptText(model: PromptsModel, text: string): PromptsModel {
  const written = model.written || true;
  return { ...model, text, written, saved: false };
}

export function resetPrompt(model: PromptsModel): PromptsModel {
  return { ...model, written: false, text: model.inherited, saved: false };
}

export function isPromptDirty(model: PromptsModel): boolean {
  return model.written !== model.baselineWritten || model.text !== model.baselineText;
}

export function promptPayload(model: PromptsModel): PromptWrite {
  return { cwd: model.cwd, written: model.written, value: model.text };
}

export function beginPromptLoad(model: PromptsModel): PromptsModel {
  return { ...model, loaded: false, loadError: undefined, saveError: undefined, saved: false };
}

export function applyPromptLoadError(model: PromptsModel, message: string): PromptsModel {
  return { ...model, loaded: false, loadError: message };
}

export function setPromptSaving(model: PromptsModel, saving: boolean): PromptsModel {
  return { ...model, saving, saveError: saving ? undefined : model.saveError };
}

export function finishPromptSave(
  model: PromptsModel,
  result: { ok: true; snapshot: PromptConfig } | { ok: false; error: PromptsSaveError },
): PromptsModel {
  if (result.ok) {
    return { ...applyPromptConfig(model, result.snapshot), saving: false, saved: true };
  }
  return { ...model, saving: false, saveError: result.error };
}


export interface MemoryState {
  projects: MemoryProject[];
  selectedProject: string | undefined;
  entries: MemoryEntry[];
  loading: boolean;
  loaded: boolean;
  error: string | undefined;
}

export function defaultMemoryState(): MemoryState {
  return { projects: [], selectedProject: undefined, entries: [], loading: false, loaded: false, error: undefined };
}

export function applyMemoryProjects(state: MemoryState, projects: MemoryProject[]): MemoryState {
  const stillExists = state.selectedProject ? projects.some((p) => p.cwd === state.selectedProject) : true;
  if (!stillExists) {
    return { ...state, projects, selectedProject: undefined, entries: [], loaded: false };
  }
  return { ...state, projects };
}

export function beginMemoryLoad(state: MemoryState, cwd: string): MemoryState {
  return { ...state, selectedProject: cwd, entries: [], loading: true, loaded: false, error: undefined };
}

export function applyMemoryEntries(state: MemoryState, entries: MemoryEntry[]): MemoryState {
  return { ...state, entries, loading: false, loaded: true, error: undefined };
}

export function applyMemoryError(state: MemoryState, message: string): MemoryState {
  return { ...state, loading: false, loaded: false, error: message };
}


export interface IntegrationState {
  loading: boolean;
  results: McpInstallResult[];
  error: string | undefined;
}

export interface CleanupState {
  snapshot: CleanupSnapshot | undefined;
  loading: boolean;
  saving: boolean;
  running: boolean;
  error: string | undefined;
  result: CleanupResult | undefined;
}

export function defaultCleanupState(): CleanupState {
  return {
    snapshot: undefined,
    loading: false,
    saving: false,
    running: false,
    error: undefined,
    result: undefined,
  };
}

export function defaultIntegrationState(): IntegrationState {
  return { loading: false, results: [], error: undefined };
}


export interface SettingsState {
  overview: LoadState;
  error: string | undefined;
  profiles: ProfileView[];
  projects: ProjectList | undefined;
  modelScope: ProjectSettingsScope;
  modelSettings: ModelSettingsStore;
  promptScope: ProjectSettingsScope;
  prompts: PromptsModel;
  memories: MemoryState;
  health: HealthReport | undefined;
  integrations: IntegrationState;
  cleanup: CleanupState;
}

export function defaultSettingsState(): SettingsState {
  return {
    overview: "idle",
    error: undefined,
    profiles: [],
    projects: undefined,
    modelScope: { kind: "global" },
    modelSettings: defaultModelSettingsStore(),
    promptScope: { kind: "global" },
    prompts: defaultPromptsModel(),
    memories: defaultMemoryState(),
    health: undefined,
    integrations: defaultIntegrationState(),
    cleanup: defaultCleanupState(),
  };
}

// First paint — held until the Workers tab (the tab that opens by default)
// has what it needs, so the modal shows one finished screen instead of a
// shell that repaints as each fetch lands. Cached across mounts so
// reopening settings after this has been true once skips the wait outright.

let cachedSettingsState: SettingsState | undefined;

export function readCachedSettingsState(): SettingsState | undefined {
  return cachedSettingsState;
}

export function writeCachedSettingsState(state: SettingsState): void {
  cachedSettingsState = state;
}

export function clearCachedSettingsState(): void {
  cachedSettingsState = undefined;
}

export function settingsFirstPaintReady(state: SettingsState): boolean {
  if (state.overview === "error") return true;
  if (state.overview !== "ready") return false;
  return state.modelSettings.snapshot !== undefined || state.modelSettings.loadError !== undefined;
}

export function applyOverview(
  state: SettingsState,
  summary: BrokerSummaryState,
  projects: ProjectList,
  health: HealthReport | undefined,
): SettingsState {
  return {
    ...state,
    profiles: summary.profiles,
    memories: applyMemoryProjects(state.memories, summary.memoryProjects),
    projects,
    health: health ?? state.health,
    overview: "ready",
    error: undefined,
  };
}


export function providerLabel(provider: Provider): string {
  switch (provider) {
    case "claude":
      return "Claude";
    case "codex":
      return "Codex";
    case "opencode":
      return "OpenCode";
    case "opencode-2":
      return "OpenCode 2";
    case "antigravity":
      return "Antigravity";
    case "pi":
      return "Pi";
  }
}

export function providerFromString(raw: string): Provider | undefined {
  switch (raw) {
    case "claude":
      return "claude";
    case "codex":
      return "codex";
    case "opencode":
      return "opencode";
    case "opencode-2":
      return "opencode-2";
    case "antigravity":
      return "antigravity";
    case "pi":
      return "pi";
    default:
      return undefined;
  }
}

export function supportedProviders(): Provider[] {
  return ["claude", "codex", "opencode", "opencode-2", "antigravity", "pi"];
}

export function isSecretKey(key: string): boolean {
  const upper = key.toUpperCase();
  return ["KEY", "TOKEN", "SECRET", "PASS"].some((part) => upper.includes(part));
}

export function profileError(action: string, status?: number, message?: string): string {
  if (status === 409) return "This worker changed outside Oga. Refresh settings, then re-enter your changes.";
  if (status !== undefined) return `Couldn't ${action} the worker: ${message ?? ""}`.trim();
  return `Couldn't ${action} the worker. Check that Oga is running, then try again.`;
}

export function projectName(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

export function buildModelSettingsUpdate(
  cwd: string,
  profileId: string,
  modelId: string | undefined,
  revision: string | undefined,
  patch: { enabled?: boolean | null; preferred?: boolean | null; capabilities?: string[] | null },
): ModelSettingsUpdate {
  return {
    cwd,
    profileId,
    modelId,
    expectedRevision: revision,
    ...(patch.enabled !== undefined ? { enabled: patch.enabled } : {}),
    ...(patch.preferred !== undefined ? { preferred: patch.preferred } : {}),
    ...(patch.capabilities !== undefined ? { capabilities: patch.capabilities } : {}),
  };
}
