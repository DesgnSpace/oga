// One file's unified patch, which is what the diff viewer parses. Both change
// sources arrive as something less: git hands over a bare hunk body, and the
// event-derived view only has the bounded block model.

import type { DiffKind, DiffLine } from "@/domain/changes";

type PatchLine = DiffLine & { kind: Exclude<DiffKind, "skipped"> };

const MARKERS = {
  context: " ",
  added: "+",
  removed: "-",
} satisfies Record<PatchLine["kind"], string>;

/** Only feeds language detection: every surface draws its own file header. */
const UNNAMED = "changes";

function header(path: string): string {
  return `--- ${path}\n+++ ${path}\n`;
}

/** Names the file a bare sequence of hunks belongs to. */
export function patchFromBody(path: string, body: string): string {
  return header(path) + body;
}

/**
 * Writes the block model back out as a patch. Blocks carry no line numbers, so
 * every hunk claims to start at line one and the viewer keeps its gutter
 * hidden; a skipped marker closes the hunk it sits in.
 */
export function patchFromBlocks(path: string | undefined, blocks: DiffLine[][]): string | undefined {
  const hunks: string[] = [];
  for (const block of blocks) {
    let lines: PatchLine[] = [];
    for (const line of block) {
      if (line.kind === "skipped") {
        pushHunk(hunks, lines);
        lines = [];
        continue;
      }
      lines.push({ kind: line.kind, text: line.text });
    }
    pushHunk(hunks, lines);
  }
  return hunks.length > 0 ? header(path ?? UNNAMED) + hunks.join("") : undefined;
}

function pushHunk(hunks: string[], lines: PatchLine[]): void {
  if (lines.length === 0) return;
  const removed = lines.filter((line) => line.kind !== "added").length;
  const added = lines.filter((line) => line.kind !== "removed").length;
  const body = lines.map((line) => `${MARKERS[line.kind]}${line.text}`);
  hunks.push(`@@ -1,${removed} +1,${added} @@\n${body.join("\n")}\n`);
}
