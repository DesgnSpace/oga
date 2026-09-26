import { afterEach, describe, expect, it } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { setTransport, type Transport } from "@/bridge/transport";
import { FONT_OPTIONS } from "@/appearance";
import type {
  AppearanceSettings,
  BrokerCall,
  CleanupResult,
  CleanupSettings,
  CleanupSnapshot,
  ModelSettingsSnapshot,
  ProfileView,
  PromptConfig,
} from "@/bridge/types";
import { clearCachedSettingsState } from "./state";
import { SettingsPage } from "./index";

const defaultBriefRules = "The brief is all the worker gets: it can't see this conversation and guesses badly. Write it as a message to a teammate. Cover what done looks like and why it matters, what you already know, the choices you've made, what must not change, how to check the work, and what to send back, including what it couldn't verify. One deliverable per task. Don't read files just to write the brief; if you'd need to, the task is too vague or too big. Use headings only when the work has several parts.";
const inheritedPrompt: PromptConfig = { cwd: "/tmp/project", scope: "global", written: false, value: defaultBriefRules, inherited: defaultBriefRules };
let callerPrompt = inheritedPrompt;
let savedCallerPrompt: { cwd: string; written: boolean; value: string } | undefined;
let appearance: AppearanceSettings = { font: null };
let savedAppearance: AppearanceSettings | undefined;

afterEach(() => {
  cleanup();
  clearCachedSettingsState();
  callerPrompt = inheritedPrompt;
  savedCallerPrompt = undefined;
  appearance = { font: null };
  savedAppearance = undefined;
  document.documentElement.removeAttribute("style");
});

const longModelId =
  "opencode-go/deepseek-v4-flash-instruct-preview-extended-context-window-edition";

const profiles = [
  {
    id: "claude-work",
    label: "Claude work",
    provider: "claude" as const,
    model: "sonnet",
    enabled: true,
    env: {},
    capabilities: [],
  },
  {
    id: "opencode-work",
    label: "OpenCode work",
    provider: "opencode" as const,
    model: longModelId,
    enabled: false,
    env: {},
    capabilities: [],
  },
];

function snapshot(): ModelSettingsSnapshot {
  return {
    cwd: "/tmp/project",
    scope: "global",
    revision: "rev1",
    love: [
      { model: "openai/gpt-5.6-luna", profileId: "opencode-work", when: ["context"], effort: "low", scope: "project" },
      { model: "opus", profileId: "claude-work", when: [], scope: "project" },
    ],
    workers: [
      {
        id: "claude-work",
        label: "Claude work",
        provider: "claude",
        enabled: true,
        inheritedEnabled: true,
        hasEnabledOverride: false,
        availableGlobally: true,
        configured: true,
        models: [
          {
            id: "opus",
            label: "Opus",
            enabled: false,
            inheritedEnabled: true,
            hasEnabledOverride: true,
            preferred: true,
            inheritedPreferred: false,
            hasPreferredOverride: true,
            capabilities: ["build"],
            inheritedCapabilities: ["build"],
            hasCapabilitiesOverride: false,
            availableGlobally: true,
          },
        ],
      },
      {
        id: "opencode-work",
        label: "OpenCode work",
        provider: "opencode",
        enabled: false,
        inheritedEnabled: true,
        hasEnabledOverride: false,
        availableGlobally: true,
        configured: true,
        models: [
          {
            id: longModelId,
            label: longModelId,
            enabled: true,
            inheritedEnabled: true,
            hasEnabledOverride: false,
            preferred: false,
            inheritedPreferred: false,
            hasPreferredOverride: false,
            capabilities: [],
            inheritedCapabilities: [],
            hasCapabilitiesOverride: false,
            availableGlobally: true,
          },
          {
            id: "openai/gpt-5.6-luna",
            label: "openai/gpt-5.6-luna",
            enabled: false,
            inheritedEnabled: false,
            hasEnabledOverride: false,
            preferred: false,
            inheritedPreferred: false,
            hasPreferredOverride: false,
            capabilities: [],
            inheritedCapabilities: [],
            hasCapabilitiesOverride: false,
            availableGlobally: true,
          },
        ],
      },
    ],
  };
}

function cleanupSnapshot(settings: CleanupSettings = { enabled: false, olderThanDays: 30, archivedOnly: true }): CleanupSnapshot {
  return {
    settings,
    plan: {
      cutoff: "2026-01-01T00:00:00Z",
      tasks: 2,
      byState: [{ state: "completed", tasks: 2 }],
      events: 12,
      bytes: 12_000,
      heldBack: 1,
    },
  };
}

