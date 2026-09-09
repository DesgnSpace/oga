import * as React from "react";
import { processFile, registerCustomTheme, type FileDiffOptions, type ThemeRegistration } from "@pierre/diffs";
import { FileDiff } from "@pierre/diffs/react";
import { useDiffView } from "@/state/diff-preferences";

const THEME_NAME = "oga";

/** The app's own code colours, named rather than spelled out. */
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

/** Both schemes take the one theme: the colours behind it already switch. */
const THEMES = { light: THEME_NAME, dark: THEME_NAME };

export interface CodeDiffProps {
  /** One file's unified patch, headed by its `---` and `+++` lines. */
  patch: string;
  /** Set when the patch's hunk headers carry the file's real line numbers. */
  numbered?: boolean;
  /** Long lines fold instead of scrolling sideways. */
  wrap?: boolean;
}

/** A file's change, unified or side by side, in the app's own code colours. */
export function CodeDiff({ patch, numbered = false, wrap = false }: CodeDiffProps) {
  const [view] = useDiffView();
  const fileDiff = React.useMemo(() => processFile(patch), [patch]);
  const options = React.useMemo(
    (): FileDiffOptions<undefined> => ({
      theme: THEMES,
      diffStyle: view,
      overflow: wrap ? "wrap" : "scroll",
      diffIndicators: "classic",
      hunkSeparators: "simple",
      lineDiffType: "word",
      disableFileHeader: true,
      disableLineNumbers: !numbered,
    }),
    [numbered, view, wrap],
  );
  if (!fileDiff) return null;
  return <FileDiff fileDiff={fileDiff} options={options} className="code-diff" />;
}
