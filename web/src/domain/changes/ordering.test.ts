import { describe, expect, it } from "bun:test";
import type { ChangedFileView } from ".";
import { isGeneratedFilePath, orderChangeFiles } from "./ordering";
import { buildFileTree } from "./tree";

function file(path: string, added: number, removed = 0): ChangedFileView {
  return { path, change: { path, blocks: [] }, added, removed, hiddenLines: 0, shortened: false };
}

function paths(files: readonly ChangedFileView[]): string[] {
  return files.map((entry) => entry.path);
}

describe("isGeneratedFilePath", () => {
  it("sets aside what a machine writes and keeps what a person wrote", () => {
    const generated = [
      "bun.lock",
      "rust/Cargo.lock",
      "web/package-lock.json",
      "web/dist/app.js",
      "rust/target/debug/oga-server",
      "web/src/view.test.ts.snap",
      "web/public/app.min.js",
      "web/public/app.js.map",
      "web/src/__snapshots__/one.ts.snap",
    ];
    const written = ["web/src/one.ts", "rust/src/lib.rs", "web/src/lockfile.ts"];

    expect(generated.filter((path) => !isGeneratedFilePath(path))).toEqual([]);
    expect(written.filter((path) => isGeneratedFilePath(path))).toEqual([]);
  });
});

describe("orderChangeFiles", () => {
  it("leaves the order alone under folder order and ranks by size under size order", () => {
    const files = [file("web/src/one.ts", 2), file("bun.lock", 400), file("web/src/two.ts", 90), file("web/src/three.ts", 5)];

    const byFolder = orderChangeFiles(files, "folder");
    expect(paths(byFolder.files)).toEqual(["web/src/one.ts", "web/src/two.ts", "web/src/three.ts"]);
    expect(paths(byFolder.generated)).toEqual(["bun.lock"]);

    const bySize = orderChangeFiles(files, "size");
    expect(paths(bySize.files)).toEqual(["web/src/two.ts", "web/src/three.ts", "web/src/one.ts"]);
  });
});

describe("buildFileTree", () => {
  it("gathers generated files under one group at the bottom", () => {
    const tree = buildFileTree([file("web/src/one.ts", 2)], [file("bun.lock", 400), file("web/dist/app.js", 90)], "folder");

    expect(tree.map((node) => node.name)).toEqual(["web", "Generated"]);
    const generated = tree[1];
    if (generated.kind !== "dir") throw new Error("expected the generated group to be a directory");
    expect(generated.children.map((child) => child.name)).toEqual(["app.js", "bun.lock"]);
  });
});