function makeTransport(
  onPutModelSettings?: (request: unknown) => ModelSettingsSnapshot,
  onUpdateProfile?: (request: unknown) => ProfileView,
  onModelSettings?: (refresh: boolean) => ModelSettingsSnapshot | Promise<ModelSettingsSnapshot>,
  onCleanup?: () => CleanupSnapshot,
  onPutCleanup?: (settings: CleanupSettings) => CleanupSnapshot,
  onRunCleanup?: () => CleanupResult,
): Transport {
  return {
    async invoke<T>(command: string, args?: Record<string, unknown>) {
      if (command !== "broker_call") throw new Error(`Unexpected command: ${command}`);
      // SAFETY: the broker client always sends a `BrokerCall` under `call`.
      const call = args?.call as BrokerCall;
      switch (call.call) {
        case "summary":
          return { profiles, tasks: [], memoryProjects: [] } as T;
        case "projects":
          return { global: "/tmp/project", projects: [] } as T;
        case "health":
          return { version: "test", mcpContractVersion: 1, build: "test", stale: false } as T;
        case "modelSettings":
          return (onModelSettings ? onModelSettings(call.refresh === true) : snapshot()) as T;
        case "putModelSettings":
          return (onPutModelSettings ? onPutModelSettings(call.request) : snapshot()) as T;
        case "updateProfile":
          return (
            onUpdateProfile
              ? onUpdateProfile({ profileId: call.profileId, enabled: call.patch?.enabled })
              : profiles[0]
          ) as T;
        case "callerPrompt":
          return callerPrompt as T;
        case "putCallerPrompt":
          savedCallerPrompt = call.request;
          return { ...callerPrompt, ...savedCallerPrompt } as T;
        case "cleanup":
          return (onCleanup ? onCleanup() : cleanupSnapshot()) as T;
        case "putCleanup":
          return (onPutCleanup ? onPutCleanup(call.settings) : cleanupSnapshot(call.settings)) as T;
        case "appearance":
          return appearance as T;
        case "putAppearance":
          savedAppearance = call.settings;
          appearance = call.settings;
          return appearance as T;
        case "runCleanup":
          return (
            onRunCleanup
              ? onRunCleanup()
              : { ...cleanupSnapshot(), finishedAt: "2026-01-01T00:00:00Z", fileBytesBefore: 20_000, fileBytesAfter: 8_000 }
          ) as T;
        default:
          throw new Error(`Unexpected broker call: ${call.call}`);
      }
    },
    listen: () => {},
  };
}

describe("first paint", () => {
  it("holds real content until accounts and models both arrive, then renders once", async () => {
    let resolveModelSettings: ((value: ModelSettingsSnapshot) => void) | undefined;
    const pendingModelSettings = new Promise<ModelSettingsSnapshot>((resolve) => {
      resolveModelSettings = resolve;
    });
    setTransport(makeTransport(undefined, undefined, () => pendingModelSettings));

    render(<SettingsPage />);

    // Accounts arrive well before models do; the skeleton must still be
    // showing here, with neither in the DOM yet.
    const skeleton = await screen.findByLabelText("Loading settings…");
    expect(skeleton.getAttribute("aria-busy")).toBe("true");
    expect(screen.queryByRole("button", { name: "Add worker" })).toBeNull();
    expect(screen.queryByText("Claude work")).toBeNull();

    await act(async () => {
      resolveModelSettings?.(snapshot());
    });

    expect(await screen.findByRole("button", { name: "Add worker" })).toBeTruthy();
    expect(screen.queryByLabelText("Loading settings…")).toBeNull();
  });

  it("reopening after the data is already loaded renders content immediately, no skeleton", async () => {
    setTransport(makeTransport());
    const first = render(<SettingsPage />);
    await screen.findByRole("button", { name: "Add worker" });
    first.unmount();

    render(<SettingsPage />);
    expect(screen.queryByLabelText("Loading settings…")).toBeNull();
    expect(screen.getByRole("button", { name: "Add worker" })).toBeTruthy();
  });
});

