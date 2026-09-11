// Read-only code/diff card rendering used by task detail panels.
// Ported from rust/crates/oga-ui/src/review/mod.rs — the data derivation
// (`buildCodeCard`/`buildDiffCard`/`highlight`) already lives in @/domain/review;
// this file only draws it.

import type { CodeLanguage } from "@/domain/review";
import { resolveCodeLanguage } from "@/domain/review";
import { SyntaxCode } from "@/components/SyntaxCode";

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
