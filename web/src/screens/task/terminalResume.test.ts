import { describe, expect, test } from "bun:test";
import type { ProfileView, Task } from "@/bridge/types";
import { terminalResumeCommand } from "./terminalResume";

function task(overrides: Partial<Task> = {}): Task {
  // SAFETY: the literal sets every required Task field; overrides only narrow optional ones.
  return {
    id: "task-1",
    profileId: "claude",
    model: "model",
    prompt: "prompt",
    cwd: "/repo",
    state: "completed",
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    output: "",
    scope: { read: [], write: [] },
    allowQuestions: false,
    canDelegate: false,
    sessionId: "session-1",
    ...overrides,
  } as Task;
}

function profile(overrides: Partial<ProfileView> = {}): ProfileView {
  return {
    id: "claude",
    label: "Claude",
    provider: "claude",
    model: "model",
    enabled: true,
    env: {},
    capabilities: [],
    ...overrides,
  };
}

describe("terminalResumeCommand", () => {
  test("builds the claude resume command", () => {
    expect(terminalResumeCommand(task(), profile())).toBe("cd /repo && claude --resume session-1");
  });

  test("builds each provider's interactive resume shape", () => {
    const cases: Array<[ProfileView["provider"], string, string]> = [
      ["codex", "thread-1", "cd /repo && codex resume thread-1"],
      ["opencode", "ses_abc", "cd /repo && opencode --session ses_abc"],
      ["opencode-2", "ses_abc", "cd /repo && opencode2 --session ses_abc"],
      ["antigravity", "conv-1", "cd /repo && agy --conversation conv-1"],
      ["pi", "pi-1", "cd /repo && pi --session pi-1"],
    ];
    for (const [provider, session, expected] of cases) {
      expect(terminalResumeCommand(task({ sessionId: session }), profile({ id: "worker", provider }))).toBe(expected);
    }
  });

  test("runs in the worktree checkout, not the project root", () => {
    const worktreeTask = task({
      cwd: "/checkout",
      worktree: { originCwd: "/repo", path: "/checkout", branch: "oga/work" },
    });
    expect(terminalResumeCommand(worktreeTask, profile())).toBe("cd /checkout && claude --resume session-1");
  });

  test("quotes directories and sessions the shell would split", () => {
    const quoted = task({ cwd: "/my repo", sessionId: "my session" });
    expect(terminalResumeCommand(quoted, profile())).toBe(`cd '/my repo' && claude --resume 'my session'`);
  });

  test("hides when there is no session, no profile, or a custom command", () => {
    expect(terminalResumeCommand(task({ sessionId: undefined }), profile())).toBeNull();
    expect(terminalResumeCommand(task({ sessionId: "  " }), profile())).toBeNull();
    expect(terminalResumeCommand(task(), undefined)).toBeNull();
    expect(terminalResumeCommand(task(), profile({ command: ["my-cli", "{prompt}"] }))).toBeNull();
  });

  test("passes the profile's own environment through", () => {
    const withEnv = profile({ env: { CODEX_HOME: "$HOME/.codex-me", API_KEY: "secret" } });
    expect(terminalResumeCommand(task(), { ...withEnv, id: "codex-me", provider: "codex" })).toBe(
      `cd /repo && CODEX_HOME="$HOME/.codex-me" API_KEY=secret codex resume session-1`,
    );
  });

  test("keeps a claude profile on its own account directory", () => {
    expect(terminalResumeCommand(task(), profile({ id: "claude-me" }))).toBe(
      `cd /repo && CLAUDE_CONFIG_DIR="$HOME/.claude-me" claude --resume session-1`,
    );
  });
});
