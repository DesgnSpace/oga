import { memo, useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { broker, installMcpConfigs } from "@/bridge/client";
import { EmptyState } from "@/components/atoms/ListState";
import { absoluteTime, relativeTime } from "@/ui/time";
import { SearchField } from "@/components/SearchField";
import { ProviderLogo } from "@/components/atoms/ProviderLogo";
import { Switch } from "@/components/atoms/Switch";
import { SyntaxCode } from "@/components/SyntaxCode";
import {
  BackArrowIcon,
  BellIcon,
  BookmarkIcon,
  ChevronIcon,
  CloseIcon,
  HistoryIcon,
  InfoIcon,
  KeyboardIcon,
  LinkIcon,
  ListIcon,
  TerminalIcon,
} from "@/ui/icons";
import { Card, CardRow, PageHeader, Section } from "@/components/primitives/Page";
import { MarkdownContent } from "@/domain/markdown";
import { useTaskNotifications } from "@/state/notification-preferences";
import { toast } from "@/state/toast";
import { workerToastName } from "@/lib/toast-subject";
import type { AppUpdateStatus } from "@/shell/useAppUpdates";
import type {
  AdvisorSettings,
  BridgeResult,
  CleanupSettings,
  CleanupSnapshot,
  LoveRule,
  McpInstallResult,
  MemoryEntry,
  ModelSettingsSnapshot,
  ProfileView,
  PromptConfig,
  PromptWrite,
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
import type { ProjectSettingsScope, PromptsModel, SettingsState, SettingsTab } from "./state";
import { SETTINGS_GROUPS, SETTINGS_TABS, tabLabel } from "./state";

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
    case "fx":
      return "fx";
    case "cursor":
      return "auto-smart[optimize_for=balanced]";
  }
}

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

  const visibleTabs = useMemo(() => {
    const needle = sectionQuery.trim().toLowerCase();
    return SETTINGS_TABS.filter((tab) => !needle || tabLabel(tab).toLowerCase().includes(needle));
  }, [sectionQuery]);

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

  const loadCallerPromptScope = useCallback(
    async (scope: ProjectSettingsScope, projects: SettingsState["projects"]) => {
      const cwd = scopeCwd(scope, projects);
      setState((s) => ({ ...s, callerPromptScope: scope, callerPrompts: beginPromptLoad(s.callerPrompts) }));
      const result = await broker.callerPrompt(cwd);
      if (result.ok) {
        setState((s) => ({ ...s, callerPrompts: applyPromptConfig(s.callerPrompts, result.value) }));
      } else {
        setState((s) => ({ ...s, callerPrompts: applyPromptLoadError(s.callerPrompts, result.error.message) }));
      }
    },
    [],
  );

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
    if (
      !state.callerPrompts.loaded &&
      !state.callerPrompts.loadError &&
      state.callerPrompts.text === "" &&
      state.callerPrompts.inherited === ""
    ) {
      void loadCallerPromptScope({ kind: "global" }, state.projects);
    }
    // This effect must run only when the overview becomes ready.
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
          {visibleTabs.length === 0 ? (
            <p className="settings-rail-empty">No sections match &ldquo;{sectionQuery}&rdquo;.</p>
          ) : (
            SETTINGS_GROUPS.map((group) => {
              const tabs = group.tabs.filter((tab) => visibleTabs.includes(tab));
              if (tabs.length === 0) return null;
              return (
                <div key={group.label} className="settings-nav-group" role="presentation">
                  <p className="settings-nav-label" aria-hidden="true">{group.label}</p>
                  {tabs.map((tab) => (
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
                      <TabIcon tab={tab} />
                      <span>{tabLabel(tab)}</span>
                    </button>
                  ))}
                </div>
              );
            })
          )}
        </nav>
      </aside>

      <div className="settings-content">
        <div className="page-column">
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
            className={activeTab !== "workers" ? "settings-tab-panel-hidden" : undefined}
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
            id="settings-panel-notifications"
            role="tabpanel"
            tabIndex={0}
            aria-labelledby="settings-tab-notifications"
            hidden={activeTab !== "notifications"}
            className={activeTab !== "notifications" ? "settings-tab-panel-hidden" : undefined}
          >
            <NotificationsPanel />
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
            id="settings-panel-callerPrompts"
            role="tabpanel"
            tabIndex={0}
            aria-labelledby="settings-tab-callerPrompts"
            hidden={activeTab !== "callerPrompts"}
            className={activeTab !== "callerPrompts" ? "settings-tab-panel-hidden" : undefined}
          >
            <PromptsPanel
              state={state}
              setState={setState}
              loadPromptScope={loadCallerPromptScope}
              offline={offline}
              surface={CALLER_PROMPT_SURFACE}
            />
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
            id="settings-panel-shortcuts"
            role="tabpanel"
            tabIndex={0}
            aria-labelledby="settings-tab-shortcuts"
            hidden={activeTab !== "shortcuts"}
            className={activeTab !== "shortcuts" ? "settings-tab-panel-hidden" : undefined}
          >
            <ShortcutsPanel />
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
    </div>
  );
}

