import * as React from "react";
import {
  limitHighlightedSource,
  resolveCodeLanguage,
  type CodeLanguage,
  type CodeToken,
} from "@/domain/review/core";

interface HighlightedSource {
  source: string;
  language: CodeLanguage;
  tokens: CodeToken[];
}

type IdleWindow = Window & {
  requestIdleCallback?: (callback: () => void, options?: { timeout: number }) => number;
  cancelIdleCallback?: (handle: number) => void;
};

type HighlighterModule = typeof import("@/domain/review");
let highlighterModule: Promise<HighlighterModule> | undefined;

function loadHighlighter(): Promise<HighlighterModule> {
  return (highlighterModule ??= import("@/domain/review"));
}

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

export function SyntaxTokens({
  source,
  language,
  path,
}: {
  source: string;
  language?: CodeLanguage;
  path?: string;
}) {
  const resolvedLanguage = resolveCodeLanguage(language, path);
  const [highlighted, setHighlighted] = React.useState<HighlightedSource | null>(null);

  React.useEffect(() => {
    setHighlighted(null);
    if (resolvedLanguage === "plain") return;

    let active = true;
    const cancel = scheduleHighlight(() => {
      void loadHighlighter().then(({ highlight }) => {
        if (!active) return;
        const tokens = highlight(source, resolvedLanguage, path);
        React.startTransition(() => setHighlighted({ source, language: resolvedLanguage, tokens }));
      });
    });
    return () => {
      active = false;
      cancel();
    };
  }, [path, resolvedLanguage, source]);

  const ready = highlighted?.source === source && highlighted.language === resolvedLanguage;
  return ready && highlighted ? <TokenList tokens={highlighted.tokens} /> : limitHighlightedSource(source);
}

export function SyntaxCode({
  source,
  language,
  path,
  className,
  inline = false,
}: {
  source: string;
  language?: CodeLanguage;
  path?: string;
  className?: string;
  inline?: boolean;
}) {
  const resolvedLanguage = resolveCodeLanguage(language, path);
  const hasAttributes = !inline || resolvedLanguage !== "plain" || className !== undefined;
  const classes = [
    hasAttributes ? "syntax-code" : undefined,
    inline && hasAttributes ? "syntax-code-inline" : undefined,
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <code
      className={classes || undefined}
      data-language={!inline ? resolvedLanguage : undefined}
    >
      <SyntaxTokens source={source} language={resolvedLanguage} path={path} />
    </code>
  );
}
