import { memo, useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { broker, installMcpConfigs } from "@/bridge/client";
import { EmptyState } from "@/components/atoms/ListState";
import { absoluteTime, relativeTime } from "@/ui/time";
import { SearchField } from "@/components/SearchField";
import { ProviderLogo } from "@/components/atoms/ProviderLogo";
import { Switch } from "@/components/atoms/Switch";
import { SyntaxCode } from "@/components/SyntaxCode";
import { toast } from "@/state/toast";
import { workerToastName } from "@/lib/toast-subject";
import type { AppUpdateStatus } from "@/shell/useAppUpdates";
import type {
  CleanupSettings,
  CleanupSnapshot,
  LoveRule,
  McpInstallResult,
  MemoryEntry,
  ModelSettingsSnapshot,
  ProfileView,
  Provider,
  WaitSettings,
  WorkKind,
} from "@/bridge/types";
import {
  applyMemoryEntries,
  applyMemoryError,
  applyModelLoadError,
  applyModelSnapshot,
  applyOptimisticModelUpdate,
  applyOverview,
  applyPromptConfig,
  applyPromptLoadError,
  beginMemoryLoad,
  beginModelLoad,
  beginModelUpdate,
  beginPromptLoad,
  buildModelSettingsUpdate,
  defaultSettingsState,
  finishModelUpdate,
  finishPromptSave,
  isPromptDirty,
  isSecretKey,
  modelRowKey,
  projectName,
  promptPayload,
  providerFromString,
  providerLabel,
  readCachedSettingsState,
  resetPrompt,
  scopeCwd,
  scopeFromKey,
  scopeKey,
  settingsFirstPaintReady,
  setPromptSaving,
  supportedProviders,
  updatePromptText,
  writeCachedSettingsState,
} from "./state";
import type { ProjectSettingsScope, SettingsState, SettingsTab } from "./state";
import { SETTINGS_TABS, tabLabel } from "./state";

function defaultModelFor(provider: Provider): string {
  switch (provider) {
    case "claude":
      return "sonnet";
    case "codex":
      return "gpt-5";
    case "opencode":
      return "opencode";
    case "opencode-2":
      return "opencode-2";
    case "antigravity":
      return "antigravity";
    case "pi":
      return "pi";
  }
}

const SETTINGS_GROUPS: ReadonlyArray<{ label: string; tabs: readonly SettingsTab[] }> = [
  { label: "Workspace", tabs: ["workers", "connections"] },
  { label: "Worker context", tabs: ["memories", "prompts"] },
  { label: "Application", tabs: ["storage", "about"] },
];


export default function SettingsPage({
  open = true,
  offline = false,
  initialTab,
  updateStatus = { kind: "idle" },
  onCheckForUpdates = () => {},
  onInstallUpdate = () => {},
}: {
  open?: boolean;
  offline?: boolean;
  initialTab?: SettingsTab;
  updateStatus?: AppUpdateStatus;
  onCheckForUpdates?: () => void;
  onInstallUpdate?: () => void;
}) {
  const [state, setState] = useState<SettingsState>(() => readCachedSettingsState() ?? defaultSettingsState());
  const [activeTab, setActiveTab] = useState<SettingsTab>(initialTab ?? "workers");
  const [sectionQuery, setSectionQuery] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const wasOpen = useRef(false);
  const tabRefs = useRef<Partial<Record<SettingsTab, HTMLButtonElement | null>>>({});

  const visibleGroups = useMemo(() => {
    const needle = sectionQuery.trim().toLowerCase();
    return SETTINGS_GROUPS.map((group) => ({
      ...group,
      tabs: group.tabs.filter((tab) => !needle || tabLabel(tab).toLowerCase().includes(needle)),
    })).filter((group) => group.tabs.length > 0);
  }, [sectionQuery]);

  const visibleTabs = useMemo(() => visibleGroups.flatMap((group) => group.tabs), [visibleGroups]);

  const handleTabKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLButtonElement>, tab: SettingsTab) => {
      const currentIndex = visibleTabs.indexOf(tab);
      if (currentIndex === -1) return;

      let nextIndex: number | undefined;
      if (event.key === "Home") {
        nextIndex = 0;
      } else if (event.key === "End") {
        nextIndex = visibleTabs.length - 1;
      } else if (event.key === "ArrowUp") {
        nextIndex = (currentIndex - 1 + visibleTabs.length) % visibleTabs.length;
      } else if (event.key === "ArrowDown") {
        nextIndex = (currentIndex + 1) % visibleTabs.length;
      } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        nextIndex =
          event.key === "ArrowLeft"
            ? (currentIndex - 1 + visibleTabs.length) % visibleTabs.length
            : (currentIndex + 1) % visibleTabs.length;
      }

      if (nextIndex === undefined || nextIndex === currentIndex) return;
      event.preventDefault();
      const nextTab = visibleTabs[nextIndex];
      setActiveTab(nextTab);
      tabRefs.current[nextTab]?.focus();
    },
    [visibleTabs],
  );

  const loadOverview = useCallback(async () => {
    setState((s) => (s.overview === "ready" ? s : { ...s, overview: "loading", error: undefined }));
    const summaryResult = await broker.summary({ compact: true, limit: 1 });
    if (!summaryResult.ok) {
      setState((s) => ({ ...s, overview: "error", error: summaryResult.error.message }));
      return;
    }
    const projectsResult = await broker.projects();
    if (!projectsResult.ok) {
      setState((s) => ({ ...s, overview: "error", error: projectsResult.error.message }));
      return;
    }
    const healthResult = await broker.health();
    const health = healthResult.ok ? healthResult.value : undefined;
    setState((s) => applyOverview(s, summaryResult.value, projectsResult.value, health));
  }, []);

  const loadModelScope = useCallback(
    async (scope: ProjectSettingsScope, projects: SettingsState["projects"], refresh = false) => {
      const cwd = scopeCwd(scope, projects);
      setState((s) => ({ ...s, modelScope: scope, modelSettings: beginModelLoad(s.modelSettings) }));
      const result = await broker.modelSettings(cwd, refresh);
      if (result.ok) {
        setState((s) => ({ ...s, modelSettings: applyModelSnapshot(s.modelSettings, result.value) }));
      } else {
        setState((s) => ({ ...s, modelSettings: applyModelLoadError(s.modelSettings, result.error.message) }));
      }
    },
    [],
  );

  const loadPromptScope = useCallback(async (scope: ProjectSettingsScope, projects: SettingsState["projects"]) => {
    const cwd = scopeCwd(scope, projects);
    setState((s) => ({ ...s, promptScope: scope, prompts: beginPromptLoad(s.prompts) }));
    const result = await broker.prompt(cwd);
    if (result.ok) {
      setState((s) => ({ ...s, prompts: applyPromptConfig(s.prompts, result.value) }));
    } else {
      setState((s) => ({ ...s, prompts: applyPromptLoadError(s.prompts, result.error.message) }));
    }
  }, []);

  const loadCleanup = useCallback(async () => {
    setState((s) => ({ ...s, cleanup: { ...s.cleanup, loading: true, error: undefined } }));
    const result = await broker.cleanup();
    if (result.ok) {
      setState((s) => ({ ...s, cleanup: { ...s.cleanup, loading: false, snapshot: result.value } }));
    } else {
      setState((s) => ({ ...s, cleanup: { ...s.cleanup, loading: false, error: result.error.message } }));
    }
  }, []);

  useEffect(() => {
    if (settingsFirstPaintReady(state)) writeCachedSettingsState(state);
  }, [state]);

  useEffect(() => {
    void (async () => {
      await loadOverview();
    })();
  }, [loadOverview]);

  useEffect(() => {
    void loadCleanup();
  }, [loadCleanup]);

  useEffect(() => {
    if (state.overview !== "ready") return;
    if (!state.modelSettings.snapshot && !state.modelSettings.loading && !state.modelSettings.loadError) {
      void loadModelScope({ kind: "global" }, state.projects);
    }
    if (!state.prompts.loaded && !state.prompts.loadError && state.prompts.text === "" && state.prompts.inherited === "") {
      void loadPromptScope({ kind: "global" }, state.projects);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.overview]);

  useEffect(() => {
    if (!open) {
      wasOpen.current = false;
      return;
    }
    if (wasOpen.current) return;
    wasOpen.current = true;
    if (!state.modelSettings.snapshot) return;
    void loadModelScope(state.modelScope, state.projects, true);
  }, [loadModelScope, open, state.modelScope, state.modelSettings.snapshot, state.projects]);

  const handleRefresh = useCallback(async () => {
    setRefreshing(true);
    try {
      await loadOverview();
      await loadModelScope(state.modelScope, state.projects, true);
    } finally {
      setRefreshing(false);
    }
  }, [loadModelScope, loadOverview, state.modelScope, state.projects]);

  if (!settingsFirstPaintReady(state)) {
    return <SettingsSkeleton />;
  }

  return (
    <div className="settings-page">
      <aside className="settings-rail">
        <div className="settings-rail-header">
          <h1 id="settings-modal-title">Settings</h1>
          <SearchField className="settings-section-search" value={sectionQuery} onChange={setSectionQuery} placeholder="Search settings" />
        </div>
        <nav className="settings-tabs" aria-label="Settings sections" role="tablist" aria-orientation="vertical">
          {visibleGroups.length === 0 ? (
            <p className="settings-rail-empty">No sections match &ldquo;{sectionQuery}&rdquo;.</p>
          ) : (
            visibleGroups.map((group) => (
              <div className="settings-section-group" key={group.label}>
                <h2>{group.label}</h2>
                {group.tabs.map((tab) => (
                  <button
                    key={tab}
                    className={`settings-tab${activeTab === tab ? " settings-tab-active" : ""}`}
                    type="button"
                    role="tab"
                    id={`settings-tab-${tab}`}
                    tabIndex={activeTab === tab ? 0 : -1}
                    aria-controls={`settings-panel-${tab}`}
                    aria-selected={activeTab === tab}
                    ref={(element) => {
                      tabRefs.current[tab] = element;
                    }}
                    onKeyDown={(event) => handleTabKeyDown(event, tab)}
                    onClick={() => setActiveTab(tab)}
                  >
                    <span>{tabLabel(tab)}</span>
                  </button>
                ))}
              </div>
            ))
          )}
        </nav>
      </aside>

      <div className="settings-content">
        <header className="settings-header">
          <p className="settings-description">Choose how Oga works with your projects and command-line tools.</p>
        </header>

        {state.error ? (
          <div className="settings-message settings-message-error" role="alert">
            <strong>Couldn&apos;t load workspace settings</strong>
            <p>{state.error}</p>
            <button className="settings-button" type="button" onClick={handleRefresh}>
              Try again
            </button>
          </div>
        ) : null}

        <div
          id="settings-panel-workers"
          role="tabpanel"
          tabIndex={0}
          aria-labelledby="settings-tab-workers"
          hidden={activeTab !== "workers"}
          className={activeTab !== "workers" ? "settings-tab-panel-hidden" : "settings-workers-panel"}
        >
          <WorkersPanel
            state={state}
            setState={setState}
            loadModelScope={loadModelScope}
            onRefresh={handleRefresh}
            refreshing={refreshing}
            offline={offline}
          />
        </div>
        <div
          id="settings-panel-connections"
          role="tabpanel"
          tabIndex={0}
          aria-labelledby="settings-tab-connections"
          hidden={activeTab !== "connections"}
          className={activeTab !== "connections" ? "settings-tab-panel-hidden" : undefined}
        >
          <McpIntegrationPanel state={state} setState={setState} offline={offline} />
        </div>
        <div
          id="settings-panel-memories"
          role="tabpanel"
          tabIndex={0}
          aria-labelledby="settings-tab-memories"
          hidden={activeTab !== "memories"}
          className={activeTab !== "memories" ? "settings-tab-panel-hidden" : undefined}
        >
          <MemoriesPanel state={state} setState={setState} />
        </div>
        <div
          id="settings-panel-prompts"
          role="tabpanel"
          tabIndex={0}
          aria-labelledby="settings-tab-prompts"
          hidden={activeTab !== "prompts"}
          className={activeTab !== "prompts" ? "settings-tab-panel-hidden" : undefined}
        >
          <PromptsPanel state={state} setState={setState} loadPromptScope={loadPromptScope} offline={offline} />
        </div>
        <div
          id="settings-panel-storage"
          role="tabpanel"
          tabIndex={0}
          aria-labelledby="settings-tab-storage"
          hidden={activeTab !== "storage"}
          className={activeTab !== "storage" ? "settings-tab-panel-hidden" : undefined}
        >
          <CleanupPanel state={state} setState={setState} reload={loadCleanup} offline={offline} />
        </div>
        <div
          id="settings-panel-about"
          role="tabpanel"
          tabIndex={0}
          aria-labelledby="settings-tab-about"
          hidden={activeTab !== "about"}
          className={activeTab !== "about" ? "settings-tab-panel-hidden" : undefined}
        >
          <AboutPanel
            state={state}
            setState={setState}
            updateStatus={updateStatus}
            onCheckForUpdates={onCheckForUpdates}
            onInstallUpdate={onInstallUpdate}
          />
        </div>
      </div>
    </div>
  );
}

