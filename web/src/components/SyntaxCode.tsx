import * as React from "react";
import {
  highlight,
  limitHighlightedSource,
  resolveCodeLanguage,
  type CodeLanguage,
  type CodeToken,
} from "@/domain/review";
import type { DiffRange } from "@/lib/inline-diff";

type DiffKind = "added" | "removed";

interface HighlightedSource {
  source: string;
  language: CodeLanguage;
  tokens: CodeToken[];
}

type IdleWindow = Window & {
  requestIdleCallback?: (callback: () => void, options?: { timeout: number }) => number;
  cancelIdleCallback?: (handle: number) => void;
};

function scheduleHighlight(work: () => void): () => void {
  const browserWindow: IdleWindow | undefined = globalThis.window;
  if (browserWindow?.requestIdleCallback) {
    const handle = browserWindow.requestIdleCallback(work, { timeout: 200 });
    return () => browserWindow.cancelIdleCallback?.(handle);
  }
  const handle = globalThis.setTimeout(work, 0);
  return () => globalThis.clearTimeout(handle);
}

function TokenList({ tokens }: { tokens: CodeToken[] }) {
  return (
    <>
      {tokens.map((token, index) => (
        <span
          className={["review-token", token.className, token.changed ? "review-token-changed" : undefined]
            .filter(Boolean)
            .join(" ")}
          key={index}
        >
          {token.text}
        </span>
      ))}
    </>
  );
}

function markTokens(tokens: CodeToken[], range: DiffRange): CodeToken[] {
  const marked: CodeToken[] = [];
  let offset = 0;
  for (const token of tokens) {
    const start = offset;
    const end = start + token.text.length;
    if (end <= range.start || start >= range.end) marked.push(token);
    else {
      if (start < range.start) marked.push({ ...token, text: token.text.slice(0, range.start - start) });
      marked.push({ ...token, text: token.text.slice(Math.max(0, range.start - start), range.end - start), changed: true });
      if (end > range.end) marked.push({ ...token, text: token.text.slice(range.end - start) });
    }
    offset = end;
  }
  return marked;
}

export function SyntaxTokens({
  source,
  language,
  path,
  diffKind,
  changedRange,
}: {
  source: string;
  language?: CodeLanguage;
  path?: string;
  diffKind?: DiffKind;
  changedRange?: DiffRange;
}) {
  const resolvedLanguage = resolveCodeLanguage(language, path);
  const [highlighted, setHighlighted] = React.useState<HighlightedSource | null>(null);

  React.useEffect(() => {
    setHighlighted(null);
    if (resolvedLanguage === "plain") return;

    let active = true;
    const cancel = scheduleHighlight(() => {
      const tokens = highlight(source, resolvedLanguage, path);
      if (active) {
        React.startTransition(() => setHighlighted({ source, language: resolvedLanguage, tokens }));
      }
    });
    return () => {
      active = false;
      cancel();
    };
  }, [path, resolvedLanguage, source]);

  const ready = highlighted?.source === source && highlighted.language === resolvedLanguage;
  const children = ready && highlighted
    ? <TokenList tokens={changedRange ? markTokens(highlighted.tokens, changedRange) : highlighted.tokens} />
    : limitHighlightedSource(source);
  if (!diffKind) return children;
  return <span className={`syntax-piece syntax-piece-${diffKind}`}>{children}</span>;
}

export function SyntaxCode({
  source,
  language,
  path,
  className,
  diffKind,
  changedRange,
  inline = false,
}: {
  source: string;
  language?: CodeLanguage;
  path?: string;
  className?: string;
  diffKind?: DiffKind;
  changedRange?: DiffRange;
  inline?: boolean;
}) {
  const resolvedLanguage = resolveCodeLanguage(language, path);
  const hasAttributes = !inline || resolvedLanguage !== "plain" || className !== undefined || diffKind !== undefined;
  const classes = [
    hasAttributes ? "syntax-code" : undefined,
    inline && hasAttributes ? "syntax-code-inline" : undefined,
    diffKind ? `syntax-code-${diffKind}` : undefined,
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <code
      className={classes || undefined}
      data-language={!inline ? resolvedLanguage : undefined}
    >
      <SyntaxTokens source={source} language={resolvedLanguage} path={path} diffKind={diffKind} changedRange={changedRange} />
    </code>
  );
}