describe("workers list", () => {
  it("shows a consistent on/off state for every account and never renders a raw long model id", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);

    await screen.findByRole("button", { name: /Claude work/ });
    await screen.findByRole("button", { name: /OpenCode work/ });

    const claudeSwitch = screen.getByRole("switch", { name: /Enable Claude work|Disable Claude work/ });
    const opencodeSwitch = screen.getByRole("switch", { name: /Enable OpenCode work/ });
    expect((claudeSwitch as HTMLInputElement).checked).toBe(true);
    expect((opencodeSwitch as HTMLInputElement).checked).toBe(false);
    expect(document.querySelector('[data-provider-logo="claude"]')).toBeTruthy();
    expect(document.querySelector('[data-provider-logo="opencode"]')).toBeTruthy();
    expect(screen.queryByText(longModelId)).toBeNull();
  });

  it("opens one worker at a time on its own page, and the list comes back", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);

    fireEvent.click(await screen.findByRole("button", { name: /OpenCode work/ }));

    expect(await screen.findByRole("heading", { name: "Edit worker" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Claude work/ })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Workers" }));

    expect(await screen.findByRole("button", { name: /Claude work/ })).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "Edit worker" })).toBeNull();
  });

  it("adds a worker on its own page instead of inside the list", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);

    fireEvent.click(await screen.findByRole("button", { name: "Add worker" }));

    expect(await screen.findByRole("heading", { name: "Add worker" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /OpenCode work/ })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Workers" }));

    expect(await screen.findByRole("button", { name: /OpenCode work/ })).toBeTruthy();
  });

  it("names the kind of work each favourite model takes, in plain words", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);

    expect(await screen.findByText("Reading and lookups")).toBeTruthy();
    expect(screen.getByText("Everything else")).toBeTruthy();
    expect(screen.getByText("low effort")).toBeTruthy();
    expect(screen.queryByText("context")).toBeNull();
  });

  it("reads a favourite chain first to last, with each thinking level", async () => {
    const chained: ModelSettingsSnapshot = {
      ...snapshot(),
      love: [
        {
          model: "openai/gpt-5.6-luna",
          profileId: "opencode-work",
          when: ["context"],
          effort: "low",
          models: [
            { profileId: "opencode-work", model: "openai/gpt-5.6-luna", effort: "low" },
            { profileId: "claude-work", model: "opus", effort: "max" },
          ],
          scope: "project",
        },
      ],
    };
    setTransport(makeTransport(undefined, undefined, () => chained));
    render(<SettingsPage />);

    expect(
      await screen.findByText("opencode-work · openai/gpt-5.6-luna → claude-work · opus"),
    ).toBeTruthy();
    expect(screen.getByText("low → max effort")).toBeTruthy();
  });
});

describe("connections tab", () => {
  it("keeps tool connections out of the workers tab", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);

    await screen.findByRole("button", { name: /Claude work/ });
    expect(screen.queryByRole("heading", { name: "Connect command-line tools" })).toBeNull();

    fireEvent.click(screen.getByRole("tab", { name: "Connections" }));

    expect(await screen.findByRole("heading", { name: "Connect command-line tools" })).toBeTruthy();
  });
});

describe("settings navigation", () => {
  it("filters the rail by user-facing section labels", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);

    await screen.findByRole("tab", { name: "Workers" });
    fireEvent.change(screen.getByRole("searchbox", { name: "Search settings" }), { target: { value: "memories" } });

    expect(screen.getByRole("tab", { name: "Memories" })).toBeTruthy();
    expect(screen.queryByRole("tab", { name: "Workers" })).toBeNull();
    expect(screen.queryByRole("tab", { name: "Brief rules" })).toBeNull();

    fireEvent.click(screen.getByRole("tab", { name: "Memories" }));
    expect(await screen.findByRole("heading", { name: "Project memories" })).toBeTruthy();
  });
});

