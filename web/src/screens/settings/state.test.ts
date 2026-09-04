import { describe, expect, it } from "bun:test";
import type { ModelSettingsSnapshot, PromptConfig, Provider } from "@/bridge/types";
import {
  applyMemoryEntries,
  applyMemoryProjects,
  applyModelLoadError,
  applyModelSnapshot,
  applyOptimisticModelUpdate,
  applyOverview,
  applyPromptConfig,
  beginMemoryLoad,
  beginModelLoad,
  beginModelUpdate,
  beginPromptLoad,
  defaultMemoryState,
  defaultModelSettingsStore,
  defaultPromptsModel,
  defaultSettingsState,
  finishModelUpdate,
  finishPromptSave,
  isPromptDirty,
  isSecretKey,
  modelRowKey,
  profileError,
  projectName,
  providerFromString,
  providerLabel,
  resetPrompt,
  scopeFromKey,
  scopeKey,
  updatePromptText,
} from "./state";

function snapshot(revision = "abc"): ModelSettingsSnapshot {
  return {
    cwd: "/tmp/project",
    scope: "project",
    revision,
    workers: [
      {
        id: "claude-work",
        label: "Claude work",
        provider: "claude" as Provider,
        enabled: true,
        inheritedEnabled: true,
        hasEnabledOverride: false,
        availableGlobally: true,
        configured: true,
        models: [
          {
            id: "opus",
            label: "Opus",
            enabled: true,
            inheritedEnabled: true,
            hasEnabledOverride: false,
            preferred: false,
            inheritedPreferred: false,
            hasPreferredOverride: false,
            capabilities: ["build"],
            inheritedCapabilities: ["build"],
            hasCapabilitiesOverride: false,
            availableGlobally: true,
          },
        ],
      },
    ],
  };
}

describe("modelRowKey", () => {
  it("separates worker from model", () => {
    expect(modelRowKey("claude-work")).toBe("claude-work");
    expect(modelRowKey("claude-work", "opus")).toBe("claude-work/opus");
  });
});

describe("model settings store", () => {
  it("begins load clearing errors", () => {
    const store = applyModelLoadError(defaultModelSettingsStore(), "offline");
    const next = beginModelLoad(store);
    expect(next.loading).toBe(true);
    expect(next.loadError).toBeUndefined();
  });

  it("applies snapshot and clears pending", () => {
    let store = beginModelLoad(defaultModelSettingsStore());
    store = applyModelSnapshot(store, snapshot());
    expect(store.loading).toBe(false);
    expect(store.snapshot?.revision).toBe("abc");
  });

  it("optimistic update toggles enabled immediately", () => {
    let store = applyModelSnapshot(defaultModelSettingsStore(), snapshot());
    const key = modelRowKey("claude-work", "opus");
    store = beginModelUpdate(store, key);
    store = applyOptimisticModelUpdate(store, "claude-work", "opus", { enabled: false });
    expect(store.snapshot?.workers[0].models[0].enabled).toBe(false);
    expect(store.pending.has(key)).toBe(true);
  });

  it("rolls back optimistic on 409 conflict", () => {
    let store = applyModelSnapshot(defaultModelSettingsStore(), snapshot("rev1"));
    const key = modelRowKey("claude-work", "opus");
    store = beginModelUpdate(store, key);
    store = applyOptimisticModelUpdate(store, "claude-work", "opus", { enabled: false });
    expect(store.snapshot?.workers[0].models[0].enabled).toBe(false);
    store = finishModelUpdate(store, key, { ok: false, status: 409 });
    expect(store.snapshot?.workers[0].models[0].enabled).toBe(true);
    expect(store.saveError).toBe("This file changed outside Oga. Reload it before saving.");
    expect(store.pending.has(key)).toBe(false);
  });

  it("keeps optimistic on non-conflict error with message", () => {
    let store = applyModelSnapshot(defaultModelSettingsStore(), snapshot());
    const key = modelRowKey("claude-work", "opus");
    store = beginModelUpdate(store, key);
    store = applyOptimisticModelUpdate(store, "claude-work", "opus", { preferred: true });
    store = finishModelUpdate(store, key, { ok: false, status: 500 });
    // Optimistic value stays (no rollback except 409), but error surfaces
    expect(store.snapshot?.workers[0].models[0].preferred).toBe(true);
    expect(store.saveError).toBe("Couldn't save the model choice. Try again.");
  });

  it("applies server snapshot on success", () => {
    let store = applyModelSnapshot(defaultModelSettingsStore(), snapshot("rev1"));
    const key = modelRowKey("claude-work", "opus");
    store = beginModelUpdate(store, key);
    store = applyOptimisticModelUpdate(store, "claude-work", "opus", { enabled: false });
    const nextSnap = snapshot("rev2");
    nextSnap.workers[0].models[0].enabled = false;
    nextSnap.workers[0].models[0].hasEnabledOverride = true;
    store = finishModelUpdate(store, key, { ok: true, snapshot: nextSnap });
    expect(store.snapshot?.revision).toBe("rev2");
    expect(store.pending.size).toBe(0);
    expect(store.saveError).toBeUndefined();
  });

  it("handles per-row pending independently", () => {
    let store = applyModelSnapshot(defaultModelSettingsStore(), snapshot());
    const k1 = modelRowKey("claude-work", "opus");
    const k2 = modelRowKey("claude-work", "sonnet");
    store = beginModelUpdate(store, k1);
    store = beginModelUpdate(store, k2);
    expect(store.pending.size).toBe(2);
    store = finishModelUpdate(store, k1, { ok: true, snapshot: snapshot("rev2") });
    expect(store.pending.has(k2)).toBe(true);
    expect(store.pending.has(k1)).toBe(false);
  });
});

