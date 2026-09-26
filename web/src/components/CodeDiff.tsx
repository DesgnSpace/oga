import * as React from "react";

const CodeDiffView = React.lazy(() => import("./CodeDiffView"));

export interface CodeDiffProps {
  /** One file's unified patch, headed by its `---` and `+++` lines. */
  patch: string;
  /** Set when the patch's hunk headers carry the file's real line numbers. */
  numbered?: boolean;
  /** Long lines fold instead of scrolling sideways. */
  wrap?: boolean;
}

export function CodeDiff(props: CodeDiffProps) {
  return (
    <React.Suspense fallback={null}>
      <CodeDiffView {...props} />
    </React.Suspense>
  );
}