describe("appearance tab", () => {
  const plexStack = FONT_OPTIONS.find((option) => option.id === "plex-sans")!.stack;
  const systemStack = FONT_OPTIONS.find((option) => option.id === "system")!.stack;

  it("opens the App group, and the arrow keys reach it from the tab above", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);

    const appearanceTab = await screen.findByRole("tab", { name: "Appearance" });
    const group = appearanceTab.closest(".settings-nav-group")!;
    expect(group.querySelector(".settings-nav-label")?.textContent).toBe("App");
    expect(group.querySelector('[role="tab"]')).toBe(appearanceTab);

    const briefRules = screen.getByRole("tab", { name: "Brief rules" });
    fireEvent.click(briefRules);
    fireEvent.keyDown(briefRules, { key: "ArrowDown" });

    expect(appearanceTab.getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(appearanceTab);
    fireEvent.keyDown(appearanceTab, { key: "ArrowDown" });
    expect(screen.getByRole("tab", { name: "Task history" }).getAttribute("aria-selected")).toBe("true");
  });

  it("is found by searching for appearance or font", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);
    const search = await screen.findByRole("searchbox", { name: "Search settings" });

    for (const query of ["appearance", "Font"]) {
      fireEvent.change(search, { target: { value: query } });
      expect(screen.getByRole("tab", { name: "Appearance" })).toBeTruthy();
      expect(screen.queryByRole("tab", { name: "Workers" })).toBeNull();
    }
  });

  it("switches the interface font as soon as one is picked, and saves it", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);

    fireEvent.click(await screen.findByRole("tab", { name: "Appearance" }));
    const select = await screen.findByRole<HTMLSelectElement>("combobox", { name: /^Font/ });
    expect(select.value).toBe("plex-sans");

    fireEvent.change(select, { target: { value: "system" } });

    expect(document.documentElement.style.getPropertyValue("--font-sans")).toBe(systemStack);
    await waitFor(() => expect(savedAppearance).toEqual({ font: "system" }));
    expect(select.value).toBe("system");
  });

  it("shows and uses IBM Plex Sans for a font this build does not know", async () => {
    appearance = { font: "comic-sans-future" };
    setTransport(makeTransport());
    render(<SettingsPage />);

    fireEvent.click(await screen.findByRole("tab", { name: "Appearance" }));
    const select = await screen.findByRole<HTMLSelectElement>("combobox", { name: /^Font/ });

    expect(select.value).toBe("plex-sans");
    expect(select.selectedOptions[0]?.textContent).toBe("IBM Plex Sans");
    expect(document.documentElement.style.getPropertyValue("--font-sans")).toBe(plexStack);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("storage tab", () => {
  it("previews cleanup, saves choices, and confirms removal", async () => {
    let saved: CleanupSettings | undefined;
    let ran = false;
    setTransport(
      makeTransport(undefined, undefined, undefined, undefined, (settings) => {
        saved = settings;
        return cleanupSnapshot(settings);
      }, () => {
        ran = true;
        return { ...cleanupSnapshot(), finishedAt: "2026-01-01T00:00:00Z", fileBytesBefore: 20_000, fileBytesAfter: 8_000 };
      }),
    );
    render(<SettingsPage />);

    fireEvent.click(await screen.findByRole("tab", { name: "Task history" }));
    expect(await screen.findByRole("heading", { name: "Logs ready to remove" })).toBeTruthy();

    fireEvent.click(screen.getByRole("checkbox", { name: /Remove old logs automatically/ }));
    expect(screen.getByRole("button", { name: "Review and remove now" }).hasAttribute("disabled")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saved?.enabled).toBe(true));

    fireEvent.click(screen.getByRole("button", { name: "Review and remove now" }));
    expect(screen.getByRole("button", { name: "Remove logs now" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Remove logs now" }));
    await waitFor(() => expect(ran).toBe(true));
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("Removed 12 logs"));
  });
});

describe("settings refresh", () => {
  it("refreshes models when the modal opens again", async () => {
    const refreshes: boolean[] = [];
    setTransport(
      makeTransport(undefined, undefined, (refresh) => {
        refreshes.push(refresh);
        return snapshot();
      }),
    );
    const view = render(<SettingsPage open={false} />);

    await screen.findByRole("button", { name: /OpenCode work/ });
    view.rerender(<SettingsPage open />);

    await waitFor(() => expect(refreshes).toContain(true));
  });
});

describe("worker availability", () => {
  it("updates the selected worker from its accessible switch", async () => {
    let received: unknown;
    setTransport(
      makeTransport(undefined, (request) => {
        received = request;
        return { ...profiles[1], enabled: true };
      }),
    );
    render(<SettingsPage />);

    await screen.findByRole("button", { name: /OpenCode work/ });
    const workerSwitch = screen.getByRole("switch", { name: /Enable OpenCode work/ });
    expect((workerSwitch as HTMLInputElement).checked).toBe(false);

    fireEvent.click(workerSwitch);

    await waitFor(() => expect((workerSwitch as HTMLInputElement).checked).toBe(true));
    expect(received).toMatchObject({ profileId: "opencode-work", enabled: true });
  });

  it("shows the new state immediately and rolls back a failed save", async () => {
    setTransport(
      makeTransport(undefined, () => {
        throw new Error("offline");
      }),
    );
    render(<SettingsPage />);

    await screen.findByRole("button", { name: /OpenCode work/ });
    const workerSwitch = screen.getByRole("switch", { name: /Enable OpenCode work/ });

    fireEvent.click(workerSwitch);
    expect((workerSwitch as HTMLInputElement).checked).toBe(true);

    await waitFor(() => expect((workerSwitch as HTMLInputElement).checked).toBe(false));
  });
});

describe("model access", () => {
  it("lists every model for the selected account, including disabled ones", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);

    fireEvent.click(await screen.findByRole("button", { name: /OpenCode work/ }));

    expect(await screen.findByRole("switch", { name: /openai\/gpt-5\.6-luna/ })).toBeTruthy();
    const enabledSwitch = screen.getByRole("switch", { name: new RegExp(longModelId.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")) });
    const disabledSwitch = screen.getByRole("switch", { name: /openai\/gpt-5\.6-luna/ });
    expect((enabledSwitch as HTMLInputElement).checked).toBe(true);
    expect((disabledSwitch as HTMLInputElement).checked).toBe(false);
  });

  it("toggling a switch sends the update and persists the new state", async () => {
    let received: unknown;
    const transport = makeTransport((request) => {
      received = request;
      const next = snapshot();
      const worker = next.workers.find((w) => w.id === "opencode-work")!;
      const model = worker.models.find((m) => m.id === "openai/gpt-5.6-luna")!;
      model.enabled = true;
      model.hasEnabledOverride = true;
      return next;
    });
    setTransport(transport);
    render(<SettingsPage />);

    fireEvent.click(await screen.findByRole("button", { name: /OpenCode work/ }));
    const disabledSwitch = await screen.findByRole("switch", { name: /openai\/gpt-5\.6-luna/ });
    expect((disabledSwitch as HTMLInputElement).checked).toBe(false);

    fireEvent.click(disabledSwitch);

    await waitFor(() => expect((disabledSwitch as HTMLInputElement).checked).toBe(true));
    expect(received).toMatchObject({
      profileId: "opencode-work",
      modelId: "openai/gpt-5.6-luna",
      enabled: true,
    });
  });

  it("reflects a model's current allowed state on its switch", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);

    fireEvent.click(await screen.findByRole("button", { name: /Claude work/ }));

    const allowed = await screen.findByRole("switch", { name: /Opus/ });
    expect(allowed.hasAttribute("disabled")).toBe(false);
    expect((allowed as HTMLInputElement).checked).toBe(false);
  });
});

describe("brief rules", () => {
  it("shows brief rules a project file owns as read-only, and says where to edit them", async () => {
    callerPrompt = {
      cwd: "/tmp/project",
      scope: "project",
      written: false,
      value: "Always name the entry file.",
      inherited: "",
      configPath: "/tmp/project/.oga.yaml",
    };
    setTransport(makeTransport());
    render(<SettingsPage />);

    fireEvent.click(await screen.findByRole("tab", { name: "Brief rules" }));

    const editor = await screen.findByLabelText("How briefs are written");
    expect(editor.hasAttribute("readonly")).toBe(true);
    expect(screen.getByDisplayValue("Always name the entry file.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Save changes" })).toBeNull();
  });

  it("saves brief rules the user types for a scope no project file owns", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);

    fireEvent.click(await screen.findByRole("tab", { name: "Brief rules" }));
    const editor = await screen.findByLabelText("How briefs are written");
    fireEvent.change(editor, { target: { value: "Name the entry file." } });
    fireEvent.click(await screen.findByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(savedCallerPrompt).toBeTruthy());
    expect(savedCallerPrompt).toMatchObject({ written: true, value: "Name the entry file." });
  });

  it("leaves the default unset when the user clears the editor", async () => {
    setTransport(makeTransport());
    render(<SettingsPage />);

    fireEvent.click(await screen.findByRole("tab", { name: "Brief rules" }));
    const editor = await screen.findByLabelText("How briefs are written");
    expect((editor as HTMLTextAreaElement).value).toBe(defaultBriefRules);
    fireEvent.change(editor, { target: { value: "" } });
    fireEvent.click(await screen.findByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(savedCallerPrompt).toBeTruthy());
    expect(savedCallerPrompt).toMatchObject({ written: false, value: "" });
  });
});
