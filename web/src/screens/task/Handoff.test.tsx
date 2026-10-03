import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
    preferred: false,
    capabilities: [],
  };
}

function worker(id: string, label: string, models: ModelSettingsModel[]): WorkerSettings {
  return {
    id,
    label,
    provider: "claude",
    enabled: true,
    configured: true,
    models,
  };
}

const enabledModels: ModelSettingsSnapshot = {
  revision: "r1",
  love: [],
  workers: [
    worker("claude", "Claude", [model("sonnet", "Sonnet")]),
    worker("codex", "Codex", [model("gpt-5", "GPT-5"), model("gpt-5-mini", "GPT-5 mini")]),
  ],
};

function serve(calls: string[], requests: unknown[] = []) {
  setTransport({
    invoke: async (command: string, args?: Record<string, unknown>) => {
      if (command !== "broker_call") throw { message: `no handler for ${command}` };
      const { call, request } = args?.call as { call: string; request?: unknown };
      calls.push(call);
      if (call === "handoffTask") {
        requests.push(request);
        return undefined as never;
      }
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
}

describe("the handoff dialog", () => {
  afterEach(cleanup);

  it("offers the workers and models from the enabled-model read", async () => {
    const calls: string[] = [];
    serve(calls);

    render(<TaskHeaderActions task={task} events={[]} onChanged={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    fireEvent.click(await screen.findByText("Handoff"));

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

  it("hands off with the chosen effort, and with only a new effort on the same model", async () => {
    const requests: unknown[] = [];
    serve([], requests);

    render(<TaskHeaderActions task={{ ...task, profileId: "codex", model: "gpt-5", effort: "low" } as Task} events={[]} onChanged={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    fireEvent.click(await screen.findByText("Handoff"));

    fireEvent.change(await screen.findByLabelText("Worker"), { target: { value: "codex" } });
    fireEvent.change(screen.getByLabelText("Model"), { target: { value: "gpt-5" } });
    const submit = screen.getByRole("button", { name: "Hand off" });
    expect(submit.hasAttribute("disabled")).toBe(true);

    fireEvent.change(screen.getByLabelText("Effort"), { target: { value: "high" } });
    expect(submit.hasAttribute("disabled")).toBe(false);
    fireEvent.click(submit);

    await waitFor(() => expect(requests).toEqual([{ profile: "codex", model: "gpt-5", effort: "high" }]));
  });
});