describe("prompts model", () => {
  function cfg(written: boolean, value: string, inherited: string): PromptConfig {
    return { cwd: "/tmp/project", scope: "project", written, value, inherited };
  }

  it("marks inherited text as local when edited", () => {
    let m = applyPromptConfig(defaultPromptsModel(), cfg(false, "Inherited", "Inherited"));
    m = updatePromptText(m, "Custom");
    expect(isPromptDirty(m)).toBe(true);
    expect(m.written).toBe(true);
    expect(m.text).toBe("Custom");
  });

  it("resetting is an unsaved change", () => {
    let m = applyPromptConfig(defaultPromptsModel(), cfg(true, "Custom", "Inherited"));
    m = resetPrompt(m);
    expect(isPromptDirty(m)).toBe(true);
    expect(m.written).toBe(false);
    expect(m.text).toBe("Inherited");
  });

  it("begin load clears saved flag", () => {
    let m = applyPromptConfig(defaultPromptsModel(), cfg(true, "Custom", "Inherited"));
    m = { ...m, saved: true };
    m = beginPromptLoad(m);
    expect(m.loaded).toBe(false);
    expect(m.saved).toBe(false);
  });

  it("finish save maps unreachable error", () => {
    let m = beginPromptLoad(defaultPromptsModel());
    m = finishPromptSave(m, { ok: false, error: { kind: "unreachable" } });
    expect(m.saveError).toEqual({ kind: "unreachable" });
    expect(m.saving).toBe(false);
  });

  it("carries the file a project sets its instructions in", () => {
    const m = applyPromptConfig(defaultPromptsModel(), {
      ...cfg(false, "From the file", "Inherited"),
      configPath: "/tmp/project/.oga.yaml",
    });
    expect(m.configPath).toBe("/tmp/project/.oga.yaml");
    expect(m.text).toBe("From the file");
  });
});

describe("memories", () => {
  it("clears selection when project disappears", () => {
    let state = defaultMemoryState();
    state = applyMemoryProjects(state, [{ cwd: "/tmp/project", count: 1, chars: 4, updatedAt: "now" }]);
    state = beginMemoryLoad(state, "/tmp/project");
    state = applyMemoryProjects(state, []);
    expect(state.selectedProject).toBeUndefined();
    expect(state.entries).toEqual([]);
  });

  it("applies entries after load", () => {
    let state = beginMemoryLoad(defaultMemoryState(), "/tmp/project");
    state = applyMemoryEntries(state, [
      { cwd: "/tmp/project", key: "k", value: "v", version: 1, createdAt: "now", updatedAt: "now" },
    ]);
    expect(state.loaded).toBe(true);
    expect(state.entries).toHaveLength(1);
  });
});

describe("overview", () => {
  it("stores profiles and projects", () => {
    const s = applyOverview(defaultSettingsState(), { profiles: [], tasks: [], memoryProjects: [] }, { global: "/Users/me", projects: ["/tmp/project"] }, undefined);
    expect(s.projects?.global).toBe("/Users/me");
    expect(s.overview).toBe("ready");
  });
});

describe("helpers", () => {
  it("provider labels cover every supported provider", () => {
    expect(providerLabel("claude")).toBe("Claude");
    expect(providerLabel("opencode-2")).toBe("OpenCode 2");
    expect(providerLabel("pi")).toBe("Pi");
  });
  it("secret detection", () => {
    expect(isSecretKey("OPENAI_API_KEY")).toBe(true);
    expect(isSecretKey("CLAUDE_CONFIG_DIR")).toBe(false);
  });
  it("scope round trips", () => {
    expect(scopeFromKey("global")).toEqual({ kind: "global" });
    expect(scopeFromKey("/tmp/project")).toEqual({ kind: "project", path: "/tmp/project" });
    expect(scopeKey({ kind: "global" })).toBe("global");
    expect(scopeKey({ kind: "project", path: "/tmp/project" })).toBe("/tmp/project");
  });
  it("project name", () => {
    expect(projectName("/tmp/project")).toBe("project");
    expect(projectName("/")).toBe("/");
  });
  it("profile error maps 409", () => {
    expect(profileError("delete", 409)).toBe("This worker changed outside Oga. Refresh settings, then re-enter your changes.");
    expect(profileError("save", 500, "boom")).toBe("Couldn't save the worker: boom");
    expect(profileError("save")).toBe("Couldn't save the worker. Check that Oga is running, then try again.");
  });
  it("provider from string", () => {
    expect(providerFromString("claude")).toBe("claude");
    expect(providerFromString("unknown")).toBeUndefined();
  });
});