function TabIcon({ tab }: { tab: SettingsTab }) {
  switch (tab) {
    case "workers":
      return <TerminalIcon />;
    case "connections":
      return <LinkIcon />;
    case "notifications":
      return <BellIcon />;
    case "memories":
      return <BookmarkIcon />;
    case "callerPrompts":
      return <ListIcon />;
    case "storage":
      return <HistoryIcon />;
    case "shortcuts":
      return <KeyboardIcon />;
    case "about":
      return <InfoIcon />;
  }
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
          {SETTINGS_GROUPS.map((group) => (
            <div key={group.label} className="settings-nav-group">
              <p className="settings-nav-label">{group.label}</p>
              {group.tabs.map((tab) => (
                <span key={tab} className="settings-skeleton-block settings-skeleton-tab" />
              ))}
            </div>
          ))}
        </nav>
      </aside>
      <div className="settings-content">
        <div className="page-column">
          <div className="page-header">
            <span className="settings-skeleton-block settings-skeleton-title" />
          </div>
          <Card>
            {[0, 1, 2, 3].map((row) => (
              <div key={row} className="card-row">
                <span className="settings-skeleton-block settings-skeleton-row" />
              </div>
            ))}
          </Card>
        </div>
      </div>
    </div>
  );
}

type WorkersView = { kind: "list" } | { kind: "add" } | { kind: "worker"; id: string };

function WaitingPanel() {
  const [settings, setSettings] = useState<WaitSettings | undefined>(undefined);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);
  const [minutesDraft, setMinutesDraft] = useState("");
  const [minutesError, setMinutesError] = useState<string | undefined>(undefined);

  const loadWaiting = useCallback(async () => {
    const result = await broker.waiting();
    if (result.ok) {
      setSettings(result.value);
      setError(undefined);
    } else {
      setError(result.error.message);
    }
  }, []);

  useEffect(() => {
    void loadWaiting();
  }, [loadWaiting]);

  useEffect(() => {
    if (settings) setMinutesDraft(String(settings.networkMaxWaitMinutes));
  }, [settings?.networkMaxWaitMinutes]);

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

  const commitMinutes = () => {
    if (!settings) return;
    const value = Number(minutesDraft);
    if (!Number.isInteger(value) || value < 1 || value > 1440) {
      setMinutesError("Enter a number from 1 to 1440.");
      return;
    }
    setMinutesError(undefined);
    if (value !== settings.networkMaxWaitMinutes) void save({ ...settings, networkMaxWaitMinutes: value });
  };

  return (
    <Section
      title="Interruptions"
      description="Tasks that lose the connection, or run out of usage, wait and pick up on their own."
    >
      {error ? (
        <div className="settings-message settings-message-error" role="alert">
          <strong>Couldn&apos;t load these choices</strong>
          <p>{error}</p>
          <button className="text-button" type="button" onClick={() => void loadWaiting()}>
            Try again
          </button>
        </div>
      ) : null}
      {settings ? (
        <Card>
          <CardRow
            as="label"
            title="Move the task to another worker"
            description="When usage runs out, hand the task to another worker you turned on that is at least as strong."
          >
            <input
              type="checkbox"
              checked={settings.moveOnRateLimit}
              disabled={saving}
              onChange={(event) => void save({ ...settings, moveOnRateLimit: event.target.checked })}
            />
          </CardRow>
          <CardRow
            as="label"
            title="Wait for the connection up to"
            description={
              <>
                After this, the task stops and tells you the connection never came back.
                {minutesError ? (
                  <span className="settings-form-error" role="alert">
                    {minutesError}
                  </span>
                ) : null}
              </>
            }
          >
            <span className="settings-number-field">
              <input
                type="number"
                min={1}
                max={1440}
                value={minutesDraft}
                disabled={saving}
                aria-label="Minutes to wait for the connection"
                onChange={(event) => setMinutesDraft(event.target.value)}
                onBlur={commitMinutes}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    commitMinutes();
                  }
                }}
              />
              <span>minutes</span>
            </span>
          </CardRow>
        </Card>
      ) : null}
    </Section>
  );
}

function AdvisorPanel() {
  const [settings, setSettings] = useState<AdvisorSettings | undefined>(undefined);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | undefined>(undefined);
  const [keyDraft, setKeyDraft] = useState("");

  const loadAdvisor = useCallback(async () => {
    const result = await broker.advisor();
    if (result.ok) {
      setSettings(result.value);
      setError(undefined);
    } else {
      setError(result.error.message);
    }
  }, []);

  useEffect(() => {
    void loadAdvisor();
  }, [loadAdvisor]);

  useEffect(() => {
    if (settings) setKeyDraft(settings.apiKey);
  }, [settings?.apiKey]);

  const save = async (next: AdvisorSettings) => {
    const previous = settings;
    setSettings(next);
    setSaving(true);
    const result = await broker.putAdvisor(next);
    setSaving(false);
    if (result.ok) {
      setSettings(result.value);
      setError(undefined);
      return;
    }
    setSettings(previous);
    setError(result.error.message);
  };

  const commitKey = () => {
    if (!settings || keyDraft === settings.apiKey) return;
    void save({ ...settings, apiKey: keyDraft.trim() });
  };

  const hasKey = Boolean(settings?.apiKey);

  return (
    <Section
      title="Choosing a worker"
      description="Your rules decide today. Turn this on and Oga reads the brief first, so the work lands on the worker that suits it."
    >
      {error ? (
        <div className="settings-message settings-message-error" role="alert">
          <strong>Couldn&apos;t load these choices</strong>
          <p>{error}</p>
          <button className="text-button" type="button" onClick={() => void loadAdvisor()}>
            Try again
          </button>
        </div>
      ) : null}
      {settings ? (
        <Card>
          <CardRow as="label" title="TypeSafe key" description="From your TypeSafe account. It stays on this machine.">
            <input
              className="settings-mono settings-text-input"
              type="password"
              spellCheck={false}
              autoComplete="off"
              aria-label="TypeSafe key"
              value={keyDraft}
              disabled={saving}
              placeholder="Paste your key"
              onChange={(event) => setKeyDraft(event.target.value)}
              onBlur={commitKey}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  commitKey();
                }
              }}
            />
          </CardRow>
          <CardRow
            as="label"
            title="Match the worker to the brief"
            description={hasKey ? "Your rules still decide when no answer comes back." : "Add your key first."}
          >
            <input
              type="checkbox"
              checked={settings.enabled}
              disabled={saving || !hasKey}
              onChange={(event) => void save({ ...settings, enabled: event.target.checked })}
            />
          </CardRow>
        </Card>
      ) : null}
    </Section>
  );
}

