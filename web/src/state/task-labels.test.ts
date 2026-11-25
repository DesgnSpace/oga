import { describe, expect, it } from "bun:test";
import { projectLabel, projectParents } from "./task-labels";

describe("projects that share a folder name", () => {
  const evalRuns = [
    "/evals/runs/1858/claude-sonnet/orchestrate",
    "/evals/runs/1858/opencode-tidepool/orchestrate",
    "/evals/runs/1858/claude-sonnet/fix-bug",
    "/work/oga",
  ];

  it("tells them apart by the nearest parent folder that differs", () => {
    const parents = projectParents(evalRuns);
    expect(projectLabel(evalRuns[0], parents)).toBe("claude-sonnet/orchestrate");
    expect(projectLabel(evalRuns[1], parents)).toBe("opencode-tidepool/orchestrate");
    expect(projectLabel(evalRuns[2], parents)).toBe("fix-bug");
    expect(projectLabel(evalRuns[3], parents)).toBe("oga");
  });

  it("climbs past a parent folder they share, and only for the ones that share it", () => {
    const paths = ["/clients/acme/app", "/clients/globex/app", "/a/x/app", "/b/x/app"];
    const parents = projectParents(paths);
    expect(paths.map((path) => projectLabel(path, parents))).toEqual(["acme/app", "globex/app", "a/x/app", "b/x/app"]);
  });
});
