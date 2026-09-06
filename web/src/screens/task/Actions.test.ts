import { describe, expect, test } from "bun:test";
import {
  archiveBranchSuccess,
  canCancel,
  canComplete,
  canHandoff,
  canPause,
  canResume,
  explainBlocked,
} from "./Actions";
import type { ArchiveTaskResponse, Task, TaskCompletion, TaskScope } from "@/bridge/types";

function scope(read: string[], write: string[]): TaskScope {
  return { read, write };
}

function completion(
  code: TaskCompletion["code"],
  reason: string | undefined,
  suggestedScope: TaskScope | undefined,
): TaskCompletion {
  return { blocked: true, code, reason, suggestedScope };
}

describe("canCancel / canComplete", () => {
  test("every unfinished state can be marked completed", () => {
    const states: Task["state"][] = [
      "queued",
      "pending",
      "running",
      "needs_input",
      "answered",
      "blocked",
      "failed",
      "cancelled",
    ];
    for (const state of states) {
      expect(canComplete({ state } as Task)).toBe(true);
    }
    expect(canComplete({ state: "completed" } as Task)).toBe(false);
  });

  test("cancellable states exclude answered and completed", () => {
    expect(canCancel({ state: "running" } as Task)).toBe(true);
    expect(canCancel({ state: "answered" } as Task)).toBe(false);
    expect(canCancel({ state: "completed" } as Task)).toBe(false);
  });
});

describe("canPause / canResume", () => {
  test("only a running task can be paused", () => {
    expect(canPause({ state: "running" } as Task)).toBe(true);
    expect(canPause({ state: "queued" } as Task)).toBe(false);
    expect(canPause({ state: "cancelled" } as Task)).toBe(false);
    expect(canPause({ state: "completed" } as Task)).toBe(false);
  });

  test("only a paused (cancelled) task can be resumed", () => {
    expect(canResume({ state: "cancelled" } as Task)).toBe(true);
    expect(canResume({ state: "running" } as Task)).toBe(false);
    expect(canResume({ state: "completed" } as Task)).toBe(false);
    expect(canResume({ state: "failed" } as Task)).toBe(false);
  });
});

describe("canHandoff", () => {
  test("allows every unfinished task while keeping completed tasks settled", () => {
    for (const state of ["queued", "pending", "running", "needs_input", "answered", "blocked", "failed", "cancelled"] as const) {
      expect(canHandoff({ state })).toBe(true);
    }
    expect(canHandoff({ state: "completed" })).toBe(false);
  });
});

describe("explainBlocked", () => {
  test("a permission denial names only the newly-suggested write scope", () => {
    const current = scope(["**"], ["src/**"]);
    const suggested = scope(["**"], ["src/**", "docs/**"]);
    const explanation = explainBlocked(completion("permission_denied", undefined, suggested), current);
    expect(explanation.deniedPaths).toEqual(["docs/**"]);
    expect(explanation.suggestedScope).toEqual(suggested);
    expect(explanation.headline).not.toContain("permission_denied");
  });

  test("a worker error shows its reason", () => {
    const explanation = explainBlocked(
      completion("worker_error", "worker exited without output", undefined),
      undefined,
    );
    expect(explanation.rawReason).toBe("worker exited without output");
  });

  test("no completion at all gets the generic no-record headline", () => {
    const explanation = explainBlocked(undefined, undefined);
    expect(explanation.headline).toBe("This task stopped unexpectedly, and there's no record of why.");
  });
});

describe("archiveBranchSuccess", () => {
  test("does not claim branch deletion when the reply has no branch status", () => {
    expect(archiveBranchSuccess({ title: "Build the widget" }, undefined)).toBe(
      '"Build the widget" archived; branch status unavailable',
    );
  });

  test("explains why Git kept a branch", () => {
    const response: ArchiveTaskResponse = {
      id: "task",
      state: "completed",
      branchOutcome: "kept",
      branchReason: "branch has unmerged commits",
    };

    expect(archiveBranchSuccess({ title: "Build the widget" }, response)).toBe(
      '"Build the widget" archived; branch kept: branch has unmerged commits',
    );
  });
});