function NotificationsPanel() {
  const [enabled, setEnabled] = useTaskNotifications();

  return (
    <>
      <PageHeader title={tabLabel("notifications")} />
      <Section
        title="When a task stops"
        description="Oga can tell you a task finished, stopped short, or is waiting on your answer, even with the window closed."
      >
        <Card>
          <CardRow as="label" title="Tell me when a task stops" description="Click the notification to open that task.">
            <input type="checkbox" checked={enabled} onChange={(event) => setEnabled(event.target.checked)} />
          </CardRow>
        </Card>
      </Section>
    </>
  );
}

interface ShortcutRow {
  keys: string[][];
  action: string;
}

interface ShortcutGroup {
  heading: string;
  rows: ShortcutRow[];
}

// Every row below is bound somewhere in the app — app-wide menu
// accelerators (rust/apps/oga-desktop/src/commands.rs, wired through
// web/src/shell/menuCommands.ts), the web fallback for the same actions
// (web/src/shell/useKeyboardShortcuts.ts), or a component's own key
// handler. Nothing here is aspirational.
const SHORTCUT_GROUPS: ShortcutGroup[] = [
  {
    heading: "Finding your way",
    rows: [
      { keys: [["⌘", "K"]], action: "Find a task" },
      { keys: [["⌘", "["]], action: "Go back" },
      { keys: [["⌘", "]"]], action: "Go forward" },
      { keys: [["⌘", ","]], action: "Open Settings" },
      { keys: [["⌘", "⇧", "U"]], action: "Open usage" },
    ],
  },
  {
    heading: "The task screen",
    rows: [
      { keys: [["⌘", "B"]], action: "Show or hide the task list" },
      { keys: [["⌘", "\\"]], action: "Show or hide the changed-files panel" },
      { keys: [["⌘", "R"]], action: "Refresh the task list" },
      { keys: [["⌘", "1"], ["⌘", "2"], ["⌘", "3"]], action: "Jump to the activity, the request, or the response" },
    ],
  },
  {
    heading: "Text size",
    rows: [
      { keys: [["⌘", "="], ["⌘", "⇧", "="]], action: "Zoom in" },
      { keys: [["⌘", "-"]], action: "Zoom out" },
      { keys: [["⌘", "0"]], action: "Back to actual size" },
    ],
  },
  {
    heading: "Writing to a worker",
    rows: [
      { keys: [["⌘", "↵"]], action: "Send the message" },
      { keys: [["⎋"]], action: "Clear what you typed" },
    ],
  },
  {
    heading: "Reading a task",
    rows: [
      { keys: [["↑"], ["↓"]], action: "Move through the activity" },
      { keys: [["Home"], ["End"]], action: "Jump to the first or last row" },
      { keys: [["PgUp"], ["PgDn"]], action: "Move a screenful at a time" },
      { keys: [["←"], ["→"]], action: "Resize the changed-files panel" },
      { keys: [["↵"]], action: "Reset the panel width" },
    ],
  },
  {
    heading: "The task list",
    rows: [
      { keys: [["↑"], ["↓"]], action: "Move between tasks" },
      { keys: [["Home"], ["End"]], action: "Jump to the first or last task" },
      { keys: [["↓"]], action: "Jump from the search field into the list" },
    ],
  },
  {
    heading: "Dialogs, menus, and search",
    rows: [
      { keys: [["⎋"]], action: "Close the dialog" },
      { keys: [["Tab"], ["⇧", "Tab"]], action: "Move through the dialog" },
      { keys: [["↑"], ["↓"]], action: "Move through a menu" },
      { keys: [["↵"]], action: "Choose what's highlighted" },
      { keys: [["⎋"]], action: "Close menus and popovers" },
      { keys: [["⎋"]], action: "Clear the search — press again to leave the field" },
      { keys: [["⎋"]], action: "Step back out of the worker editor" },
    ],
  },
];

function ShortcutsPanel() {
  return (
    <>
      <PageHeader title={tabLabel("shortcuts")} description="What you can press, grouped by what you are doing." />
      <div className="page-sections">
        {SHORTCUT_GROUPS.map((group) => (
          <Section key={group.heading} title={group.heading}>
            <ul className="card">
              {group.rows.map((row) => (
                <li key={row.action} className="card-row">
                  <span className="card-row-text">{row.action}</span>
                  <span className="card-row-control settings-shortcut-keys">
                    {row.keys.map((combo) => (
                      <span key={combo.join("+")} className="settings-shortcut-combo">
                        {combo.map((key) => (
                          <kbd key={key}>{key}</kbd>
                        ))}
                      </span>
                    ))}
                  </span>
                </li>
              ))}
            </ul>
          </Section>
        ))}
      </div>
    </>
  );
}

