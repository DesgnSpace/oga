// Read-only code/diff card rendering used by task detail panels.
// Ported from rust/crates/oga-ui/src/review/mod.rs — the data derivation
// (`buildCodeCard`/`buildDiffCard`/`highlight`) already lives in @/domain/review;
// this file only draws it.

import * as React from "react";
import type { CodeLanguage, CodeToken } from "@/domain/review";
import { buildCodeCard, buildDiffCard, resolveCodeLanguage } from "@/domain/review";
import type { DiffLine } from "@/domain/changes";
import { SyntaxCode } from "@/components/SyntaxCode";
import { CodeGlyph } from "./CodeGlyph";

function TokenSpan({ token, index }: { token: CodeToken; index: number }) {
  const className = ["review-token", token.className, token.changed ? "review-token-changed" : undefined]
    .filter(Boolean)
    .join(" ");
  return (
    <span key={index} className={className || undefined}>
      {token.text}
    </span>
  );
}

/** Plain highlighted content, no gutter or header — used for prose/detail/command/payload expansions. */
export function ReviewContent({
  source,
  language,
  path,
}: {
  source: string;
  language?: CodeLanguage;
  path?: string;
}) {
  const resolvedLanguage = resolveCodeLanguage(language, path);
  return (
    <div className={`review-content review-language-${resolvedLanguage}`}>
      <pre data-language={resolvedLanguage}>
        <SyntaxCode source={source} language={resolvedLanguage} path={path} />
      </pre>
    </div>
  );
}

function CardHeader({
  path,
  added,
  removed,
  counts,
}: {
  path?: string;
  added: number;
  removed: number;
  counts: boolean;
}) {
  const name = path ?? "Source";
  return (
    <div className="code-card-header">
      <CodeGlyph />
      <span className="code-card-name" title={name}>
        {name}
      </span>
      {counts && (
        <span className="code-card-counts">
          <span className="code-card-added">{`+${added}`}</span>
          {removed > 0 && <span className="code-card-removed">{`-${removed}`}</span>}
        </span>
      )}
    </div>
  );
}

function CardOverflow({ hidden }: { hidden: number }) {
  if (hidden <= 0) return null;
  return <p className="code-card-note">{`${hidden} more line${hidden === 1 ? "" : "s"} not shown`}</p>;
}

/** A file's contents under its name, with a line-number gutter and syntax highlighting. */
export function CodeCard({
  source,
  path,
  language,
  startLine = 1,
  hiddenLines = 0,
}: {
  source: string;
  path?: string;
  language?: CodeLanguage;
  startLine?: number;
  hiddenLines?: number;
}) {
  const card = React.useMemo(
    () => buildCodeCard(source, { path, language, startLine, hiddenLines }),
    [source, path, language, startLine, hiddenLines],
  );
  return (
    <div className={`code-card review-language-${card.language}`}>
      <CardHeader path={path} added={0} removed={0} counts={false} />
      <div className="code-card-body">
        {card.lines.map((line) => (
          <div className="code-line" key={line.lineNumber}>
            <span className="code-line-number" aria-hidden="true">
              {line.lineNumber}
            </span>
            <code className="code-line-text">
              {line.tokens.map((token, index) => (
                <TokenSpan key={index} token={token} index={index} />
              ))}
            </code>
          </div>
        ))}
      </div>
      <CardOverflow hidden={card.hidden} />
    </div>
  );
}

/**
 * A replacement shown the way the reference draws it: removed rows tinted and
 * marked, added rows beneath them, and the span that actually changed picked
 * out inside the line. `code` is the text after the change — when it is
 * there the card offers the Code/Diff toggle.
 */
export function DiffCard({
  blocks,
  path,
  language,
  code,
  hiddenLines = 0,
}: {
  blocks: DiffLine[][];
  path?: string;
  language?: CodeLanguage;
  code?: string;
  hiddenLines?: number;
}) {
  const [showingCode, setShowingCode] = React.useState(false);
  const card = React.useMemo(
    () => buildDiffCard(blocks, { path, language, code, hiddenLines }),
    [blocks, path, language, code, hiddenLines],
  );
  const codeLines = card.code?.lines;

  return (
    <div className={`code-card review-language-${card.language}`}>
      <CardHeader path={path} added={card.added} removed={card.removed} counts={true} />
      <div className="code-card-body">
        {showingCode && codeLines
          ? codeLines.map((line) => (
              <div className="code-line" key={line.lineNumber}>
                <code className="code-line-text">
                  {line.tokens.map((token, index) => (
                    <TokenSpan key={index} token={token} index={index} />
                  ))}
                </code>
              </div>
            ))
          : card.rows.map((row, index) =>
              row.kind === "skipped" ? (
                <div className="code-line code-line-skipped" key={index}>
                  <span className="code-line-marker" aria-hidden="true" />
                  <span className="code-line-text">{row.tokens.map((t) => t.text).join("")}</span>
                </div>
              ) : (
                <div className={`code-line code-line-${row.kind}`} key={index}>
                  <span className="code-line-marker" aria-hidden="true" />
                  <code className="code-line-text">
                    {row.tokens.map((token, tokenIndex) => (
                      <TokenSpan key={tokenIndex} token={token} index={tokenIndex} />
                    ))}
                  </code>
                </div>
              ),
            )}
      </div>
      <CardOverflow hidden={card.hidden} />
      {card.code !== undefined && (
        <div className="code-card-toggle" role="group" aria-label="View">
          <button
            className={`code-card-tab${showingCode ? " code-card-tab-on" : ""}`}
            type="button"
            aria-pressed={showingCode}
            onClick={() => setShowingCode(true)}
          >
            Code
          </button>
          <button
            className={`code-card-tab${!showingCode ? " code-card-tab-on" : ""}`}
            type="button"
            aria-pressed={!showingCode}
            onClick={() => setShowingCode(false)}
          >
            Diff
          </button>
        </div>
      )}
    </div>
  );
}
