import { beforeEach, describe, expect, it, mock } from "bun:test";
import { broker, installMcpConfigs, streamStatus, unwatchTask, watchTask } from "./client";
import { setTransport, type Transport } from "./transport";
import type { BridgeError } from "./types";

function fakeTransport(invoke: Transport["invoke"]): Transport {
  return { invoke, listen: mock() };
}

beforeEach(() => {
  setTransport(fakeTransport(mock()));
});

describe("broker call serialisation", () => {
  it("wraps a read call in the wire shape broker_call expects", async () => {
    const invoke = mock().mockResolvedValue({ global: "/home/me", projects: ["/tmp/project"] });
    setTransport(fakeTransport(invoke));

    const result = await broker.projects();

    expect(invoke).toHaveBeenCalledWith("broker_call", { call: { call: "projects" } });
    expect(result).toEqual({ ok: true, value: { global: "/home/me", projects: ["/tmp/project"] } });
  });

  it("carries nested request bodies under their call variant", async () => {
    const invoke = mock().mockResolvedValue(undefined);
    setTransport(fakeTransport(invoke));

    await broker.resumeTask("task-1", { instruction: "keep going" });

    expect(invoke).toHaveBeenCalledWith("broker_call", {
      call: {
        call: "resumeTask",
        taskId: "task-1",
        request: { instruction: "keep going" },
      },
    });
  });

  it("sends the selected worker and model for a handoff", async () => {
    const invoke = mock().mockResolvedValue(undefined);
    setTransport(fakeTransport(invoke));

    await broker.handoffTask("task-1", { profile: "worker-2", model: "sonnet" });

    expect(invoke).toHaveBeenCalledWith("broker_call", {
      call: {
        call: "handoffTask",
        taskId: "task-1",
        request: { profile: "worker-2", model: "sonnet" },
      },
    });
  });

  it("passes optional query fields through untouched", async () => {
    const invoke = mock().mockResolvedValue({ events: [] });
    setTransport(fakeTransport(invoke));

    await broker.taskEvents("task-1", { after: 42, limit: 150 });

    expect(invoke).toHaveBeenCalledWith("broker_call", {
      call: { call: "taskEvents", taskId: "task-1", query: { after: 42, limit: 150 } },
    });
  });
});

describe("broker call failure", () => {
  it("surfaces a rejection as a typed result instead of throwing", async () => {
    const error: BridgeError = { message: "stale revision", status: 409 };
    const invoke = mock().mockRejectedValue(error);
    setTransport(fakeTransport(invoke));

    const result = await broker.archiveTask("task-1", true);

    expect(result).toEqual({ ok: false, error });
  });
});

describe("shell-only commands", () => {
  it("calls broker_stream_status with no arguments", async () => {
    const invoke = mock().mockResolvedValue({ connected: true, cursor: 1, streamFloor: 0, stale: false });
    setTransport(fakeTransport(invoke));

    const result = await streamStatus();

    expect(invoke).toHaveBeenCalledWith("broker_stream_status", undefined);
    expect(result.ok).toBe(true);
  });

  it("calls broker_watch_task with the task id and event count", async () => {
    const invoke = mock().mockResolvedValue({ task: {}, events: [], cursor: 0, hasEarlier: false });
    setTransport(fakeTransport(invoke));

    await watchTask("task-1", 150);

    expect(invoke).toHaveBeenCalledWith("broker_watch_task", { taskId: "task-1", events: 150 });
  });

  it("calls broker_unwatch_task with the task id", async () => {
    const invoke = mock().mockResolvedValue(undefined);
    setTransport(fakeTransport(invoke));

    await unwatchTask("task-1");

    expect(invoke).toHaveBeenCalledWith("broker_unwatch_task", { taskId: "task-1" });
  });

  it("calls install_mcp_configs with the profile list", async () => {
    const invoke = mock().mockResolvedValue([]);
    setTransport(fakeTransport(invoke));

    await installMcpConfigs([]);

    expect(invoke).toHaveBeenCalledWith("install_mcp_configs", { profiles: [] });
  });
});