const WORK_LABELS = {
  context: "Reading and lookups",
  mechanical: "Small edits",
  build: "Building and fixing",
  reasoning: "Hard thinking",
  general: "Open-ended work",
  ui: "UI work",
  ux: "UX work",
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
  return [first, ...rest].join(", ");
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

const SUBJECT_KINDS = new Set<WorkKind>(["ui", "ux", "backend", "database", "docs", "tests", "review", "research", "refactor"]);

/** Rules in the order they win a task: subjects, then classes, then the rest. */
function byPrecedence(rules: LoveRule[]): LoveRule[] {
  const tier = (rule: LoveRule) =>
    rule.when.length === 0 ? 2 : rule.when.some((kind) => SUBJECT_KINDS.has(kind)) ? 0 : 1;
  return [...rules].sort((a, b) => tier(a) - tier(b));
}

/** Where work that names no model goes, kind of work by kind of work. */
function FavouriteModels({ rules }: { rules: LoveRule[] }) {
  if (rules.length === 0) return null;
  return (
    <Section
      title="Favourite models"
      description="A task that names no model goes to the first listed model for that kind of work that can take it."
    >
      <ul className="card">
        {byPrecedence(rules).map((rule, index) => (
          <li className="card-row" key={`${rule.model}-${index}`}>
            <span className="card-row-text">
              <span className="card-row-title">{workLabel(rule.when)}</span>
              <span className="card-row-description settings-favourite-model">{modelLabel(rule)}</span>
            </span>
            <span className="card-row-control settings-muted">{effortLabel(rule)}</span>
          </li>
        ))}
      </ul>
    </Section>
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
  const [view, setView] = useState<WorkersView>({ kind: "list" });
  const [deleteId, setDeleteId] = useState<string | undefined>(undefined);
  const [deleting, setDeleting] = useState(false);
  const [checkedAt, setCheckedAt] = useState<number | undefined>(undefined);

  useEffect(() => {
    if (state.overview === "ready") setCheckedAt(Date.now());
  }, [state.overview]);

  const showList = useCallback(() => {
    setDeleteId(undefined);
    setView({ kind: "list" });
  }, []);

  const selected = view.kind === "worker" ? state.profiles.find((profile) => profile.id === view.id) : undefined;

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
        setView({ kind: "list" });
        await onRefresh();
      } else {
        lifecycle.error(quoted ? `Couldn't remove worker ${quoted}` : "Couldn't remove worker", { description: "Try again.", detail: result.error.message });
      }
      setDeleting(false);
    },
    [deleting, onRefresh, setState, state.profiles],
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

  if (view.kind === "add") {
    return (
      <div className="settings-worker-page">
        <BackToWorkers onClick={showList} />
        <ProfileEditor
          profile={undefined}
          models={suggestedModels(state, undefined)}
          offline={offline}
          setState={setState}
          onClose={showList}
          onRefresh={onRefresh}
          onSaved={(profile) => setView({ kind: "worker", id: profile.id })}
        />
      </div>
    );
  }

  if (view.kind === "worker" && selected) {
    return (
      <div className="settings-worker-page">
        <BackToWorkers onClick={showList} />
        <ProfileEditor
          profile={selected}
          models={suggestedModels(state, selected.id)}
          offline={offline}
          setState={setState}
          onClose={showList}
          onRefresh={onRefresh}
          onDelete={() => setDeleteId(selected.id)}
        />
        <WorkerModelsSection
          profile={selected}
          state={state}
          setState={setState}
          loadModelScope={loadModelScope}
          offline={offline}
        />
        {deleteId === selected.id ? (
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
                onClick={() => handleDelete(selected.id)}
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

  return (
    <>
      <PageHeader
        title={tabLabel("workers")}
        description="Each worker keeps its own provider account, model, and environment."
        actions={
          <>
            {checkedAt !== undefined ? (
              <span className="settings-muted">Checked <span title={absoluteTime(checkedAt)}>{relativeTime(checkedAt)}</span></span>
            ) : null}
            <button className="settings-button" type="button" onClick={handleRefresh} disabled={refreshing}>
              Refresh
            </button>
            <button
              className="settings-button settings-button-primary"
              type="button"
              disabled={offline}
              onClick={() => setView({ kind: "add" })}
            >
              Add worker
            </button>
          </>
        }
      />
      <div className="page-sections">
        <Section title="Command-line workers">
          <Card>
            {state.overview === "loading" ? (
              <p className="settings-status">Loading workers…</p>
            ) : state.overview === "error" ? (
              <p className="settings-status">
                Couldn&apos;t load workers. <button className="text-button" type="button" onClick={() => void onRefresh()}>Try again</button>
              </p>
            ) : state.profiles.length === 0 ? (
              <EmptyState
                title="No workers yet"
                hint="Add a worker to choose where tasks run."
                className="settings-empty"
              />
            ) : (
              state.profiles.map((profile) => (
                <WorkerRow
                  key={profile.id}
                  profile={profile}
                  worker={state.modelSettings.snapshot?.workers.find((w) => w.id === profile.id)}
                  onOpen={() => setView({ kind: "worker", id: profile.id })}
                  onToggle={handleToggle}
                  offline={offline}
                />
              ))
            )}
          </Card>
        </Section>

        <FavouriteModels rules={state.modelSettings.snapshot?.love ?? []} />
        <AdvisorPanel />
        <WaitingPanel />
      </div>
    </>
  );
}

function BackToWorkers({ onClick }: { onClick: () => void }) {
  return (
    <button className="text-button settings-worker-back" type="button" onClick={onClick}>
      <BackArrowIcon size={14} />
      Workers
    </button>
  );
}

function WorkerRow({
  profile,
  worker,
  onOpen,
  onToggle,
  offline,
}: {
  profile: ProfileView;
  worker: ModelSettingsSnapshot["workers"][number] | undefined;
  onOpen: () => void;
  onToggle: (id: string, enabled: boolean) => void;
  offline: boolean;
}) {
  const available = Boolean(worker?.configured) && profile.enabled;
  const modelCount = worker?.models.length ?? 0;
  const binaryPath = profile.command?.join(" ");

  return (
    <div className="card-row settings-worker-row">
      <button className="settings-worker-row-main" type="button" onClick={onOpen}>
        <span className="settings-worker-row-mark">
          <ProviderLogo provider={profile.provider} size={22} />
          <span
            className={`settings-worker-row-dot${available ? " settings-worker-row-dot-on" : ""}`}
            aria-hidden="true"
          />
          <span className="visually-hidden">{available ? "Available" : "Unavailable"}</span>
        </span>
        <span className="card-row-text settings-worker-row-copy">
          <span className="card-row-title">{profile.label}</span>
          <span className="card-row-description">
            {binaryPath ? <span className="settings-mono">{binaryPath}</span> : null}
            {binaryPath ? " · " : ""}
            {modelCount} {modelCount === 1 ? "model" : "models"}
          </span>
        </span>
      </button>
      <span className="card-row-control">
        <Switch
          className="settings-worker-row-switch"
          checked={profile.enabled}
          accessibleName={`${profile.enabled ? "Disable" : "Enable"} ${profile.label} for tasks`}
          disabled={offline}
          onChange={(e) => onToggle(profile.id, e.target.checked)}
        />
        <ChevronIcon className="settings-worker-row-chevron" size={16} />
      </span>
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
    <Section
      className="settings-worker-models"
      title="Models"
      actions={
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
      }
    >
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
          <Card className="settings-model-list">
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
          </Card>
        </>
      )}
      {state.modelSettings.saveError ? (
        <p className="settings-form-error" role="alert">
          {state.modelSettings.saveError}
        </p>
      ) : null}
    </Section>
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
      <header className="page-header">
        <h2 className="page-title">{isNew ? "Add worker" : "Edit worker"}</h2>
      </header>
      <form
        className="settings-worker-form"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          void handleSave();
        }}
      >
        <section className="section settings-worker-form-section" aria-labelledby="worker-form-identity">
          <h3 className="section-title" id="worker-form-identity">Identity</h3>
          <div className="card">
            {isNew ? (
              <div className="card-row settings-worker-form-row">
                <div className="settings-worker-form-label">
                  <label htmlFor="worker-form-id">Worker ID</label>
                  <small id="worker-form-id-help">Lowercase letters, numbers, and dashes. Set once.</small>
                </div>
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
                  {idError ? (
                    <p className="settings-form-error" id="worker-form-id-error" role="alert">
                      {idError}
                    </p>
                  ) : null}
                </div>
              </div>
            ) : null}
            <div className="card-row settings-worker-form-row">
              <div className="settings-worker-form-label">
                <label htmlFor="worker-form-name">Display name</label>
                <small id="worker-form-name-help">Shown in the workers list.</small>
              </div>
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
                {labelError ? (
                  <p className="settings-form-error" id="worker-form-name-error" role="alert">
                    {labelError}
                  </p>
                ) : null}
              </div>
            </div>
            <div className="card-row settings-worker-form-row">
              <div className="settings-worker-form-label">
                <span id="worker-form-available-label">Available for tasks</span>
                <small>Turn off to pause new tasks on this worker.</small>
              </div>
              <div className="settings-worker-form-field">
                <label className="settings-toggle">
                  <input
                    type="checkbox"
                    checked={enabled}
                    disabled={disabled}
                    aria-labelledby="worker-form-available-label"
                    onChange={(e) => setEnabled(e.target.checked)}
                  />
                </label>
              </div>
            </div>
          </div>
        </section>

        <section className="section settings-worker-form-section" aria-labelledby="worker-form-runs">
          <h3 className="section-title" id="worker-form-runs">Where it runs</h3>
          <div className="card">
            <div className="card-row settings-worker-form-row">
              <div className="settings-worker-form-label">
                <label htmlFor="worker-form-tool">Command-line tool</label>
                <small id="worker-form-tool-help">The tool this worker signs in with.</small>
              </div>
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
              </div>
            </div>
            <div className="card-row settings-worker-form-row">
              <div className="settings-worker-form-label">
                <label htmlFor="worker-form-model">Default model</label>
                <small id="worker-form-model-help">Leave blank for the tool default. Type to search known models.</small>
              </div>
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
              </div>
            </div>
          </div>
        </section>

        <section className="section settings-worker-form-section" aria-labelledby="worker-form-env">
          <h3 className="section-title" id="worker-form-env">Environment variables</h3>
          <div className="card">
            <div className="card-row settings-worker-form-row">
              <div className="settings-worker-form-label">
                <span id="worker-form-env-label">Variables</span>
                <small>One row per variable. Values stay on this device.</small>
              </div>
              <div className="settings-worker-form-field">
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
              </div>
            </div>
            {envRows.length > 0 || shownEnvError ? (
              <div className="card-row settings-worker-form-row settings-worker-form-row-stack">
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
                        <CloseIcon size={14} />
                      </button>
                    </div>
                  ))}
                  {shownEnvError ? (
                    <p className="settings-form-error" role="alert">
                      {shownEnvError}
                    </p>
                  ) : null}
                </div>
              </div>
            ) : null}
          </div>
        </section>

        <section className="section settings-worker-form-section" aria-labelledby="worker-form-tags">
          <h3 className="section-title" id="worker-form-tags">Tags</h3>
          <div className="card">
            <div className="card-row settings-worker-form-row">
              <div className="settings-worker-form-label">
                <label htmlFor="worker-form-tags-input">Tags</label>
                <small id="worker-form-tags-help">Tags help Oga pick this worker for matching work. Separate with commas.</small>
              </div>
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
    setState((s) => ({ ...s, integrations: { loading: true, results: [] } }));
    const lifecycle = toast.pending("Connecting tools");
    const result = await installMcpConfigs(state.profiles);
    if (result.ok) {
      const failed = result.value.filter((item) => !item.success);
      if (failed.length === 0) lifecycle.dismiss();
      else lifecycle.error("Some tools could not connect", { detail: failed.map((item) => `${item.client}: ${item.message}`).join("\n") });
      setState((s) => ({ ...s, integrations: { loading: false, results: result.value } }));
    } else {
      lifecycle.error("Couldn't connect tools", { description: "Check the client settings and try again.", detail: result.error.message });
      setState((s) => ({ ...s, integrations: { loading: false, results: [] } }));
    }
  }, [state.integrations.loading, state.profiles, setState]);

  return (
    <>
      <PageHeader title={tabLabel("connections")} />
      <Section
        title="Connect command-line tools"
        description="Add Oga to the supported client config files. Existing files are backed up before they change."
        actions={
          <button
            className="settings-button settings-button-primary"
            type="button"
            onClick={handleInstall}
            disabled={state.integrations.loading || offline}
          >
            {state.integrations.loading ? "Connecting…" : "Connect tools"}
          </button>
        }
      >
        {state.integrations.results.length ? (
          <div className="card" aria-live="polite">
            {state.integrations.results.map((result: McpInstallResult) => (
              <InstallResultRow key={result.path} result={result} />
            ))}
          </div>
        ) : null}
      </Section>
    </>
  );
}

