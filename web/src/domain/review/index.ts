// Read-only code/diff card data derivation used by task detail panels.
// Ported from rust/crates/oga-ui/src/review/mod.rs — keep behavior identical.
// This module only builds the data a renderer consumes: no components, no DOM.

import { refractor } from "refractor/core";
import bash from "refractor/bash";
import c from "refractor/c";
import cpp from "refractor/cpp";
import csharp from "refractor/csharp";
import css from "refractor/css";
import go from "refractor/go";
import java from "refractor/java";
import json from "refractor/json";
import kotlin from "refractor/kotlin";
import markdown from "refractor/markdown";
import markup from "refractor/markup";
import php from "refractor/php";
import python from "refractor/python";
import ruby from "refractor/ruby";
import rust from "refractor/rust";
import sql from "refractor/sql";
import swift from "refractor/swift";
import tsx from "refractor/tsx";
import typescript from "refractor/typescript";
import yaml from "refractor/yaml";
import toml from "refractor/toml";
import { codeLanguageFromPath, type CodeLanguage } from "@/domain/trace";
import type { DiffKind, DiffLine } from "@/domain/changes";

export type { CodeLanguage };
export { codeLanguageFromPath };

refractor.register(bash);
refractor.register(c);
refractor.register(cpp);
refractor.register(csharp);
refractor.register(css);
refractor.register(go);
refractor.register(java);
refractor.register(json);
refractor.register(kotlin);
refractor.register(markup);
refractor.register(markdown);
refractor.register(php);
refractor.register(python);
refractor.register(ruby);
refractor.register(rust);
refractor.register(sql);
refractor.register(swift);
refractor.register(tsx);
refractor.register(typescript);
refractor.register(toml);
refractor.register(yaml);

export interface CodeToken {
  text: string;
  className?: string;
  changed: boolean;
}

/** A card renders at most this many lines. Everything past it is reported as a
 * count rather than built, so opening one row costs a bounded amount of DOM. */
export const MAX_CARD_LINES = 200;

/** The highlighter spends at most this many spans across a whole card. Once the
 * budget is gone the remaining lines render as one plain token each, so a file
 * of long lines cannot multiply the node count past this. */
export const MAX_CARD_TOKENS = 4_000;

/** Past this a line is not read as an edit to another line, so the two are not
 * compared character by character to find the span that changed. */
const MAX_SPAN_CHARS = 400;

export interface CodeCardLine {
  lineNumber: number;
  tokens: CodeToken[];
}

export interface CodeCardData {
  path?: string;
  language: CodeLanguage;
  lines: CodeCardLine[];
  hidden: number;
}

/** A file's contents under its name, with a line-number gutter and syntax
 * highlighting. `startLine` is the file line the first rendered line holds. */
export function buildCodeCard(
  source: string,
  options: { path?: string; language?: CodeLanguage; startLine?: number; hiddenLines?: number } = {},
): CodeCardData {
  const language = resolveCodeLanguage(options.language, options.path);
  const startLine = options.startLine ?? 1;
  const hiddenLines = options.hiddenLines ?? 0;
  const budget = { remaining: MAX_CARD_TOKENS };
  const all = highlightLines(source, language, options.path);
  const shown = Math.min(all.length, MAX_CARD_LINES);
  const hidden = hiddenLines + Math.max(0, all.length - shown);
  const lines = all.slice(0, shown).map(
    (tokens, offset): CodeCardLine => ({
      lineNumber: startLine + offset,
      tokens: withinBudget(tokens, budget),
    }),
  );
  return { path: options.path, language, lines, hidden };
}

export type DiffLineKind = DiffKind;

export interface DiffCardRow {
  kind: DiffLineKind;
  tokens: CodeToken[];
}

export interface DiffCardData {
  path?: string;
  language: CodeLanguage;
  added: number;
  removed: number;
  rows: DiffCardRow[];
  hidden: number;
  /** Present only when the card can toggle to the file's full content. */
  code?: { lines: CodeCardLine[] };
}

/** A replacement shown the way the reference draws it: removed rows tinted and
 * marked, added rows beneath them, and the span that actually changed picked
 * out inside the line. `code` is the text after the change — when it is there
 * the card offers the Code/Diff toggle. */
export function buildDiffCard(
  blocks: DiffLine[][],
  options: { path?: string; language?: CodeLanguage; code?: string; hiddenLines?: number } = {},
): DiffCardData {
  const language = resolveCodeLanguage(options.language, options.path);
  const hiddenLines = options.hiddenLines ?? 0;
  const lines = blocks.flat();
  const added = lines.filter((line) => line.kind === "added").length;
  const removed = lines.filter((line) => line.kind === "removed").length;
  const hidden = hiddenLines + Math.max(0, lines.length - MAX_CARD_LINES);
  const rows = diffRows(lines, language, options.path);
  const code = options.code === undefined ? undefined : { lines: codeLines(options.code, language, options.path) };
  return { path: options.path, language, added, removed, rows, hidden, code };
}

