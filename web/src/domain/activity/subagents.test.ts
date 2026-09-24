// Subagent grouping against the rows each driver really sends, as the broker
// normalizes them in `rust/crates/oga-events/tests/subagents.rs`.

import { describe, expect, it } from "bun:test";
import type { TaskEventView } from "@/bridge/types";
import { TraceRowBuilder } from "@/domain/trace";
import { type ActivityComposition, type ActivityNode, type ActivitySubagent, ActivityStory } from "./index";

async function fixture(driver: string): Promise<TaskEventView[]> {
  // SAFETY: the fixtures are `event_views` output, serialized from `TaskEventView`.
  return (await Bun.file(new URL(`./fixtures/subagents/${driver}.json`, import.meta.url)).json()) as TaskEventView[];
}

async function compose(driver: string): Promise<ActivityComposition> {
  return ActivityStory.composeWithState(await fixture(driver), true, undefined, true);
}

function topNodes(composition: ActivityComposition): ActivityNode[] {
  return composition.blocks.flatMap((block) =>
    block.type === "turn" ? block.turn.segments.flatMap((segment) => segment.nodes) : [],
  );
}

function allNodes(nodes: ActivityNode[]): ActivityNode[] {
  return nodes.flatMap((node) =>
    node.type === "subagents" ? [node, ...allNodes(node.subagents.flatMap((subagent) => subagent.nodes))] : [node],
  );
}

function subagentsOf(composition: ActivityComposition): ActivitySubagent[] {
  return allNodes(topNodes(composition)).flatMap((node) => (node.type === "subagents" ? node.subagents : []));
}

function rowsOf(node: ActivityNode): TaskEventView[] {
  if (node.type === "call") return node.call.events;
  if (node.type === "notice" || node.type === "message") return [node.event];
  return [];
}

/** The subagent ids a subagent's own work names. */
function memberIds(subagent: ActivitySubagent): Set<string> {
  return new Set(
    subagent.nodes.flatMap(rowsOf).flatMap((row) => (row.subagents ?? []).filter((link) => link.role === "member")).map((link) => link.id),
  );
}

describe("subagent fixtures", () => {
  it.each(["claude", "opencode2"])("%s nests three subagents with their own calls inside", async (driver) => {
    const composition = await compose(driver);
    const subagents = subagentsOf(composition);

    expect(subagents).toHaveLength(3);
    for (const subagent of subagents) {
      expect(subagent.label).toBeDefined();
      expect(subagent.nodes.length).toBeGreaterThan(0);
      expect([...memberIds(subagent)]).toEqual([subagent.id.slice("subagent:".length)]);
      expect(subagent.status).toBe("done");
    }
    // None of their calls is left in the parent's timeline.
    const loose = topNodes(composition).flatMap(rowsOf).filter((row) => row.subagents?.some((link) => link.role === "member"));
    expect(loose).toEqual([]);
  });

  it("claude launches back to back sit under one node", async () => {
    const batches = topNodes(await compose("claude")).filter((node) => node.type === "subagents");
    expect(batches.map((node) => (node.type === "subagents" ? node.subagents.length : 0))).toEqual([3]);
  });

  it.each(["codex", "opencode", "fx"])("%s shows three subagents as cards without calls", async (driver) => {
    const subagents = subagentsOf(await compose(driver));
    const finished = subagents.filter((subagent) => subagent.status === "done");

    expect(finished).toHaveLength(3);
    expect(subagents.every((subagent) => subagent.nodes.length === 0)).toBe(true);
  });

  it.each(["codex", "fx"])("%s cards carry each subagent's report", async (driver) => {
    const finished = subagentsOf(await compose(driver)).filter((subagent) => subagent.status === "done");
    expect(finished.map((subagent) => subagent.report !== undefined)).toEqual([true, true, true]);
  });

  it("fx keeps the launches that failed as failed cards", async () => {
    const subagents = subagentsOf(await compose("fx"));
    expect(subagents.filter((subagent) => subagent.status === "failed")).toHaveLength(3);
  });

  it("codex folds its report rows into the cards", async () => {
    const composition = await compose("codex");
    const reports = allNodes(topNodes(composition))
      .flatMap(rowsOf)
      .filter((row) => row.subagents?.some((link) => link.role === "report"));
    expect(reports).toEqual([]);
    expect(composition.technical.filter((row) => row.subagents !== undefined)).toEqual([]);
  });

  it("antigravity shows one card", async () => {
    const subagents = subagentsOf(await compose("antigravity"));
    expect(subagents).toHaveLength(1);
    expect(subagents[0]?.nodes).toEqual([]);
  });

  it("pi shows no subagents", async () => {
    expect(subagentsOf(await compose("pi"))).toEqual([]);
  });

  it.each(["claude", "codex", "opencode", "opencode2", "antigravity", "pi", "fx"])(
    "%s shows no launch as thinking",
    async (driver) => {
      const events = await fixture(driver);
      const launches = new Set(
        events.filter((row) => row.subagents?.some((link) => link.role === "launch")).map((row) => row.actionId),
      );
      const composition = ActivityStory.composeWithState(events, true, undefined, true);
      const nodes = allNodes(topNodes(composition));
      const outside = nodes.flatMap(rowsOf).filter((row) => launches.has(row.actionId));
      const launchRows = new Set(events.filter((row) => launches.has(row.actionId)).map((row) => row.id));
      const pulses = nodes.flatMap((node) => (node.type === "thinking" ? [node.pulse.id] : []));

      expect(outside).toEqual([]);
      expect(pulses.filter((id) => launchRows.has(id))).toEqual([]);
    },
  );
});

describe("subagent trace rows", () => {
  it("labels a batch by its count and each subagent by its label", async () => {
    const composition = await compose("claude");
    const rows = TraceRowBuilder.rows(composition.blocks, "/", false);
    const batch = rows.find((row) => row.target === "3 subagents");

    expect(batch?.children.map((row) => row.target)).toEqual([
      "Subagent · Survey rust/crates directory",
      "Subagent · Survey web activity domain files",
      "Subagent · Read CHANGELOG unreleased section",
    ]);
    expect(batch?.children.every((row) => row.children.length > 0)).toBe(true);
  });

  it("opens a card onto its report", async () => {
    const composition = await compose("codex");
    const rows = TraceRowBuilder.rows(composition.blocks, "/", false);
    const cards = rows.flatMap(function walk(row): typeof rows {
      return row.nodeId?.startsWith("subagent:") ? [row] : row.children.flatMap(walk);
    });

    expect(cards).toHaveLength(3);
    expect(cards.every((row) => row.expansion?.type === "report" && !row.startsExpanded)).toBe(true);
  });
});