function InstallResultRow({ result }: { result: McpInstallResult }) {
  return (
    <CardRow
      className={`settings-install-result${result.success ? "" : " settings-install-result-error"}`}
      leading={
        <>
          <span className="settings-install-mark" aria-hidden="true">
            {result.success ? "OK" : "!"}
          </span>
          <span className="visually-hidden">{result.success ? "Connected" : "Failed"}</span>
        </>
      }
      title={result.client}
      description={`${result.message} · ${result.path}`}
    />
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
    <>
      <PageHeader title={tabLabel("memories")} />
      <Section
        title="Project memories"
        description="These notes are shared with workers in the project. Values stay in Oga until you open a project."
        actions={<SearchField className="settings-search" value={query} onChange={setQuery} placeholder="Search memories" />}
      >
        <div className="settings-memory-layout">
          <nav className="card settings-memory-projects" aria-label="Projects with memories">
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
                  className={`card-row settings-memory-project${state.memories.selectedProject === project.cwd ? " settings-memory-project-active" : ""}`}
                  type="button"
                  aria-pressed={state.memories.selectedProject === project.cwd}
                  onClick={() => void handleSelect(project.cwd)}
                >
                  <span className="card-row-text">
                    <span className="card-row-title">{projectName(project.cwd)}</span>
                    <span className="card-row-description">
                      {project.count} {project.count === 1 ? "memory" : "memories"}
                    </span>
                  </span>
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
                title="No memories match"
                hint="Try a different filter."
                className="settings-placeholder"
                action={<button className="text-button" type="button" onClick={() => setQuery("")}>Clear filter</button>}
              />
            ) : (
              <div className="settings-memory-list">
                {filtered.map((entry: MemoryEntry) => (
                  <article key={entry.key} className="card settings-memory-card">
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
      </Section>
    </>
  );
}


/** The two texts the Prompts tabs edit, told apart by what each one reads and writes. */
interface PromptSurface {
  tab: SettingsTab;
  label: string;
  /** Shown over the editor when the label differs from the tab's. */
  heading?: string;
  helper: React.ReactNode;
  model: (state: SettingsState) => PromptsModel;
  scope: (state: SettingsState) => ProjectSettingsScope;
  apply: (state: SettingsState, model: PromptsModel) => SettingsState;
  save: (request: PromptWrite) => Promise<BridgeResult<PromptConfig>>;
}

const CALLER_PROMPT_SURFACE: PromptSurface = {
  tab: "callerPrompts",
  label: "How briefs are written",
  heading: "How briefs are written",
  helper: (
    <>
      What your agent reads before it writes up work to hand off. Use {"{{default}}"} to keep Oga&apos;s own wording,
      and {"{{project}}"} for the folder path.
    </>
  ),
  model: (state) => state.callerPrompts,
  scope: (state) => state.callerPromptScope,
  apply: (state, callerPrompts) => ({ ...state, callerPrompts }),
  save: (request) => broker.putCallerPrompt(request),
};

function PromptsPanel({
  state,
  setState,
  loadPromptScope,
  offline,
  surface,
}: {
  state: SettingsState;
  setState: React.Dispatch<React.SetStateAction<SettingsState>>;
  loadPromptScope: (scope: ProjectSettingsScope, projects: SettingsState["projects"]) => Promise<void>;
  offline: boolean;
  surface: PromptSurface;
}) {
  const prompts = surface.model(state);
  const handleScopeChange = useCallback(
    (raw: string) => {
      const scope = scopeFromKey(raw);
      void loadPromptScope(scope, state.projects);
    },
    [loadPromptScope, state.projects],
  );

  const handleReload = useCallback(() => {
    void loadPromptScope(surface.scope(state), state.projects);
  }, [loadPromptScope, surface, state]);

  const handleSave = useCallback(async () => {
    const dirty = isPromptDirty(prompts);
    if (!dirty || prompts.saving) return;
    setState((s) => surface.apply(s, setPromptSaving(surface.model(s), true)));
    const payload = promptPayload(prompts);
    const lifecycle = toast.pending("Saving instructions");
    const result = await surface.save(payload);
    if (result.ok) {
      lifecycle.dismiss();
      setState((s) =>
        surface.apply(s, finishPromptSave(surface.model(s), { ok: true, snapshot: result.value })),
      );
    } else {
      const err =
        result.error.status !== undefined
          ? { kind: "message" as const, message: result.error.message }
          : { kind: "unreachable" as const };
      lifecycle.error("Couldn't save instructions", {
        description: err.kind === "unreachable" ? "Check that Oga is running, then try again." : "Try again.",
        detail: err.kind === "message" ? err.message : undefined,
      });
      setState((s) => surface.apply(s, finishPromptSave(surface.model(s), { ok: false, error: err })));
    }
  }, [prompts, setState, surface]);

  const handleReset = useCallback(() => {
    setState((s) => surface.apply(s, resetPrompt(surface.model(s))));
  }, [setState, surface]);

  const handleTextChange = useCallback(
    (text: string) => {
      setState((s) => surface.apply(s, updatePromptText(surface.model(s), text)));
    },
    [setState, surface],
  );

  return (
    <>
      <PageHeader
        title={tabLabel(surface.tab)}
        actions={
          <label className="settings-scope-picker">
            <span>Applies to</span>
            <select value={scopeKey(surface.scope(state))} onChange={(e) => handleScopeChange(e.target.value)}>
              <option value="global">All projects</option>
              {state.projects?.projects.map((path) => (
                <option key={path} value={path}>
                  {projectName(path)}
                </option>
              ))}
            </select>
          </label>
        }
      />
      <Section title={surface.heading}>
        {prompts.loadError ? (
          <div className="settings-message settings-message-error" role="alert">
            <strong>Couldn&apos;t read these instructions</strong>
            <p>{prompts.loadError}</p>
            <button className="settings-button" type="button" onClick={handleReload}>
              Try again
            </button>
          </div>
        ) : !prompts.loaded ? (
          <p className="settings-status">Loading instructions…</p>
        ) : (
          <div className="settings-prompt-editor">
            <div className="settings-prompt-meta">
              <span className="settings-muted">
                {prompts.configPath ? "Set by the project file" : prompts.written ? "Set for this project" : "Using the default for all projects"}
              </span>
              {prompts.configPath === undefined && prompts.written ? (
                <button className="text-button" type="button" onClick={handleReset}>
                  Use inherited
                </button>
              ) : null}
            </div>
            <textarea
              value={prompts.text}
              onChange={(e) => handleTextChange(e.target.value)}
              readOnly={prompts.configPath !== undefined}
              aria-label={surface.label}
            />
            <p className="settings-helper">{surface.helper}</p>
            {prompts.configPath !== undefined ? (
              <p className="settings-helper">Set by the project file. Edit that file to change them.</p>
            ) : (
              <div className="settings-form-footer">
                <button
                  className="settings-button settings-button-primary"
                  type="button"
                  onClick={handleSave}
                  disabled={prompts.saving || !isPromptDirty(prompts) || offline}
                >
                  {prompts.saving ? "Saving…" : "Save changes"}
                </button>
                {isPromptDirty(prompts) ? (
                  <span className="settings-muted">Unsaved changes</span>
                ) : prompts.saved ? (
                  <span className="settings-muted">Saved for new tasks</span>
                ) : null}
              </div>
            )}
          </div>
        )}
      </Section>
    </>
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
  const removeDisabledReason = hasUnsavedChanges
    ? "Save your changes first."
    : !snapshot?.plan.events
      ? "Nothing to remove yet."
      : undefined;

  useEffect(() => {
    if (snapshot) setDraft(snapshot.settings);
  }, [snapshot]);

  const updateDraft = (patch: Partial<CleanupSettings>) => {
    if (settings) setDraft({ ...settings, ...patch });
  };

  const save = async () => {
    if (!settings || state.cleanup.saving) return;
    setState((s) => ({ ...s, cleanup: { ...s.cleanup, saving: true, error: undefined } }));
    const lifecycle = toast.pending("Saving task history settings");
    const result = await broker.putCleanup(settings);
    if (result.ok) {
      lifecycle.success("Task history settings saved");
      setState((s) => ({ ...s, cleanup: { ...s.cleanup, saving: false, snapshot: result.value } }));
      setDraft(result.value.settings);
    } else {
      lifecycle.error("Couldn't save task history settings", { description: "Try again.", detail: result.error.message });
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
    <>
      <PageHeader
        title={tabLabel("storage")}
        description="Oga keeps each task's request and answer. These choices remove only its detailed activity."
        actions={
          <button className="settings-button" type="button" onClick={() => void reload()} disabled={state.cleanup.loading}>
            Refresh
          </button>
        }
      />
      {state.cleanup.loading && !settings ? <p className="settings-status">Checking stored activity…</p> : null}
      {settings ? (
        <div className="page-sections">
          <Section title="What Oga keeps">
            <Card>
              <CardRow as="label" title="Remove old logs automatically" description="Remove logs after they reach the age you choose.">
                <input type="checkbox" checked={settings.enabled} onChange={(event) => updateDraft({ enabled: event.target.checked })} />
              </CardRow>
              <CardRow as="label" title="Keep logs for" description="Logs older than this can be removed.">
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
              </CardRow>
              <CardRow as="label" title="Remove logs only for archived tasks" description="Logs for active and unarchived tasks stay.">
                <input type="checkbox" checked={settings.archivedOnly} onChange={(event) => updateDraft({ archivedOnly: event.target.checked })} />
              </CardRow>
            </Card>
            <div className="settings-form-footer">
              <button
                className="settings-button settings-button-primary"
                type="button"
                onClick={() => void save()}
                disabled={state.cleanup.saving || offline || !hasUnsavedChanges}
              >
                {state.cleanup.saving ? "Saving…" : "Save"}
              </button>
              {hasUnsavedChanges ? <span className="settings-muted">Unsaved changes</span> : null}
            </div>
          </Section>
          {snapshot ? (
            <Section title={snapshot.plan.events ? "Logs ready to remove" : "Nothing to remove"}>
              <CleanupPreview plan={snapshot.plan} />
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
                  <button className="settings-button settings-button-danger" type="button" onClick={() => setConfirming(true)} disabled={state.cleanup.running || hasUnsavedChanges || !snapshot.plan.events || offline}>
                    {state.cleanup.running ? "Removing…" : "Review and remove now"}
                  </button>
                )}
                {!confirming && removeDisabledReason ? <p className="settings-muted">{removeDisabledReason}</p> : null}
              </div>
              {state.cleanup.result ? <p className="settings-success" role="status">Removed {state.cleanup.result.plan.events.toLocaleString()} logs and reclaimed {formatBytes(state.cleanup.result.fileBytesBefore - state.cleanup.result.fileBytesAfter)}.</p> : null}
            </Section>
          ) : null}
        </div>
      ) : null}
    </>
  );
}

function CleanupPreview({ plan }: { plan: CleanupSnapshot["plan"] }) {
  return (
    <div aria-live="polite">
      <dl className="card">
        <div className="card-row"><dt>Tasks</dt><dd>{plan.tasks.toLocaleString()}</dd></div>
        <div className="card-row"><dt>Logs</dt><dd>{plan.events.toLocaleString()}</dd></div>
        <div className="card-row"><dt>Space to free</dt><dd>{formatBytes(plan.bytes)}</dd></div>
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
      toast.error("Couldn't check Oga", { description: "Check that Oga is running and try again.", detail: result.error.message });
    }
  }, [setState]);

  return (
    <>
      <PageHeader title={tabLabel("about")} />
      <div className="page-sections">
        <Section title="Oga" description="Desktop client">
          {state.overview === "loading" ? (
            <p className="settings-status">Checking Oga…</p>
          ) : state.health ? (
            <>
              <dl className="card">
                <div className="card-row">
                  <dt className="settings-health-heading">
                    <span className="settings-health-dot" />
                    <strong>Oga is running</strong>
                  </dt>
                </div>
                <div className="card-row">
                  <dt>Version</dt>
                  <dd>{state.health.version}</dd>
                </div>
                <div className="card-row">
                  <dt>Connection version</dt>
                  <dd>v{state.health.mcpContractVersion}</dd>
                </div>
                <div className="card-row">
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
              <p>Check that Oga is running, then try again.</p>
              <button className="settings-button" type="button" onClick={handleRefresh}>
                Try again
              </button>
            </div>
          )}
        </Section>
        <Section title="Keep Oga current">
          <div className="card settings-update-card" aria-live="polite">
            {updateStatus.kind === "available" ? (
              <>
                <CardRow title={`Version ${updateStatus.version} is ready to install.`} description="Oga restarts when the update finishes.">
                  <button className="settings-button settings-button-primary" type="button" onClick={onInstallUpdate}>
                    Install and restart Oga
                  </button>
                </CardRow>
                {updateStatus.notes ? (
                  <div className="card-row">
                    <MarkdownContent source={updateStatus.notes} />
                  </div>
                ) : null}
              </>
            ) : updateStatus.kind === "checking" ? (
              <CardRow title="Checking for a new version…" />
            ) : updateStatus.kind === "downloading" ? (
              <CardRow
                title={`Downloading version ${updateStatus.version}${updateStatus.progress === undefined ? "…" : ` (${updateStatus.progress}%)`}`}
              />
            ) : updateStatus.kind === "installing" ? (
              <CardRow title={`Installing version ${updateStatus.version}…`} />
            ) : updateStatus.kind === "up-to-date" ? (
              <CardRow title="You have the latest version.">
                <button className="settings-button" type="button" onClick={onCheckForUpdates}>
                  Check for updates
                </button>
              </CardRow>
            ) : updateStatus.kind === "failed" ? (
              <CardRow
                className="settings-update-failed"
                title={updateStatus.reason === "check" ? "We couldn't check for a new version. Try again in a moment." : "We couldn't install the update. Try again."}
              >
                <button className="settings-button" type="button" onClick={updateStatus.reason === "check" ? onCheckForUpdates : onInstallUpdate}>
                  Try again
                </button>
              </CardRow>
            ) : updateStatus.kind === "unavailable" ? (
              <CardRow title="Update Oga from the desktop app." />
            ) : (
              <div className="card-row">
                <button className="settings-button" type="button" onClick={onCheckForUpdates}>
                  Check for updates
                </button>
              </div>
            )}
          </div>
        </Section>
      </div>
    </>
  );
}
