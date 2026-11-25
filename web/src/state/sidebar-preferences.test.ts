// Ported from rust/crates/oga-ui/src/state/mod.rs `#[cfg(test)] mod tests`
// (the preference-encoding pieces).

import { describe, expect, it } from "bun:test";
import {
  decodeCollapsedGroups,
  encodeCollapsedGroups,
  toggleCollapsedGroup,
  collapsedGroupIds,
} from "./sidebar-preferences";

describe("collapsed groups", () => {
  it("round trips in sorted JSON", () => {
    let groups = toggleCollapsedGroup({ byMode: {} }, "completed", "status");
    groups = toggleCollapsedGroup(groups, "root", "parent");

    expect(encodeCollapsedGroups(groups)).toBe('{"parent":["root"],"status":["completed"]}');
    const restored = decodeCollapsedGroups(encodeCollapsedGroups(groups));
    expect(collapsedGroupIds(restored, "parent")).toEqual(new Set(["root"]));
    expect(collapsedGroupIds(restored, "none").size).toBe(0);
  });

  it("ignores malformed JSON", () => {
    expect(collapsedGroupIds(decodeCollapsedGroups('{"parent":5}'), "parent").size).toBe(0);
  });

});