function codeLines(source: string, language: CodeLanguage, path: string | undefined): CodeCardLine[] {
  const budget = { remaining: MAX_CARD_TOKENS };
  return highlightLines(source, language, path)
    .slice(0, MAX_CARD_LINES)
    .map((tokens, index) => ({ lineNumber: index + 1, tokens: withinBudget(tokens, budget) }));
}

/** Builds the diff body: each removed line is paired with the added line that
 * replaced it so the changed span can be marked on both. */
function diffRows(lines: DiffLine[], language: CodeLanguage, path: string | undefined): DiffCardRow[] {
  const budget = { remaining: MAX_CARD_TOKENS };
  const spans = changedSpans(lines);
  return lines.slice(0, MAX_CARD_LINES).map((line, index) => {
    if (line.kind === "skipped") {
      return { kind: line.kind, tokens: [{ text: line.text, changed: false }] };
    }
    const tokens = withinBudget(highlight(line.text, language, path), budget);
    const span = spans[index];
    return { kind: line.kind, tokens: span ? markTokens(tokens, span) : tokens };
  });
}

/** Pairs each removed line with the added line that replaced it — the runs are
 * already ordered removed-then-added — and returns the span to mark on each. */
function changedSpans(lines: DiffLine[]): (Range | undefined)[] {
  const spans: (Range | undefined)[] = [];
  spans.length = lines.length;
  spans.fill(undefined);
  let index = 0;
  while (index < lines.length) {
    if (lines[index].kind !== "removed") {
      index += 1;
      continue;
    }
    const removedStart = index;
    while (index < lines.length && lines[index].kind === "removed") index += 1;
    const addedStart = index;
    while (index < lines.length && lines[index].kind === "added") index += 1;
    const pairs = Math.min(addedStart - removedStart, index - addedStart);
    for (let offset = 0; offset < pairs; offset += 1) {
      const span = changedSpan(lines[removedStart + offset].text, lines[addedStart + offset].text);
      if (span) {
        spans[removedStart + offset] = span.before;
        spans[addedStart + offset] = span.after;
      }
    }
  }
  return spans;
}

interface Range {
  start: number;
  end: number;
}

/** The stretch that differs between a line and the line that replaced it, as
 * char offsets into each. A pair that shares almost nothing is a rewrite
 * rather than an edit, and marking all of it would say nothing. */
function changedSpan(beforeText: string, afterText: string): { before: Range; after: Range } | undefined {
  const before = Array.from(beforeText);
  const after = Array.from(afterText);
  if (before.length > MAX_SPAN_CHARS || after.length > MAX_SPAN_CHARS) return undefined;
  let prefix = 0;
  while (prefix < before.length && prefix < after.length && before[prefix] === after[prefix]) prefix += 1;
  const shortest = Math.min(before.length, after.length);
  let suffix = 0;
  while (
    suffix < shortest - prefix &&
    before[before.length - 1 - suffix] === after[after.length - 1 - suffix]
  ) {
    suffix += 1;
  }
  if (prefix === 0 && suffix === 0) return undefined;
  if ((prefix + suffix) * 3 < shortest) return undefined;
  return {
    before: { start: prefix, end: before.length - suffix },
    after: { start: prefix, end: after.length - suffix },
  };
}

/** Splits the tokens covering `span` so the changed stretch carries its own
 * class without losing the highlighting underneath it. */
function markTokens(tokens: CodeToken[], span: Range): CodeToken[] {
  if (span.start >= span.end) return tokens;
  const marked: CodeToken[] = [];
  let offset = 0;
  for (const token of tokens) {
    const characters = Array.from(token.text);
    const length = characters.length;
    const end = offset + length;
    if (end <= span.start || offset >= span.end) {
      marked.push(token);
      offset = end;
      continue;
    }
    const start = Math.max(0, span.start - offset);
    const stop = Math.min(length, span.end - offset);
    for (const [rangeStart, rangeEnd, changed] of [
      [0, start, false],
      [start, stop, true],
      [stop, length, false],
    ] as const) {
      if (rangeStart >= rangeEnd) continue;
      marked.push({ text: characters.slice(rangeStart, rangeEnd).join(""), className: token.className, changed });
    }
    offset = end;
  }
  return marked;
}

/** Highlights once and splits on newlines, so a string or comment spanning
 * several lines keeps its colour across them. */
