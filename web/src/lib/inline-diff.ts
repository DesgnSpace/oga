import type { DiffLine } from "@/domain/changes";

export interface DiffRange {
  start: number;
  end: number;
}

export interface InlineDiffRanges {
  before: DiffRange;
  after: DiffRange;
}

export interface SplitDiffRow {
  before?: DiffLine;
  after?: DiffLine;
}

interface Token {
  text: string;
  start: number;
  end: number;
}

function lcsLength(before: Token[], after: Token[]): number {
  const table = Array.from({ length: before.length + 1 }, () => new Uint16Array(after.length + 1));
  for (let row = before.length - 1; row >= 0; row -= 1) {
    for (let column = after.length - 1; column >= 0; column -= 1) {
      table[row][column] = before[row].text === after[column].text
        ? table[row + 1][column + 1] + 1
        : Math.max(table[row + 1][column], table[row][column + 1]);
    }
  }
  return table[0][0];
}

function tokenize(source: string): Token[] {
  const tokens: Token[] = [];
  const pattern = /\p{L}[\p{L}\p{N}_]*|\p{N}[\p{L}\p{N}_]*|\s+|[^\p{L}\p{N}_\s]/gu;
  for (const match of source.matchAll(pattern)) {
    const text = match[0];
    const start = match.index ?? 0;
    tokens.push({ text, start, end: start + text.length });
  }
  return tokens;
}

function changedRange(beforeText: string, afterText: string): InlineDiffRanges | undefined {
  const before = tokenize(beforeText);
  const after = tokenize(afterText);
  const contentBefore = before.filter((token) => !/^\s+$/u.test(token.text));
  const contentAfter = after.filter((token) => !/^\s+$/u.test(token.text));
  const shortestContent = Math.min(contentBefore.length, contentAfter.length);
  if (shortestContent > 0 && lcsLength(contentBefore, contentAfter) * 3 <= shortestContent) return undefined;

  let prefix = 0;
  while (prefix < before.length && prefix < after.length && before[prefix].text === after[prefix].text) prefix += 1;
  let suffix = 0;
  while (
    suffix < before.length - prefix &&
    suffix < after.length - prefix &&
    before[before.length - suffix - 1].text === after[after.length - suffix - 1].text
  ) suffix += 1;
  return {
    before: { start: prefix === 0 ? 0 : before[prefix - 1].end, end: before[before.length - suffix]?.start ?? beforeText.length },
    after: { start: prefix === 0 ? 0 : after[prefix - 1].end, end: after[after.length - suffix]?.start ?? afterText.length },
  };
}

export function buildInlineDiffRanges(blocks: DiffLine[][]): (InlineDiffRanges | undefined)[][] {
  return blocks.map((block) => {
    const ranges: (InlineDiffRanges | undefined)[] = Array.from({ length: block.length });
    let index = 0;
    while (index < block.length) {
      if (block[index].kind !== "removed") {
        index += 1;
        continue;
      }
      const removedStart = index;
      while (index < block.length && block[index].kind === "removed") index += 1;
      const addedStart = index;
      while (index < block.length && block[index].kind === "added") index += 1;
      const pairs = Math.min(addedStart - removedStart, index - addedStart);
      for (let offset = 0; offset < pairs; offset += 1) {
        const range = changedRange(
          block[removedStart + offset].text,
          block[addedStart + offset].text,
        );
        ranges[removedStart + offset] = range;
        ranges[addedStart + offset] = range;
      }
    }
    return ranges;
  });
}

export function splitDiffBlock(block: DiffLine[]): SplitDiffRow[] {
  const rows: SplitDiffRow[] = [];
  let index = 0;
  while (index < block.length) {
    if (block[index].kind === "removed") {
      const removedStart = index;
      while (index < block.length && block[index].kind === "removed") index += 1;
      const addedStart = index;
      while (index < block.length && block[index].kind === "added") index += 1;
      const removed = block.slice(removedStart, addedStart);
      const added = block.slice(addedStart, index);
      for (let offset = 0; offset < Math.max(removed.length, added.length); offset += 1) {
        rows.push({ before: removed[offset], after: added[offset] });
      }
    } else {
      rows.push({ before: block[index], after: block[index] });
      index += 1;
    }
  }
  return rows;
}