function SettingsSkeleton() {
  return (
    <div className="settings-page" aria-busy="true" aria-label="Loading settings…">
      <aside className="settings-rail" aria-hidden="true">
        <div className="settings-rail-header">
          <h1 id="settings-modal-title">Settings</h1>
          <span className="settings-skeleton-block settings-skeleton-search" />
        </div>
        <nav className="settings-tabs">
          {SETTINGS_TABS.map((tab) => (
            <span key={tab} className="settings-skeleton-block settings-skeleton-tab" />
          ))}
        </nav>
      </aside>
      <div className="settings-content">
        <header className="settings-header">
          <p className="settings-description">Choose how Oga works with your projects and command-line tools.</p>
        </header>
        <section className="settings-section">
          <div className="settings-worker-rows" aria-hidden="true">
            {[0, 1, 2, 3].map((row) => (
              <span key={row} className="settings-skeleton-block settings-skeleton-row" />
            ))}
          </div>
        </section>
      </div>
    </div>
  );
}

type EditorMode = { kind: "closed" } | { kind: "add" } | { kind: "edit"; profile: ProfileView };

function WaitingPanel() {
  const [settings, setSettings] = useState<WaitSettings | undefined>(undefined);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);

  useEffect(() => {
    let disposed = false;
    void broker.waiting().then((result) => {
      if (disposed) return;
      if (result.ok) setSettings(result.value);
      else setError(result.error.message);
    });
    return () => {
      disposed = true;
    };
  }, []);

  const save = async (next: WaitSettings) => {
    const previous = settings;
    setSettings(next);
    setSaving(true);
    const result = await broker.putWaiting(next);
    setSaving(false);
    if (result.ok) {
      setSettings(result.value);
      setError(undefined);
      return;
    }
    setSettings(previous);
    setError(result.error.message);
  };

  return (
    <section className="settings-section">
      <div className="settings-section-heading">
        <div>
          <p className="eyebrow">Interruptions</p>
          <h2>When a task is interrupted</h2>
        </div>
      </div>
      <p className="settings-helper">
        Tasks that lose the connection, or run out of usage, wait and pick up on their own.
      </p>
      {error ? <p className="settings-status">We couldn&apos;t load these choices. Try again in a moment.</p> : null}
      {settings ? (
        <div className="settings-option-list">
          <label className="settings-option">
            <span>
              <strong>Move the task to another worker</strong>
              <small>When usage runs out, hand the task to another worker you turned on that is at least as strong.</small>
            </span>
            <input
              type="checkbox"
              checked={settings.moveOnRateLimit}
              disabled={saving}
              onChange={(event) => void save({ ...settings, moveOnRateLimit: event.target.checked })}
            />
          </label>
          <label className="settings-option settings-option-field">
            <span>
              <strong>Wait for the connection up to</strong>
              <small>After this, the task stops and tells you the connection never came back.</small>
            </span>
            <span className="settings-number-field">
              <input
                type="number"
                min={1}
                max={1440}
                value={settings.networkMaxWaitMinutes}
                disabled={saving}
                aria-label="Minutes to wait for the connection"
                onChange={(event) =>
                  void save({ ...settings, networkMaxWaitMinutes: Number(event.target.value) })
                }
              />
              <span>minutes</span>
            </span>
          </label>
        </div>
      ) : null}
    </section>
  );
}

function ChevronGlyph({ expanded }: { expanded: boolean }) {
  return (
    <svg
      className={`settings-worker-row-chevron${expanded ? " settings-worker-row-chevron-open" : ""}`}
      viewBox="0 0 16 16"
      width={16}
      height={16}
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M5 6 8 9l3-3" />
    </svg>
  );
}

const WORK_LABELS = {
  context: "Reading and lookups",
  mechanical: "Small edits",
  build: "Building and fixing",
  reasoning: "Hard thinking",
  general: "Open-ended work",
  ui: "UI work",
  backend: "Backend work",
  database: "Database work",
  docs: "Docs and writing",
  tests: "Tests",
  review: "Reviews",
  research: "Research",
  refactor: "Refactoring",
} satisfies Record<WorkKind, string>;

function workLabel(when: WorkKind[]): string {
  if (when.length === 0) return "Everything else";
  const [first, ...rest] = when.map((kind) => WORK_LABELS[kind]);
  return [first, ...rest.map((label) => label.toLowerCase())].join(", ");
}