function highlightLines(source: string, language: CodeLanguage, path: string | undefined): CodeToken[][] {
  const lines: CodeToken[][] = [[]];
  const push = (text: string, className: string | undefined) => {
    if (text === "") return;
    lines[lines.length - 1].push({ text, className, changed: false });
  };
  for (const token of highlight(source, language, path)) {
    const parts = token.text.split("\n");
    push(parts[0], token.className);
    for (let index = 1; index < parts.length; index += 1) {
      lines.push([]);
      push(parts[index], token.className);
    }
  }
  if (lines.length > 1 && lines[lines.length - 1].length === 0) lines.pop();
  return lines;
}

/** Spends the card's token budget, and once it is gone hands back the line as a
 * single plain token. */
function withinBudget(tokens: CodeToken[], budget: { remaining: number }): CodeToken[] {
  if (tokens.length <= budget.remaining) {
    budget.remaining -= tokens.length;
    return tokens;
  }
  budget.remaining = 0;
  return [{ text: tokens.map((token) => token.text).join(""), changed: false }];
}

/** Past this the trace is a wall of text nobody reads, and highlighting it
 * costs more than the whole rest of the expansion. */
export const MAX_HIGHLIGHTED_CHARS = 20_000;

/** Every token becomes its own DOM node, and the character cap alone does not
 * bound them when a grammar marks punctuation one character at a time. */
const MAX_HIGHLIGHTED_TOKENS = 8_000;

export function resolveCodeLanguage(language: CodeLanguage | undefined, path: string | undefined): CodeLanguage {
  return language !== undefined && language !== "plain" ? language : codeLanguageFromPath(path);
}

export function limitHighlightedSource(source: string): string {
  const limited = source.slice(0, MAX_HIGHLIGHTED_CHARS);
  if (limited.length >= source.length) return limited;
  const last = limited.charCodeAt(limited.length - 1);
  const safe = last >= 0xd800 && last <= 0xdbff ? limited.slice(0, -1) : limited;
  return `${safe}\n… truncated`;
}

function grammarFor(language: CodeLanguage, path: string | undefined): string | undefined {
  if (language === "plain") return undefined;
  if (language === "shell") return "bash";
  if (language === "html") return "markup";
  if (language === "tsx") return "tsx";
  if (language === "typescript" && path?.toLowerCase().endsWith(".tsx")) return "tsx";
  if (language === "javascript" && path?.toLowerCase().endsWith(".jsx")) return "tsx";
  return language;
}

function tokenClassName(classes: string[]): string | undefined {
  const names = classes.filter((name) => name !== "token").map((name) => `review-token-${name}`);
  return names.length > 0 ? names.join(" ") : undefined;
}

function addToken(tokens: CodeToken[], text: string, className: string | undefined): void {
  if (text === "") return;
  const previous = tokens[tokens.length - 1];
  if (previous && previous.className === className && !previous.changed) {
    previous.text += text;
  } else {
    tokens.push({ text, className, changed: false });
  }
}

type HighlightNode = ReturnType<typeof refractor.highlight>["children"][number];
type HighlightElement = Extract<HighlightNode, { type: "element" }>;

function classNames(value: HighlightElement["properties"]["className"]): string[] {
  return value ?? [];
}

function flattenNode(node: HighlightNode, inherited: string[], tokens: CodeToken[]): boolean {
  if (tokens.length >= MAX_HIGHLIGHTED_TOKENS) return true;
  if (node.type === "text") {
    addToken(tokens, node.value, tokenClassName(inherited));
    return false;
  }
  if (node.type !== "element") return false;
  const classes = classNames(node.properties.className);
  const next = [...new Set([...inherited, ...classes])];
  for (const child of node.children) {
    if (flattenNode(child, next, tokens)) return true;
  }
  return false;
}

interface FlattenedHighlight {
  tokens: CodeToken[];
  truncated: boolean;
}

function flattenHighlighted(children: HighlightNode[]): FlattenedHighlight {
  const tokens: CodeToken[] = [];
  for (const child of children) {
    if (flattenNode(child, [], tokens)) return { tokens, truncated: true };
  }
  return { tokens, truncated: false };
}

export function highlight(source: string, language: CodeLanguage, path?: string): CodeToken[] {
  const limited = source.slice(0, MAX_HIGHLIGHTED_CHARS);
  const sourceTruncated = limited.length < source.length;
  if (language === "plain") {
    const text = sourceTruncated ? limitHighlightedSource(source).replace(/\n… truncated$/, "") : limited;
    const tokens: CodeToken[] = [{ text, changed: false }];
    if (sourceTruncated) tokens.push({ text: "\n… truncated", className: "review-token-comment", changed: false });
    return tokens;
  }

  const grammar = grammarFor(language, path);
  if (grammar === undefined) return [{ text: limitHighlightedSource(source), changed: false }];
  const result = flattenHighlighted(refractor.highlight(limited, grammar).children);
  if (sourceTruncated || result.truncated) {
    result.tokens.push({ text: "\n… truncated", className: "review-token-comment", changed: false });
  }
  return result.tokens;
}
