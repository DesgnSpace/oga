import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { setTransport } from "@/bridge/transport";
import type { ModelSettingsModel, ModelSettingsSnapshot, Task, WorkerSettings } from "@/bridge/types";
import { TaskHeaderActions } from "./Actions";

const task = {
  id: "task-1",
  profileId: "claude",
  model: "opus",
  cwd: "/repo",
  state: "running",
  prompt: "Tidy the parser",
  createdAt: "2026-09-26T10:00:00Z",
  updatedAt: "2026-09-26T10:00:00Z",
} as Task;

function model(id: string, label: string): ModelSettingsModel {
  return {
    id,
    label,
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
  };
}

function worker(id: string, label: string, models: ModelSettingsModel[]): WorkerSettings {
  return {
    id,
    label,
    provider: "claude",
    enabled: true,
    inheritedEnabled: true,
    hasEnabledOverride: false,
    availableGlobally: true,
    configured: true,
    models,
  };
}

const enabledModels: ModelSettingsSnapshot = {
  cwd: "/repo",
  scope: "project",
  revision: "r1",
  love: [],
  workers: [
    worker("claude", "Claude", [model("sonnet", "Sonnet")]),
    worker("codex", "Codex", [model("gpt-5", "GPT-5"), model("gpt-5-mini", "GPT-5 mini")]),
  ],
};

describe("the handoff dialog", () => {
  afterEach(cleanup);

  it("offers the workers and models from the enabled-model read", async () => {
    const calls: string[] = [];
    setTransport({
      invoke: async (command: string, args?: Record<string, unknown>) => {
        if (command !== "broker_call") throw { message: `no handler for ${command}` };
        const call = (args?.call as { call: string }).call;
        calls.push(call);
        if (call === "summary") {
          return {
            profiles: [
              { id: "claude", label: "Claude", provider: "claude", model: "sonnet", enabled: true, env: {}, capabilities: [] },
              { id: "codex", label: "Codex", provider: "codex", model: "gpt-5", enabled: true, env: {}, capabilities: [] },
            ],
            tasks: [],
            tasksHasMore: false,
            profileFailures: [],
            grants: [],
            memoryProjects: [],
          } as never;
        }
        if (call === "enabledModels") return enabledModels as never;
        throw { message: `no handler for ${call}` };
      },
      listen: () => {},
    });

    render(<TaskHeaderActions task={task} onChanged={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    fireEvent.click(await screen.findByText("Move to another worker"));

    const workerSelect = await screen.findByLabelText("Worker");
    expect(within(workerSelect).getAllByRole("option").map((option) => option.textContent)).toEqual([
      "Claude",
      "Codex",
    ]);
    expect((workerSelect as HTMLSelectElement).value).toBe("codex");
    const modelSelect = screen.getByLabelText("Model");
    expect(within(modelSelect).getAllByRole("option").map((option) => option.textContent)).toEqual([
      "GPT-5",
      "GPT-5 mini",
    ]);
    expect(screen.getByText("Current: Claude / opus")).toBeTruthy();
    expect(calls).not.toContain("modelSettings");
  });
});