function destinationLabel(destination: { profileId?: string; model?: string }): string {
  if (destination.profileId && destination.model) return `${destination.profileId} · ${destination.model}`;
  return destination.profileId ?? destination.model ?? "";
}

function modelLabel(rule: LoveRule): string {
  const chain = rule.models?.length ? rule.models : [rule];
  return chain.map(destinationLabel).filter(Boolean).join(" → ");
}

function effortLabel(rule: LoveRule): string {
  const chain = rule.models?.length ? rule.models : [rule];
  if (chain.every((destination) => !destination.effort)) return "effort to suit the task";
  return `${chain.map((destination) => destination.effort ?? "as needed").join(" → ")} effort`;
}

/** Where work that names no model goes, kind of work by kind of work. */
function FavouriteModels({ rules }: { rules: LoveRule[] }) {
  if (rules.length === 0) return null;
  return (
    <div className="settings-favourites">
      <p className="eyebrow">Favourite models</p>
      <ul className="settings-favourite-rules">
        {rules.map((rule, index) => (
          <li className="settings-favourite-rule" key={`${rule.model}-${index}`}>
            <span className="settings-favourite-work">{workLabel(rule.when)}</span>
            <span className="settings-favourite-model">{modelLabel(rule)}</span>
            <span className="settings-muted">{effortLabel(rule)}</span>
          </li>
        ))}
      </ul>
      <p className="settings-helper">A task that names no model goes to the first listed model for that kind of work that can take it.</p>
    </div>
  );
}

function suggestedModels(state: SettingsState, profileId: string | undefined): string[] {
  const workers = state.modelSettings.snapshot?.workers ?? [];
  const pool = profileId ? workers.filter((w) => w.id === profileId) : workers;
  const seen = new Set<string>();
  for (const worker of pool) {
    for (const model of worker.models) {
      if (!seen.has(model.id)) seen.add(model.id);
    }
  }
  return [...seen].sort((a, b) => a.localeCompare(b)).slice(0, 200);
}

