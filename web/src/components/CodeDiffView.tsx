// The diff renderer and its highlighter, loaded only once a diff is shown.

import * as React from "react";
import { processFile, registerCustomTheme, type FileDiffOptions, type ThemeRegistration } from "@pierre/diffs";
import { FileDiff } from "@pierre/diffs/react";
import { useDiffView } from "@/state/diff-preferences";
import type { CodeDiffProps } from "./CodeDiff";

const THEME_NAME = "oga";

const THEME: ThemeRegistration = {
  name: THEME_NAME,
  colors: {
    "editor.foreground": "var(--syntax-text)",
    "editor.background": "var(--syntax-bg)",
    "gitDecoration.addedResourceForeground": "var(--syntax-added)",
    "gitDecoration.deletedResourceForeground": "var(--syntax-removed)",
    "gitDecoration.modifiedResourceForeground": "var(--syntax-changed)",
  },
  settings: [
    {
      scope: ["comment", "punctuation.definition.comment"],
      settings: { foreground: "var(--syntax-comment)", fontStyle: "italic" },
    },
    {
      scope: ["string", "string.regexp", "constant.other.symbol", "meta.attribute-selector"],
      settings: { foreground: "var(--syntax-string)" },
    },
    {
      scope: ["constant.numeric", "constant.language", "constant.character", "support.constant"],
      settings: { foreground: "var(--syntax-constant)" },
    },
    {
      scope: ["keyword", "storage", "keyword.operator", "variable.language"],
      settings: { foreground: "var(--syntax-keyword)", fontStyle: "bold" },
    },
    {
      scope: ["entity.name.function", "support.function", "meta.function-call.generic"],
      settings: { foreground: "var(--syntax-function)", fontStyle: "bold" },
    },
    {
      scope: [
        "entity.name.type",
        "entity.name.class",
        "support.type",
        "support.class",
        "entity.other.attribute-name",
        "variable.other.property",
        "meta.property-name",
        "support.type.property-name",
      ],
      settings: { foreground: "var(--syntax-type)" },
    },
    { scope: ["punctuation", "meta.brace"], settings: { foreground: "var(--syntax-punctuation)" } },
    { scope: ["entity.name.tag", "entity.name.selector", "meta.selector"], settings: { foreground: "var(--syntax-tag)" } },
  ],
};

registerCustomTheme(THEME_NAME, () => Promise.resolve(THEME));

const THEMES = { light: THEME_NAME, dark: THEME_NAME };

// Reaches inside the diff's shadow root. Each rule falls back to the viewer's
// own value, so only a place that sets the `--diff-*` settings looks different.
const DIFF_CSS = `
  [data-line], [data-no-newline] {
    --mix-light: var(--diff-line-mix, 88%);
    --mix-dark: var(--diff-line-mix, 80%);
  }
  [data-gutter-buffer], [data-column-number] {
    --mix-light: var(--diff-gutter-mix, 91%);
    --mix-dark: var(--diff-gutter-mix, 85%);
  }
  [data-column-number] {
    padding-left: var(--diff-gutter-padding, 2ch);
  }
  [data-column-number][data-line-type="change-addition"] {
    box-shadow: inset var(--diff-marker-width, 0) 0 var(--diffs-addition-base);
  }
  [data-column-number][data-line-type="change-deletion"] {
    box-shadow: inset var(--diff-marker-width, 0) 0 var(--diffs-deletion-base);
  }
  [data-overflow="wrap"] [data-line] {
    padding-inline-start: calc(2ch + var(--diff-wrap-indent, 0ch));
    text-indent: calc(var(--diff-wrap-indent, 0ch) * -1);
  }
  [data-unmodified-lines] {
    font-family: var(--font-sans);
    font-size: var(--text-md);
  }
`;

export default function CodeDiffView({ patch, numbered = false, wrap = false }: CodeDiffProps) {
  const [view] = useDiffView();
  const fileDiff = React.useMemo(() => processFile(patch), [patch]);
  const options = React.useMemo(
    (): FileDiffOptions<undefined> => ({
      theme: THEMES,
      diffStyle: view,
      overflow: wrap ? "wrap" : "scroll",
      diffIndicators: "classic",
      // Only a patch with real line numbers knows how many lines sit between
      // two hunks, so only that one can say "26 unmodified lines".
      hunkSeparators: numbered ? "line-info" : "simple",
      lineDiffType: "word",
      disableFileHeader: true,
      disableLineNumbers: !numbered,
      unsafeCSS: DIFF_CSS,
    }),
    [numbered, view, wrap],
  );
  if (!fileDiff) return null;
  return <FileDiff fileDiff={fileDiff} options={options} className="code-diff" />;
}
