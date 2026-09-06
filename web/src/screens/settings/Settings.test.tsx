import { afterEach, describe, expect, it } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { setTransport, type Transport } from "@/bridge/transport";
import type {
  CleanupResult,
  CleanupSettings,
  CleanupSnapshot,
  ModelSettingsSnapshot,
  ProfileView,
  PromptConfig,
} from "@/bridge/types";
import { clearCachedSettingsState } from "./state";
import { SettingsPage } from "./index";

const inheritedPrompt: PromptConfig = { cwd: "/tmp/project", scope: "global", written: false, value: "", inherited: "" };
let prompt = inheritedPrompt;

afterEach(() => {
  cleanup();
  clearCachedSettingsState();
  prompt = inheritedPrompt;
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
      const call = args?.call as {
        call: string;
        request?: unknown;
        profileId?: string;
        patch?: { enabled?: boolean };
        refresh?: boolean;
        settings?: CleanupSettings;
      };
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
        case "prompt":
          return prompt as T;
        case "cleanup":
          return (onCleanup ? onCleanup() : cleanupSnapshot()) as T;
        case "putCleanup":
          return (onPutCleanup ? onPutCleanup(call.settings!) : cleanupSnapshot(call.settings!)) as T;
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
    expect(claudeSwitch.getAttribute("aria-checked")).toBe("true");
    expect(opencodeSwitch.getAttribute("aria-checked")).toBe("false");
    expect(document.querySelector('[data-provider-logo="claude"]')).toBeTruthy();
    expect(document.querySelector('[data-provider-logo="opencode"]')).toBeTruthy();
    expect(screen.queryByText(longModelId)).toBeNull();
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
    expect(screen.queryByRole("tab", { name: "Prompts" })).toBeNull();

    fireEvent.click(screen.getByRole("tab", { name: "Memories" }));
    expect(await screen.findByRole("heading", { name: "Project memories" })).toBeTruthy();
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

    fireEvent.click(await screen.findByRole("tab", { name: "Storage" }));
    expect(await screen.findByRole("heading", { name: "Logs ready to remove" })).toBeTruthy();

    fireEvent.click(screen.getByRole("checkbox", { name: /Remove old logs automatically/ }));
    expect(screen.getByRole("button", { name: "Review and remove now" }).hasAttribute("disabled")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Save storage choices" }));
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
    expect(workerSwitch.getAttribute("aria-checked")).toBe("false");

    fireEvent.click(workerSwitch);

    await waitFor(() => expect(workerSwitch.getAttribute("aria-checked")).toBe("true"));
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
    expect(workerSwitch.getAttribute("aria-checked")).toBe("true");

    await waitFor(() => expect(workerSwitch.getAttribute("aria-checked")).toBe("false"));
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
    expect(enabledSwitch.getAttribute("aria-checked")).toBe("true");
    expect(disabledSwitch.getAttribute("aria-checked")).toBe("false");
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
    expect(disabledSwitch.getAttribute("aria-checked")).toBe("false");

    fireEvent.click(disabledSwitch);

    await waitFor(() => expect(disabledSwitch.getAttribute("aria-checked")).toBe("true"));
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
    expect(allowed.getAttribute("aria-checked")).toBe("false");
  });
});

describe("worker instructions", () => {
  it("shows instructions a project file owns as read-only, and says where to edit them", async () => {
    prompt = {
      cwd: "/tmp/project",
      scope: "project",
      written: false,
      value: "1. Blocked means stop.",
      inherited: "",
      configPath: "/tmp/project/.oga.yaml",
    };
    setTransport(makeTransport());
    render(<SettingsPage />);

    fireEvent.click(await screen.findByRole("tab", { name: "Worker Prompt" }));

    const editor = await screen.findByLabelText("Worker instructions");
    expect(editor.hasAttribute("readonly")).toBe(true);
    expect(screen.getByDisplayValue("1. Blocked means stop.")).toBeTruthy();
    expect(screen.getByText("Set in /tmp/project/.oga.yaml. Edit that file to change them.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Save changes" })).toBeNull();
  });
});
