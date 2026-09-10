import type { ProfileView, Task } from "@/bridge/types";

type ResumeTask = Pick<Task, "cwd" | "sessionId" | "worktree">;

const SAFE_ARG = /^[A-Za-z0-9_@%+=:,./-]+$/;

function quoteArg(value: string): string {
  if (SAFE_ARG.test(value)) return value;
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

function quoteEnvValue(value: string): string {
  const homeRelative = value === "~" ? "$HOME" : value.startsWith("~/") ? `$HOME/${value.slice(2)}` : value;
  if (homeRelative.includes("$HOME")) {
    return `"${homeRelative.replace(/\\/g, "\\\\").replace(/"/g, `\\"`)}"`;
  }
  return quoteArg(homeRelative);
}

function resumeArgv(provider: ProfileView["provider"], session: string): string[] | null {
  switch (provider) {
    case "claude":
      return ["claude", "--resume", session];
    case "codex":
      return ["codex", "resume", session];
    case "opencode":
      return ["opencode", "--session", session];
    case "opencode-2":
      return ["opencode2", "--session", session];
    case "antigravity":
      return ["agy", "--conversation", session];
    case "pi":
      return ["pi", "--session", session];
    default:
      return null;
  }
}

function envPrefix(profile: ProfileView): string {
  const entries = Object.entries(profile.env ?? {});
  if (profile.provider === "claude" && profile.id !== "claude" && !entries.some(([key]) => key === "CLAUDE_CONFIG_DIR")) {
    entries.push(["CLAUDE_CONFIG_DIR", `$HOME/.${profile.id}`]);
  }
  if (entries.length === 0) return "";
  return `${entries.map(([key, value]) => `${key}=${quoteEnvValue(value)}`).join(" ")} `;
}

/**
 * Shell command that continues this task's provider session from a terminal.
 * Null when the task holds no session, its profile is unknown, or the profile
 * runs a custom command with no resumable session behind it.
 */
export function terminalResumeCommand(task: ResumeTask, profile: ProfileView | undefined): string | null {
  const session = task.sessionId?.trim();
  if (!session || !profile) return null;
  if (profile.command && profile.command.length > 0) return null;
  const argv = resumeArgv(profile.provider, session);
  if (!argv) return null;
  const directory = task.worktree?.path ?? task.cwd;
  return `cd ${quoteArg(directory)} && ${envPrefix(profile)}${argv.map(quoteArg).join(" ")}`;
}
