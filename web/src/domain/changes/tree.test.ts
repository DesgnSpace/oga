import { describe, expect, it } from "bun:test";
import type { ChangedFileView } from ".";
import { buildFileTree } from "./tree";

function file(path: string): ChangedFileView {
  return { path, change: { path, blocks: [] }, added: 1, removed: 0, hiddenLines: 0, shortened: false };
}

describe("buildFileTree", () => {
  it("nests files under shared directories, directories before files, alphabetically", () => {
    const tree = buildFileTree([file("b.ts"), file("src/index.ts"), file("src/lib/a.ts"), file("src/lib/b.ts")]);

    expect(tree.map((node) => node.name)).toEqual(["src", "b.ts"]);
    const src = tree[0];
    if (src.kind !== "dir") throw new Error("expected dir");
    expect(src.children.map((node) => node.name)).toEqual(["lib", "index.ts"]);
    const lib = src.children[0];
    if (lib.kind !== "dir") throw new Error("expected dir");
    expect(lib.children.map((node) => node.name)).toEqual(["a.ts", "b.ts"]);
    expect(lib.path).toBe("src/lib");
  });

  it("keeps each file's original path and view on its leaf", () => {
    const tree = buildFileTree([file("src/index.ts")]);
    const src = tree[0];
    if (src.kind !== "dir") throw new Error("expected dir");
    const leaf = src.children[0];
    if (leaf.kind !== "file") throw new Error("expected file");
    expect(leaf.path).toBe("src/index.ts");
    expect(leaf.file.path).toBe("src/index.ts");
  });
});