function WorkersPanel({
  state,
  setState,
  loadModelScope,
  onRefresh,
  refreshing,
  offline,
}: {
  state: SettingsState;
  setState: React.Dispatch<React.SetStateAction<SettingsState>>;
  loadModelScope: (scope: ProjectSettingsScope, projects: SettingsState["projects"]) => Promise<void>;
  onRefresh: () => Promise<void>;
  refreshing: boolean;
  offline: boolean;
}) {
  const [expandedId, setExpandedId] = useState<string | undefined>(undefined);
  const [editor, setEditor] = useState<EditorMode>({ kind: "closed" });
  const [deleteId, setDeleteId] = useState<string | undefined>(undefined);
  const [deleting, setDeleting] = useState(false);
  const [checkedAt, setCheckedAt] = useState(() => Date.now());

  const handleDelete = useCallback(
    async (id: string) => {
      if (deleting) return;
      setDeleteId(undefined);
      setDeleting(true);
      const name = workerToastName(state.profiles.find((profile) => profile.id === id)?.label);
      const quoted = name ? `"${name}"` : undefined;
      const lifecycle = toast.pending(quoted ? `Removing worker ${quoted}` : "Removing worker");
      const result = await broker.deleteProfile(id);
      if (result.ok) {
        lifecycle.dismiss();
        setState((s) => ({ ...s, profiles: s.profiles.filter((p) => p.id !== id) }));
        if (expandedId === id) setExpandedId(undefined);
        await onRefresh();
      } else {
        lifecycle.error(quoted ? `Couldn't remove worker ${quoted}` : "Couldn't remove worker", { description: "Try again.", detail: result.error.message });
      }
      setDeleting(false);
    },
    [deleting, expandedId, onRefresh, setState, state.profiles],
  );

  const handleToggle = useCallback(
    async (profileId: string, enabled: boolean) => {
      setState((s) => ({
        ...s,
        profiles: s.profiles.map((profile) => (profile.id === profileId ? { ...profile, enabled } : profile)),
      }));
      const result = await broker.updateProfile(profileId, { enabled });
      if (result.ok) {
        setState((s) => ({
          ...s,
          profiles: s.profiles.map((p) => (p.id === profileId ? result.value : p)),
        }));
        await onRefresh();
      } else {
        setState((s) => ({
          ...s,
          profiles: s.profiles.map((profile) =>
            profile.id === profileId && profile.enabled === enabled ? { ...profile, enabled: !enabled } : profile,
          ),
        }));
        const name = workerToastName(state.profiles.find((profile) => profile.id === profileId)?.label);
        toast.error(name ? `Couldn't update worker "${name}"` : "Couldn't update worker", { description: "Try again.", detail: result.error.message });
      }
    },
    [onRefresh, setState, state.profiles],
  );

  const handleRefresh = useCallback(async () => {
    await onRefresh();
    setCheckedAt(Date.now());
  }, [onRefresh]);

  return (
    <div className="settings-stack">
      <section className="settings-section">
        <div className="settings-section-heading">
          <div>
            <p className="eyebrow">Workers</p>
            <h2>Command-line workers</h2>
            <p className="settings-helper">Each worker keeps its own provider account, model, and environment.</p>
          </div>
          <div className="settings-inline-actions">
            <span className="settings-muted">Checked <span title={absoluteTime(checkedAt)}>{relativeTime(checkedAt)}</span></span>
            <button className="text-button" type="button" onClick={handleRefresh} disabled={refreshing}>
              Refresh
            </button>
            <button
              className="settings-button settings-button-primary"
              type="button"
              disabled={offline}
              onClick={() => setEditor({ kind: "add" })}
            >
              Add worker
            </button>
          </div>
        </div>

        <FavouriteModels rules={state.modelSettings.snapshot?.love ?? []} />

        <div className="settings-worker-rows">
          {editor.kind === "add" ? (
            <div className="settings-worker-row settings-worker-row-expanded settings-worker-row-editing">
              <ProfileEditor
                profile={undefined}
                models={suggestedModels(state, undefined)}
                offline={offline}
                setState={setState}
                onClose={() => setEditor({ kind: "closed" })}
                onRefresh={onRefresh}
                onSaved={(profile) => setExpandedId(profile.id)}
              />
            </div>
          ) : null}
          {state.overview === "loading" ? (
            <p className="settings-status">Loading workers…</p>
          ) : state.overview === "error" ? (
            <p className="settings-status">Workers are unavailable. Try again above.</p>
          ) : state.profiles.length === 0 ? (
            editor.kind === "add" ? null : (
              <EmptyState
                title="No workers yet"
                hint="Add a worker to choose where tasks run."
                className="settings-empty"
              />
            )
          ) : (
            state.profiles.map((profile) =>
              editor.kind === "edit" && editor.profile.id === profile.id ? (
                <div
                  key={profile.id}
                  className="settings-worker-row settings-worker-row-expanded settings-worker-row-editing"
                >
                  <ProfileEditor
                    profile={editor.profile}
                    models={suggestedModels(state, profile.id)}
                    offline={offline}
                    setState={setState}
                    onClose={() => setEditor({ kind: "closed" })}
                    onRefresh={onRefresh}
                    onDelete={() => {
                      setEditor({ kind: "closed" });
                      setDeleteId(profile.id);
                    }}
                  />
                </div>
              ) : (
                <WorkerRow
                  key={profile.id}
                  profile={profile}
                  worker={state.modelSettings.snapshot?.workers.find((w) => w.id === profile.id)}
                  expanded={expandedId === profile.id}
                  onToggleExpand={() => setExpandedId((id) => (id === profile.id ? undefined : profile.id))}
                  onToggle={handleToggle}
                  state={state}
                  setState={setState}
                  loadModelScope={loadModelScope}
                  onEdit={() => {
                    setExpandedId(profile.id);
                    setEditor({ kind: "edit", profile });
                  }}
                  onDelete={() => setDeleteId(profile.id)}
                  offline={offline}
                />
              ),
            )
          )}
        </div>
      </section>

      <WaitingPanel />

      {deleteId ? (
        <div className="settings-confirmation" role="alert">
          <div>
            <strong>Delete this worker?</strong>
            <p>Existing tasks keep their history. New tasks cannot use this worker, and you can&apos;t undo this here.</p>
          </div>
          <div className="settings-confirmation-actions">
            <button
              className="settings-button settings-button-danger"
              type="button"
              disabled={deleting || offline}
              onClick={() => handleDelete(deleteId)}
            >
              {deleting ? "Deleting…" : "Delete worker"}
            </button>
            <button className="settings-button" type="button" disabled={deleting} onClick={() => setDeleteId(undefined)}>
              Cancel
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}

function WorkerRow({
  profile,
  worker,
  expanded,
  onToggleExpand,
  onToggle,
  state,
  setState,
  loadModelScope,
  onEdit,
  onDelete,
  offline,
}: {
  profile: ProfileView;
  worker: ModelSettingsSnapshot["workers"][number] | undefined;
  expanded: boolean;
  onToggleExpand: () => void;
  onToggle: (id: string, enabled: boolean) => void;
  state: SettingsState;
  setState: React.Dispatch<React.SetStateAction<SettingsState>>;
  loadModelScope: (scope: ProjectSettingsScope, projects: SettingsState["projects"]) => Promise<void>;
  onEdit: () => void;
  onDelete: () => void;
  offline: boolean;
}) {
  const available = Boolean(worker?.configured) && profile.enabled;
  const modelCount = worker?.models.length ?? 0;
  const binaryPath = profile.command?.join(" ");
  const panelId = `worker-row-panel-${profile.id}`;

  return (
    <div className={`settings-worker-row${expanded ? " settings-worker-row-expanded" : ""}`}>
      <div className="settings-worker-row-header">
        <button
          className="settings-worker-row-main"
          type="button"
          aria-expanded={expanded}
          aria-controls={panelId}
          onClick={onToggleExpand}
        >
          <span className="settings-worker-row-mark">
            <ProviderLogo provider={profile.provider} size={22} />
            <span
              className={`settings-worker-row-dot${available ? " settings-worker-row-dot-on" : ""}`}
              aria-hidden="true"
            />
          </span>
          <span className="settings-worker-row-copy">
            <strong>{profile.label}</strong>
            <small>
              {binaryPath ? <span className="settings-mono">{binaryPath}</span> : null}
              {binaryPath ? " · " : ""}
              {modelCount} {modelCount === 1 ? "model" : "models"}
            </small>
          </span>
        </button>
        <div className="settings-worker-row-actions">
          <ChevronGlyph expanded={expanded} />
          <Switch
            className="settings-worker-row-switch"
            checked={profile.enabled}
            label=""
            accessibleName={`${profile.enabled ? "Disable" : "Enable"} ${profile.label} for tasks`}
            disabled={offline}
            onChange={(e) => onToggle(profile.id, e.target.checked)}
          />
        </div>
      </div>
      {expanded ? (
        <div className="settings-worker-row-panel" id={panelId}>
          <WorkerModelsSection
            profile={profile}
            state={state}
            setState={setState}
            loadModelScope={loadModelScope}
            offline={offline}
          />
          <div className="settings-worker-row-env">
            <h4>Environment</h4>
            {Object.keys(profile.env).length === 0 ? (
              <p className="settings-muted">No environment overrides.</p>
            ) : (
              Object.entries(profile.env).map(([key, value]) => (
                <div key={key} className="settings-env-row">
                  <SyntaxCode source={key} inline />
                  <SyntaxCode source={value} inline />
                </div>
              ))
            )}
          </div>
          <div className="settings-detail-actions">
            <button className="settings-button" type="button" disabled={offline} onClick={onEdit}>
              Edit
            </button>
            <button className="text-button text-button-danger" type="button" disabled={offline} onClick={onDelete}>
              Delete worker
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}

// WorkerModelsSection — which models this account may run, switched per model

function WorkerModelsSection({
  profile,
  state,
  setState,
  loadModelScope,
  offline,
}: {
  profile: ProfileView;
  state: SettingsState;
  setState: React.Dispatch<React.SetStateAction<SettingsState>>;
  loadModelScope: (scope: ProjectSettingsScope, projects: SettingsState["projects"]) => Promise<void>;
  offline: boolean;
}) {
  const [filter, setFilter] = useState("");

  const handleReload = useCallback(() => {
    void loadModelScope(state.modelScope, state.projects);
  }, [loadModelScope, state.modelScope, state.projects]);

  const handleScopeChange = useCallback(
    (raw: string) => {
      void loadModelScope(scopeFromKey(raw), state.projects);
    },
    [loadModelScope, state.projects],
  );

  const worker = state.modelSettings.snapshot?.workers.find((w) => w.id === profile.id);
  const orderedModelIds = useRef<string[]>([]);
  const modelIds = worker?.models.map((model) => model.id).join("\u0000") ?? "";
  if (worker && orderedModelIds.current.join("\u0000") !== modelIds) {
    orderedModelIds.current = [...worker.models]
      .sort((a, b) => {
        if (a.preferred !== b.preferred) return a.preferred ? -1 : 1;
        if (a.enabled !== b.enabled) return a.enabled ? -1 : 1;
        return (a.label || a.id).localeCompare(b.label || b.id);
      })
      .map((model) => model.id);
  }
  const models = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    const byId = new Map((worker?.models ?? []).map((model) => [model.id, model]));
    const ordered = orderedModelIds.current.flatMap((id) => {
      const model = byId.get(id);
      return model ? [model] : [];
    });
    if (!needle) return ordered;
    return ordered.filter(
      (model) => model.id.toLowerCase().includes(needle) || model.label.toLowerCase().includes(needle),
    );
  }, [filter, worker?.models]);

  /* An account can offer hundreds of models, so a row must not re-render when
     the filter text or an unrelated setting changes. Reading state through a
     ref keeps this callback's identity stable across those renders. */
  const latest = useRef(state);
  latest.current = state;

  const toggleModel = useCallback(
    async (modelId: string, enabled: boolean) => {
      const snapshot = latest.current;
      const key = modelRowKey(profile.id, modelId);
      const cwd = scopeCwd(snapshot.modelScope, snapshot.projects) ?? "";
      const update = buildModelSettingsUpdate(cwd, profile.id, modelId, snapshot.modelSettings.snapshot?.revision, {
        enabled,
      });

      setState((s) => {
        let next = beginModelUpdate(s.modelSettings, key);
        next = applyOptimisticModelUpdate(next, profile.id, modelId, { enabled });
        return { ...s, modelSettings: next };
      });

      const result = await broker.putModelSettings(update);
      if (result.ok) {
        setState((s) => ({
          ...s,
          modelSettings: finishModelUpdate(s.modelSettings, key, { ok: true, snapshot: result.value }),
        }));
      } else {
        toast.error("Couldn't update model settings", { description: "Try again.", detail: result.error.message });
        setState((s) => ({
          ...s,
          modelSettings: finishModelUpdate(s.modelSettings, key, { ok: false, status: result.error.status }),
        }));
      }
    },
    [profile.id, setState],
  );

  return (
    <section className="settings-worker-models">
      <div className="settings-section-heading">
        <h4>Models</h4>
        <label className="settings-scope-picker">
          <span>Applies to</span>
          <select value={scopeKey(state.modelScope)} onChange={(e) => handleScopeChange(e.target.value)}>
            <option value="global">All projects</option>
            {state.projects?.projects.map((path) => (
              <option key={path} value={path}>
                {projectName(path)}
              </option>
            ))}
          </select>
        </label>
      </div>
      {state.modelSettings.loading ? (
        <p className="settings-status" role="status">Discovering models…</p>
      ) : state.modelSettings.loadError ? (
        <div className="settings-message settings-message-error" role="alert">
          <strong>Couldn&apos;t read this account&apos;s models</strong>
          <p>{state.modelSettings.loadError}</p>
          <button className="settings-button" type="button" onClick={handleReload}>
            Try again
          </button>
        </div>
      ) : !worker ? (
        <p className="settings-status">Loading models…</p>
      ) : worker.models.length === 0 ? (
        <EmptyState
          title="No models yet"
          hint="Models appear here when this account provides them."
          className="settings-empty"
        />
      ) : (
        <>
          {worker.models.length > 8 ? (
            <SearchField className="settings-search" value={filter} onChange={setFilter} placeholder="Search models" />
          ) : null}
          <div className="settings-model-list">
            {models.map((model) => (
              <WorkerModelRow
                key={model.id}
                modelId={model.id}
                label={model.label || model.id}
                enabled={model.enabled}
                pending={state.modelSettings.pending.has(modelRowKey(profile.id, model.id))}
                offline={offline}
                onToggle={toggleModel}
              />
            ))}
          </div>
        </>
      )}
      {state.modelSettings.saveError ? (
        <p className="settings-form-error" role="alert">
          {state.modelSettings.saveError}
        </p>
      ) : null}
    </section>
  );
}

const WorkerModelRow = memo(function WorkerModelRow({
  modelId,
  label,
  enabled,
  pending,
  offline,
  onToggle,
}: {
  modelId: string;
  label: string;
  enabled: boolean;
  pending: boolean;
  offline: boolean;
  onToggle: (modelId: string, enabled: boolean) => void;
}) {
  return (
    <div className="settings-model-row" title={modelId}>
      <Switch
        checked={enabled}
        label={label}
        accessibleName={`${enabled ? "Disable" : "Enable"} ${label}`}
        disabled={pending || offline}
        onChange={(e) => onToggle(modelId, e.target.checked)}
      />
    </div>
  );
});


interface EnvDraft {
  id: number;
  key: string;
  value: string;
}

function ProfileEditor({
  profile,
  models,
  offline,
  setState,
  onClose,
  onRefresh,
  onSaved,
  onDelete,
}: {
  profile: ProfileView | undefined;
  models: string[];
  offline: boolean;
  setState: React.Dispatch<React.SetStateAction<SettingsState>>;
  onClose: () => void;
  onRefresh: () => Promise<void>;
  onSaved?: (profile: ProfileView) => void;
  onDelete?: () => void;
}) {
  const isNew = !profile;
  const [id, setId] = useState(profile?.id ?? "");
  const [label, setLabel] = useState(profile?.label ?? "");
  const [provider, setProvider] = useState<Provider>(profile?.provider ?? "claude");
  const [model, setModel] = useState(profile?.model ?? "");
  const [enabled, setEnabled] = useState(profile?.enabled ?? true);
  const [tags, setTags] = useState((profile?.capabilities ?? []).join(", "));
  const [envRows, setEnvRows] = useState<EnvDraft[]>(() => {
    if (!profile) return [];
    return Object.entries(profile.env).map(([key, value], idx) => ({ id: idx, key, value }));
  });
  const [touched, setTouched] = useState({ id: false, label: false, model: false, env: false });
  const [envError, setEnvError] = useState<string | undefined>(undefined);
  const [saveError, setSaveError] = useState<string | undefined>(undefined);
  const [saving, setSaving] = useState(false);
  const firstFieldRef = useRef<HTMLInputElement | null>(null);
  const modelListId = useId();
  const disabled = saving || offline;

  useEffect(() => {
    firstFieldRef.current?.focus();
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const idError = isNew && touched.id && id.trim() && !/^[a-z0-9][a-z0-9-]*$/.test(id.trim())
    ? "Use lowercase letters, numbers, and dashes."
    : undefined;
  const labelError = touched.label && !label.trim() ? "Enter a display name." : undefined;

  const validateEnv = useCallback((rows: EnvDraft[]): string | undefined => {
    const seen = new Set<string>();
    for (const row of rows) {
      const key = row.key.trim();
      if (!key) {
        if (row.value.trim()) return "Give every variable a name, or remove the empty row.";
        continue;
      }
      if (seen.has(key)) return `Variable “${key}” is listed twice.`;
      seen.add(key);
    }
    return undefined;
  }, []);

  const shownEnvError = touched.env ? (envError ?? validateEnv(envRows)) : envError;

  const handleSave = useCallback(async () => {
    if (saving) return;
    setTouched({ id: true, label: true, model: true, env: true });
    const labelValue = label.trim();
    if (!labelValue) return;
    if (isNew && id.trim() && !/^[a-z0-9][a-z0-9-]*$/.test(id.trim())) return;
    const nextEnvError = validateEnv(envRows);
    setEnvError(nextEnvError);
    if (nextEnvError) return;
    const env: Record<string, unknown> = {};
    for (const row of envRows) {
      const key = row.key.trim();
      if (!key) continue;
      env[key] = row.value;
    }
    const caps = tags
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    setSaving(true);
    setSaveError(undefined);
    const workerName = workerToastName(labelValue) ?? labelValue;
    const lifecycle = toast.pending(isNew ? `Adding worker "${workerName}"` : `Saving worker "${workerName}"`);
    const modelValue = model.trim() || undefined;
    if (profile) {
      const result = await broker.updateProfile(profile.id, {
        enabled,
        label: labelValue,
        provider,
        model: modelValue ?? "",
        capabilities: caps,
        env,
      });
      if (result.ok) {
        lifecycle.dismiss();
        setState((s) => ({
          ...s,
          profiles: s.profiles.map((p) => (p.id === profile.id ? result.value : p)),
        }));
        onClose();
        await onRefresh();
      } else {
        lifecycle.error(`Couldn't save worker "${workerName}"`, { description: "Check the details and try again.", detail: result.error.message });
        setSaveError("Couldn't save these changes. Try again.");
        setSaving(false);
      }
    } else {
      const result = await broker.createProfile({
        id: id.trim() || undefined,
        label: labelValue,
        provider,
        model: modelValue,
        enabled,
        env,
        capabilities: caps,
      });
      if (result.ok) {
        lifecycle.dismiss();
        setState((s) => ({ ...s, profiles: [...s.profiles, result.value] }));
        onSaved?.(result.value);
        onClose();
        await onRefresh();
      } else {
        lifecycle.error(`Couldn't add worker "${workerName}"`, { description: "Check the details and try again.", detail: result.error.message });
        setSaveError("Couldn't add this worker. Try again.");
        setSaving(false);
      }
    }
  }, [enabled, envRows, id, label, model, onClose, onRefresh, onSaved, profile, provider, saving, setState, tags, validateEnv, isNew]);

  const markTouched = (field: keyof typeof touched) => setTouched((t) => ({ ...t, [field]: true }));

  return (
    <article className="settings-worker-form-card" aria-label={isNew ? "Add worker" : `Edit ${profile?.label ?? "worker"}`}>
      <header className="settings-worker-form-heading">
        <div>
          <p className="eyebrow">Worker</p>
          <h3>{isNew ? "Add worker" : "Edit worker"}</h3>
        </div>
      </header>
      <form
        className="settings-worker-form"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          void handleSave();
        }}
      >
        <section className="settings-worker-form-section" aria-labelledby="worker-form-identity">
          <h4 id="worker-form-identity">Identity</h4>
          <div className="settings-worker-form-grid">
            {isNew ? (
              <div className="settings-worker-form-row">
                <label htmlFor="worker-form-id">Worker ID</label>
                <div className="settings-worker-form-field">
                  <input
                    id="worker-form-id"
                    ref={firstFieldRef}
                    spellCheck={false}
                    autoComplete="off"
                    value={id}
                    disabled={disabled}
                    aria-invalid={Boolean(idError)}
                    aria-describedby={idError ? "worker-form-id-error" : "worker-form-id-help"}
                    onChange={(e) => setId(e.target.value)}
                    onBlur={() => markTouched("id")}
                    placeholder="claude-work"
                  />
                  <small id="worker-form-id-help">Lowercase letters, numbers, and dashes. Set once.</small>
                  {idError ? (
                    <p className="settings-form-error" id="worker-form-id-error" role="alert">
                      {idError}
                    </p>
                  ) : null}
                </div>
              </div>
            ) : null}
            <div className="settings-worker-form-row">
              <label htmlFor="worker-form-name">Display name</label>
              <div className="settings-worker-form-field">
                <input
                  id="worker-form-name"
                  ref={isNew ? undefined : firstFieldRef}
                  spellCheck={false}
                  autoComplete="off"
                  value={label}
                  disabled={disabled}
                  aria-invalid={Boolean(labelError)}
                  aria-describedby={labelError ? "worker-form-name-error" : "worker-form-name-help"}
                  onChange={(e) => setLabel(e.target.value)}
                  onBlur={() => markTouched("label")}
                  placeholder="Claude work"
                />
                <small id="worker-form-name-help">Shown in the workers list.</small>
                {labelError ? (
                  <p className="settings-form-error" id="worker-form-name-error" role="alert">
                    {labelError}
                  </p>
                ) : null}
              </div>
            </div>
            <div className="settings-worker-form-row">
              <span id="worker-form-available-label">Availability</span>
              <div className="settings-worker-form-field">
                <label className="settings-toggle" aria-labelledby="worker-form-available-label">
                  <input
                    type="checkbox"
                    checked={enabled}
                    disabled={disabled}
                    onChange={(e) => setEnabled(e.target.checked)}
                  />
                  <span>Available for tasks</span>
                </label>
                <small>Turn off to pause new tasks on this worker.</small>
              </div>
            </div>
          </div>
        </section>

        <section className="settings-worker-form-section" aria-labelledby="worker-form-runs">
          <h4 id="worker-form-runs">Where it runs</h4>
          <div className="settings-worker-form-grid">
            <div className="settings-worker-form-row">
              <label htmlFor="worker-form-tool">Command-line tool</label>
              <div className="settings-worker-form-field">
                <span className="settings-provider-select">
                  <ProviderLogo provider={provider} size={18} />
                  <select
                    id="worker-form-tool"
                    value={provider}
                    disabled={disabled}
                    aria-describedby="worker-form-tool-help"
                    onChange={(e) => {
                      const next = providerFromString(e.target.value);
                      if (next) setProvider(next);
                    }}
                  >
                    {supportedProviders().map((p) => (
                      <option key={p} value={p}>
                        {providerLabel(p)}
                      </option>
                    ))}
                  </select>
                </span>
                <small id="worker-form-tool-help">The tool this worker signs in with.</small>
              </div>
            </div>
            <div className="settings-worker-form-row">
              <label htmlFor="worker-form-model">Default model</label>
              <div className="settings-worker-form-field">
                <input
                  id="worker-form-model"
                  spellCheck={false}
                  autoComplete="off"
                  value={model}
                  disabled={disabled}
                  list={modelListId}
                  aria-describedby="worker-form-model-help"
                  onChange={(e) => setModel(e.target.value)}
                  onBlur={() => markTouched("model")}
                  placeholder={defaultModelFor(provider)}
                />
                {models.length > 0 ? (
                  <datalist id={modelListId}>
                    {models.map((name) => (
                      <option key={name} value={name} />
                    ))}
                  </datalist>
                ) : null}
                <small id="worker-form-model-help">Leave blank for the tool default. Type to search known models.</small>
              </div>
            </div>
          </div>
        </section>

        <section className="settings-worker-form-section" aria-labelledby="worker-form-env">
          <h4 id="worker-form-env">Environment variables</h4>
          <div className="settings-worker-form-grid">
            <div className="settings-worker-form-row">
              <span id="worker-form-env-label">Variables</span>
              <div className="settings-worker-form-field" role="group" aria-labelledby="worker-form-env-label">
                {envRows.map((row) => (
                  <div key={row.id} className="settings-env-editor-row">
                    <input
                      className="settings-mono"
                      spellCheck={false}
                      autoComplete="off"
                      aria-label="Variable name"
                      value={row.key}
                      disabled={disabled}
                      onChange={(e) =>
                        setEnvRows((rows) => rows.map((r) => (r.id === row.id ? { ...r, key: e.target.value } : r)))
                      }
                      onBlur={() => markTouched("env")}
                      placeholder="NAME"
                    />
                    <input
                      className="settings-mono"
                      spellCheck={false}
                      autoComplete="off"
                      aria-label="Variable value"
                      type={isSecretKey(row.key) ? "password" : "text"}
                      value={row.value}
                      disabled={disabled}
                      onChange={(e) =>
                        setEnvRows((rows) => rows.map((r) => (r.id === row.id ? { ...r, value: e.target.value } : r)))
                      }
                      onBlur={() => markTouched("env")}
                      placeholder="Value"
                    />
                    <button
                      className="icon-button"
                      type="button"
                      aria-label={row.key.trim() ? `Remove ${row.key.trim()}` : "Remove variable"}
                      disabled={disabled}
                      onClick={() => {
                        markTouched("env");
                        setEnvRows((rows) => rows.filter((r) => r.id !== row.id));
                      }}
                    >
                      –
                    </button>
                  </div>
                ))}
                <button
                  className="text-button"
                  type="button"
                  disabled={disabled}
                  onClick={() =>
                    setEnvRows((rows) => {
                      const nid = rows.length ? Math.max(...rows.map((r) => r.id)) + 1 : 0;
                      return [...rows, { id: nid, key: "", value: "" }];
                    })
                  }
                >
                  Add variable
                </button>
                <small>One row per variable. Values stay on this device.</small>
                {shownEnvError ? (
                  <p className="settings-form-error" role="alert">
                    {shownEnvError}
                  </p>
                ) : null}
              </div>
            </div>
          </div>
        </section>

        <section className="settings-worker-form-section" aria-labelledby="worker-form-tags">
          <h4 id="worker-form-tags">Tags</h4>
          <div className="settings-worker-form-grid">
            <div className="settings-worker-form-row">
              <label htmlFor="worker-form-tags-input">Tags</label>
              <div className="settings-worker-form-field">
                <input
                  id="worker-form-tags-input"
                  spellCheck={false}
                  autoComplete="off"
                  value={tags}
                  disabled={disabled}
                  aria-describedby="worker-form-tags-help"
                  onChange={(e) => setTags(e.target.value)}
                  placeholder="build, review"
                />
                <small id="worker-form-tags-help">Separate tags with commas.</small>
              </div>
            </div>
          </div>
        </section>

        {saveError ? (
          <p className="settings-form-error" role="alert">
            {saveError}
          </p>
        ) : null}

        <footer className="settings-worker-form-footer">
          {onDelete ? (
            <button className="text-button text-button-danger" type="button" disabled={disabled} onClick={onDelete}>
              Delete worker
            </button>
          ) : (
            <span />
          )}
          <span className="settings-worker-form-footer-actions">
            <button className="settings-button" type="button" disabled={disabled} onClick={onClose}>
              Cancel
            </button>
            <button className="settings-button settings-button-primary" type="submit" disabled={disabled}>
              {saving ? "Saving…" : isNew ? "Add worker" : "Save changes"}
            </button>
          </span>
        </footer>
      </form>
    </article>
  );
}


function McpIntegrationPanel({
  state,
  setState,
  offline,
}: {
  state: SettingsState;
  setState: React.Dispatch<React.SetStateAction<SettingsState>>;
  offline: boolean;
}) {
  const handleInstall = useCallback(async () => {
    if (state.integrations.loading) return;
    setState((s) => ({ ...s, integrations: { loading: true, error: undefined, results: [] } }));
    const lifecycle = toast.pending("Connecting tools");
    const result = await installMcpConfigs(state.profiles);
    if (result.ok) {
      const failed = result.value.filter((item) => !item.success);
      if (failed.length === 0) lifecycle.dismiss();
      else lifecycle.error("Some tools could not connect", { detail: failed.map((item) => `${item.client}: ${item.message}`).join("\n") });
      setState((s) => ({ ...s, integrations: { loading: false, error: undefined, results: result.value } }));
    } else {
      lifecycle.error("Couldn't connect tools", { description: "Check the client settings and try again.", detail: result.error.message });
      setState((s) => ({ ...s, integrations: { loading: false, error: undefined, results: [] } }));
    }
  }, [state.integrations.loading, state.profiles, setState]);

  return (
    <section className="settings-section settings-integration-section">
      <div className="settings-section-heading">
        <div>
          <p className="eyebrow">Connections</p>
          <h2>Connect command-line tools</h2>
        </div>
        <button
          className="settings-button settings-button-primary"
          type="button"
          onClick={handleInstall}
          disabled={state.integrations.loading || offline}
        >
          {state.integrations.loading ? "Connecting…" : "Connect tools"}
        </button>
      </div>
      <p className="settings-helper">Add Oga to the supported client config files. Existing files are backed up before they change.</p>
      {state.integrations.results.length ? (
        <div className="settings-install-results" aria-live="polite">
          {state.integrations.results.map((result: McpInstallResult) => (
            <InstallResultRow key={result.path} result={result} />
          ))}
        </div>
      ) : null}
    </section>
  );
}

function InstallResultRow({ result }: { result: McpInstallResult }) {
  return (
    <div className={`settings-install-result${result.success ? "" : " settings-install-result-error"}`}>
      <span className="settings-install-mark" aria-hidden="true">
        {result.success ? "OK" : "!"}
      </span>
      <div>
        <strong>{result.client}</strong>
        <p>
          {result.message} - {result.path}
        </p>
      </div>
    </div>
  );
}


function MemoriesPanel({
  state,
  setState,
}: {
  state: SettingsState;
  setState: React.Dispatch<React.SetStateAction<SettingsState>>;
}) {
  const [query, setQuery] = useState("");

  const handleSelect = useCallback(
    async (cwd: string) => {
      setState((s) => ({ ...s, memories: beginMemoryLoad(s.memories, cwd) }));
      const result = await broker.memories(cwd);
      if (result.ok) {
        setState((s) => ({ ...s, memories: applyMemoryEntries(s.memories, result.value) }));
      } else {
        setState((s) => ({ ...s, memories: applyMemoryError(s.memories, result.error.message) }));
      }
    },
    [setState],
  );

  const handleReload = useCallback(async () => {
    const cwd = state.memories.selectedProject;
    if (!cwd) return;
    setState((s) => ({ ...s, memories: beginMemoryLoad(s.memories, cwd) }));
    const result = await broker.memories(cwd);
    if (result.ok) {
      setState((s) => ({ ...s, memories: applyMemoryEntries(s.memories, result.value) }));
    } else {
      setState((s) => ({ ...s, memories: applyMemoryError(s.memories, result.error.message) }));
    }
  }, [state.memories.selectedProject, setState]);

  const filtered = useMemo(() => {
    if (!state.memories.entries.length) return [];
    const needle = query.trim().toLowerCase();
    if (!needle) return state.memories.entries;
    return state.memories.entries.filter(
      (e: MemoryEntry) => e.key.toLowerCase().includes(needle) || e.value.toLowerCase().includes(needle),
    );
  }, [state.memories.entries, query]);

  return (
    <section className="settings-section settings-memory-section">
      <div className="settings-section-heading">
        <div>
          <p className="eyebrow">Memories</p>
          <h2>Project memories</h2>
        </div>
        <SearchField className="settings-search" value={query} onChange={setQuery} placeholder="Search memories" />
      </div>
      <p className="settings-helper">These notes are shared with workers in the project. Values stay on the broker until you open a project.</p>
      <div className="settings-memory-layout">
        <nav className="settings-memory-projects" aria-label="Projects with memories">
          {state.memories.projects.length === 0 ? (
            <EmptyState
              title="No memories yet"
              hint="Memories appear here after you add one to this project."
              className="settings-empty"
            />
          ) : (
            state.memories.projects.map((project) => (
              <button
                key={project.cwd}
                className={`settings-memory-project${state.memories.selectedProject === project.cwd ? " settings-memory-project-active" : ""}`}
                type="button"
                aria-pressed={state.memories.selectedProject === project.cwd}
                onClick={() => void handleSelect(project.cwd)}
              >
                <strong>{projectName(project.cwd)}</strong>
                <small>
                  {project.count} {project.count === 1 ? "memory" : "memories"}
                </small>
              </button>
            ))
          )}
        </nav>
        <div className="settings-memory-detail">
          {state.memories.loading ? (
            <p className="settings-status">Loading memories…</p>
          ) : state.memories.error ? (
            <div className="settings-message settings-message-error" role="alert">
              <strong>Couldn&apos;t read memories</strong>
              <p>{state.memories.error}</p>
              <button className="settings-button" type="button" onClick={handleReload}>
                Try again
              </button>
            </div>
          ) : !state.memories.selectedProject ? (
            <div className="settings-placeholder">
              <h3>Choose a project</h3>
              <p>Select a project to read its memories.</p>
            </div>
          ) : state.memories.entries.length === 0 ? (
            <EmptyState
              title="No memories yet"
              hint="Memories appear here after you add one to this project."
              className="settings-empty"
            />
          ) : filtered.length === 0 ? (
            <EmptyState
              title="No memories yet"
              hint="Try a different filter."
              className="settings-placeholder"
              action={<button className="text-button" type="button" onClick={() => setQuery("")}>Clear filter</button>}
            />
          ) : (
            <div className="settings-memory-list">
              {filtered.map((entry: MemoryEntry) => (
                <article key={entry.key} className="settings-memory-card">
                  <header>
                    <SyntaxCode source={entry.key} inline />
                    <small>Version {entry.version}</small>
                  </header>
                  <p>{entry.value}</p>
                </article>
              ))}
            </div>
          )}
        </div>
      </div>
    </section>
  );
}


function PromptsPanel({
  state,
  setState,
  loadPromptScope,
  offline,
}: {
  state: SettingsState;
  setState: React.Dispatch<React.SetStateAction<SettingsState>>;
  loadPromptScope: (scope: ProjectSettingsScope, projects: SettingsState["projects"]) => Promise<void>;
  offline: boolean;
}) {
  const handleScopeChange = useCallback(
    (raw: string) => {
      const scope = scopeFromKey(raw);
      void loadPromptScope(scope, state.projects);
    },
    [loadPromptScope, state.projects],
  );

  const handleReload = useCallback(() => {
    void loadPromptScope(state.promptScope, state.projects);
  }, [loadPromptScope, state.promptScope, state.projects]);

  const handleSave = useCallback(async () => {
    const dirty = isPromptDirty(state.prompts);
    if (!dirty || state.prompts.saving) return;
    setState((s) => ({ ...s, prompts: setPromptSaving(s.prompts, true) }));
    const payload = promptPayload(state.prompts);
    const lifecycle = toast.pending("Saving instructions");
    const result = await broker.putPrompt(payload);
    if (result.ok) {
      lifecycle.dismiss();
      setState((s) => ({ ...s, prompts: finishPromptSave(s.prompts, { ok: true, snapshot: result.value }) }));
    } else {
      const err =
        result.error.status !== undefined
          ? { kind: "message" as const, message: result.error.message }
          : { kind: "unreachable" as const };
      lifecycle.error("Couldn't save instructions", {
        description: err.kind === "unreachable" ? "Check that Oga is running, then try again." : "Try again.",
        detail: err.kind === "message" ? err.message : undefined,
      });
      setState((s) => ({ ...s, prompts: finishPromptSave(s.prompts, { ok: false, error: err }) }));
    }
  }, [state.prompts, setState]);

  const handleReset = useCallback(() => {
    setState((s) => ({ ...s, prompts: resetPrompt(s.prompts) }));
  }, [setState]);

  const handleTextChange = useCallback(
    (text: string) => {
      setState((s) => ({ ...s, prompts: updatePromptText(s.prompts, text) }));
    },
    [setState],
  );

  return (
    <section className="settings-section settings-prompts-section">
      <div className="settings-section-heading">
        <div>
          <p className="eyebrow">Prompts</p>
          <h2>Worker instructions</h2>
        </div>
        <label className="settings-scope-picker">
          <span>Applies to</span>
          <select value={scopeKey(state.promptScope)} onChange={(e) => handleScopeChange(e.target.value)}>
            <option value="global">All projects</option>
            {state.projects?.projects.map((path) => (
              <option key={path} value={path}>
                {projectName(path)}
              </option>
            ))}
          </select>
        </label>
      </div>
      {state.prompts.loadError ? (
        <div className="settings-message settings-message-error" role="alert">
          <strong>Couldn&apos;t read these instructions</strong>
          <p>{state.prompts.loadError}</p>
          <button className="settings-button" type="button" onClick={handleReload}>
            Try again
          </button>
        </div>
      ) : !state.prompts.loaded ? (
        <p className="settings-status">Loading instructions…</p>
      ) : (
        <div className="settings-prompt-editor">
          <div className="settings-prompt-meta">
            <span className="settings-muted">
              {state.prompts.configPath ? "Set in .oga.yaml" : state.prompts.written ? "Set for this scope" : "Inherited"}
            </span>
            {state.prompts.configPath === undefined && state.prompts.written ? (
              <button className="text-button" type="button" onClick={handleReset}>
                Use inherited
              </button>
            ) : null}
          </div>
          <textarea
            value={state.prompts.text}
            onChange={(e) => handleTextChange(e.target.value)}
            readOnly={state.prompts.configPath !== undefined}
            aria-label="Worker instructions"
          />
          <p className="settings-helper">What every worker is told before it starts, including how to report back.</p>
          {state.prompts.configPath !== undefined ? (
            <p className="settings-helper">Set in {state.prompts.configPath}. Edit that file to change them.</p>
          ) : (
            <div className="settings-form-footer">
              {isPromptDirty(state.prompts) ? (
                <span className="settings-muted">Unsaved changes</span>
              ) : state.prompts.saved ? (
                <span className="settings-muted">Saved for new tasks</span>
              ) : null}
              <button
                className="settings-button settings-button-primary"
                type="button"
                onClick={handleSave}
                disabled={state.prompts.saving || !isPromptDirty(state.prompts) || offline}
              >
                {state.prompts.saving ? "Saving…" : "Save changes"}
              </button>
            </div>
          )}
        </div>
      )}
    </section>
  );
}


function CleanupPanel({
  state,
  setState,
  reload,
  offline,
}: {
  state: SettingsState;
  setState: React.Dispatch<React.SetStateAction<SettingsState>>;
  reload: () => Promise<void>;
  offline: boolean;
}) {
  const [draft, setDraft] = useState<CleanupSettings | undefined>(undefined);
  const [confirming, setConfirming] = useState(false);
  const snapshot = state.cleanup.snapshot;
  const settings = draft ?? snapshot?.settings;
  const hasUnsavedChanges = Boolean(
    settings && snapshot && (
      settings.enabled !== snapshot.settings.enabled ||
      settings.olderThanDays !== snapshot.settings.olderThanDays ||
      settings.archivedOnly !== snapshot.settings.archivedOnly
    ),
  );

  useEffect(() => {
    if (snapshot) setDraft(snapshot.settings);
  }, [snapshot]);

  const updateDraft = (patch: Partial<CleanupSettings>) => {
    if (settings) setDraft({ ...settings, ...patch });
  };

  const save = async () => {
    if (!settings || state.cleanup.saving) return;
    setState((s) => ({ ...s, cleanup: { ...s.cleanup, saving: true, error: undefined } }));
    const lifecycle = toast.pending("Saving history settings");
    const result = await broker.putCleanup(settings);
    if (result.ok) {
      lifecycle.success("History settings saved");
      setState((s) => ({ ...s, cleanup: { ...s.cleanup, saving: false, snapshot: result.value } }));
      setDraft(result.value.settings);
    } else {
      lifecycle.error("Couldn't save history settings", { description: "Try again.", detail: result.error.message });
      setState((s) => ({ ...s, cleanup: { ...s.cleanup, saving: false, error: result.error.message } }));
    }
  };

  const run = async () => {
    if (state.cleanup.running) return;
    setConfirming(false);
    setState((s) => ({ ...s, cleanup: { ...s.cleanup, running: true, error: undefined } }));
    const lifecycle = toast.pending("Removing old logs");
    const result = await broker.runCleanup();
    if (result.ok) {
      lifecycle.dismiss();
      setState((s) => ({
        ...s,
        cleanup: { ...s.cleanup, running: false, result: result.value, snapshot: result.value },
      }));
    } else {
      lifecycle.error("Couldn't remove old logs", { description: "Try again.", detail: result.error.message });
      setState((s) => ({ ...s, cleanup: { ...s.cleanup, running: false, error: result.error.message } }));
    }
  };

  return (
    <section className="settings-section settings-storage-section">
      <div className="settings-section-heading">
        <div>
          <p className="eyebrow">Task history</p>
          <h2>Manage task history</h2>
        </div>
        <button className="text-button" type="button" onClick={() => void reload()} disabled={state.cleanup.loading}>
          Refresh
        </button>
      </div>
      <p className="settings-helper">Oga keeps each task&apos;s request and answer. These choices remove only its detailed activity.</p>
      {state.cleanup.loading && !settings ? <p className="settings-status">Checking stored activity…</p> : null}
      {settings ? (
        <>
          <div className="settings-option-list">
            <label className="settings-option">
              <span>
                <strong>Remove old logs automatically</strong>
                <small>Remove logs after they reach the age you choose.</small>
              </span>
              <input type="checkbox" checked={settings.enabled} onChange={(event) => updateDraft({ enabled: event.target.checked })} />
            </label>
            <label className="settings-option settings-option-field">
              <span>
                <strong>Keep logs for</strong>
                <small>Logs older than this can be removed.</small>
              </span>
              <span className="settings-number-field">
                <input
                  type="number"
                  min={1}
                  max={3650}
                  value={settings.olderThanDays}
                  onChange={(event) => updateDraft({ olderThanDays: Number(event.target.value) })}
                  aria-label="Days to keep logs"
                />
                <span>days</span>
              </span>
            </label>
            <label className="settings-option">
              <span>
                <strong>Remove logs only for archived tasks</strong>
                <small>Logs for active and unarchived tasks stay.</small>
              </span>
              <input type="checkbox" checked={settings.archivedOnly} onChange={(event) => updateDraft({ archivedOnly: event.target.checked })} />
            </label>
          </div>
          <div className="settings-inline-actions">
            <button className="settings-button settings-button-primary" type="button" onClick={() => void save()} disabled={state.cleanup.saving || offline}>
              {state.cleanup.saving ? "Saving…" : "Save storage choices"}
            </button>
          </div>
          {snapshot ? <CleanupPreview plan={snapshot.plan} /> : null}
          <div className="settings-cleanup-actions">
            {confirming ? (
              <div className="settings-confirmation" role="alert">
                <div>
                  <strong>Remove these old logs?</strong>
                  <p>This cannot be undone. Task requests and answers stay.</p>
                </div>
                <div className="settings-confirmation-actions">
                  <button className="settings-button settings-button-danger" type="button" onClick={() => void run()} disabled={state.cleanup.running || offline}>Remove logs now</button>
                  <button className="settings-button" type="button" onClick={() => setConfirming(false)}>Cancel</button>
                </div>
              </div>
            ) : (
              <button className="settings-button settings-button-danger" type="button" onClick={() => setConfirming(true)} disabled={state.cleanup.running || hasUnsavedChanges || !snapshot?.plan.events || offline}>
                {state.cleanup.running ? "Removing…" : "Review and remove now"}
              </button>
            )}
          </div>
          {hasUnsavedChanges ? <p className="settings-muted">Save your history settings to refresh this preview before removing logs.</p> : null}
          {state.cleanup.result ? <p className="settings-success" role="status">Removed {state.cleanup.result.plan.events.toLocaleString()} logs and reclaimed {formatBytes(state.cleanup.result.fileBytesBefore - state.cleanup.result.fileBytesAfter)}.</p> : null}
        </>
      ) : null}
    </section>
  );
}

function CleanupPreview({ plan }: { plan: CleanupSnapshot["plan"] }) {
  return (
    <div className="settings-cleanup-preview" aria-live="polite">
      <div>
        <p className="eyebrow">Preview</p>
        <h3>{plan.events ? "Logs ready to remove" : "Nothing to remove"}</h3>
      </div>
      <dl className="settings-health-list">
        <div><dt>Tasks</dt><dd>{plan.tasks.toLocaleString()}</dd></div>
        <div><dt>Logs</dt><dd>{plan.events.toLocaleString()}</dd></div>
        <div><dt>Space to free</dt><dd>{formatBytes(plan.bytes)}</dd></div>
      </dl>
      {plan.heldBack ? <p className="settings-muted">{plan.heldBack.toLocaleString()} task{plan.heldBack === 1 ? "" : "s"} held back because related work is not ready.</p> : null}
    </div>
  );
}

function formatBytes(value: number): string {
  if (value < 1000) return `${value} B`;
  if (value < 1_000_000) return `${(value / 1000).toFixed(1)} kB`;
  if (value < 1_000_000_000) return `${(value / 1_000_000).toFixed(1)} MB`;
  return `${(value / 1_000_000_000).toFixed(1)} GB`;
}


function AboutPanel({
  state,
  setState,
  updateStatus,
  onCheckForUpdates,
  onInstallUpdate,
}: {
  state: SettingsState;
  setState: React.Dispatch<React.SetStateAction<SettingsState>>;
  updateStatus: AppUpdateStatus;
  onCheckForUpdates: () => void;
  onInstallUpdate: () => void;
}) {
  const handleRefresh = useCallback(async () => {
    const result = await broker.health();
    if (result.ok) {
      setState((s) => ({ ...s, health: result.value }));
    } else {
      toast.error("Couldn't check Oga", { description: "Check that the broker is running and try again.", detail: result.error.message });
    }
  }, [setState]);

  return (
    <section className="settings-section settings-about-section">
      <div className="settings-about-hero">
        <div>
          <p className="eyebrow">About</p>
          <h2>Oga</h2>
          <p className="settings-mono">Desktop client</p>
        </div>
      </div>
      <div className="settings-about-card">
        {state.overview === "loading" ? (
          <p className="settings-status">Checking Oga…</p>
        ) : state.health ? (
          <>
            <div className="settings-health-heading">
              <span className="settings-health-dot" />
              <strong>Oga is running</strong>
            </div>
            <dl className="settings-health-list">
              <div>
                <dt>Version</dt>
                <dd>{state.health.version}</dd>
              </div>
              <div>
                <dt>Connection version</dt>
                <dd>v{state.health.mcpContractVersion}</dd>
              </div>
              <div>
                <dt>Build</dt>
                <dd className="settings-mono">{state.health.build}</dd>
              </div>
            </dl>
            {state.health.stale ? (
              <p className="settings-form-error">{state.health.hint ?? "A newer source build is available."}</p>
            ) : null}
          </>
        ) : (
          <div className="settings-message settings-message-error">
            <strong>Oga is unreachable</strong>
            <p>Check that the broker is running, then try again.</p>
            <button className="settings-button" type="button" onClick={handleRefresh}>
              Try again
            </button>
          </div>
        )}
      </div>
      <div className="settings-about-card" aria-live="polite">
        <div>
          <p className="eyebrow">Updates</p>
          <h3>Keep Oga current</h3>
        </div>
        {updateStatus.kind === "available" ? (
          <>
            <p>Version {updateStatus.version} is ready to install.</p>
            {updateStatus.notes ? <p className="settings-helper">{updateStatus.notes}</p> : null}
            <button className="settings-button settings-button-primary" type="button" onClick={onInstallUpdate}>
              Download and install
            </button>
          </>
        ) : updateStatus.kind === "checking" ? (
          <p className="settings-status">Checking for a new version…</p>
        ) : updateStatus.kind === "downloading" ? (
          <p className="settings-status">
            Downloading version {updateStatus.version}{updateStatus.progress === undefined ? "…" : ` (${updateStatus.progress}%)`}
          </p>
        ) : updateStatus.kind === "installing" ? (
          <p className="settings-status">Installing version {updateStatus.version}…</p>
        ) : updateStatus.kind === "up-to-date" ? (
          <>
            <p className="settings-status">You have the latest version.</p>
            <button className="settings-button" type="button" onClick={onCheckForUpdates}>
              Check for updates
            </button>
          </>
        ) : updateStatus.kind === "failed" ? (
          <>
            <p className="settings-form-error">We couldn&apos;t check for a new version. Try again in a moment.</p>
            <button className="settings-button" type="button" onClick={onCheckForUpdates}>
              Try again
            </button>
          </>
        ) : updateStatus.kind === "unavailable" ? (
          <p className="settings-status">Updates are available in the desktop app.</p>
        ) : (
          <button className="settings-button" type="button" onClick={onCheckForUpdates}>
            Check for updates
          </button>
        )}
      </div>
    </section>
  );
}
