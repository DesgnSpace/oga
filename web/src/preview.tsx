import { createRoot } from "react-dom/client";
import type { ChangedFileSet, ChangedFileView } from "@/domain/changes";
import { ChangedFilesFullScreen } from "@/screens/task/ChangedFiles";
import "@/oga.css";
import "./index.css";

function file(path: string, added: number, removed: number, body: string): ChangedFileView {
  return {
    path,
    change: { path, blocks: [] },
    patch: `--- a/${path}\n+++ b/${path}\n${body}`,
    added,
    removed,
    hiddenLines: 0,
    shortened: false,
    status: "modified",
  };
}

const HUNK = `@@ -12,8 +12,12 @@ export function resolve(input: string) {
   const trimmed = input.trim();
-  if (trimmed === "") return undefined;
-  return lookup(trimmed);
+  if (trimmed === "") return undefined;
+  const cached = cache.get(trimmed);
+  if (cached !== undefined) return cached;
+  const resolved = lookup(trimmed);
+  cache.set(trimmed, resolved);
+  return resolved;
 }
`;

const changes: ChangedFileSet = {
  files: [
    file("web/src/screens/task/ChangedFiles.tsx", 84, 21, HUNK),
    file("web/src/screens/task/TaskDetail.tsx", 31, 18, HUNK),
    file("web/src/components/primitives/Modal.tsx", 9, 3, HUNK),
    file("web/src/ui/icons.tsx", 22, 0, HUNK),
    file("web/src/oga.css", 51, 1, HUNK),
    file("rust/crates/oga-core/src/store/task.rs", 14, 6, HUNK),
    file("rust/crates/oga-cli/src/commands/watch.rs", 7, 7, HUNK),
    file("CHANGELOG.md", 1, 0, HUNK),
  ],
  unmatched: 0,
};

document.documentElement.classList.add("platform-macos");

createRoot(document.getElementById("root")!).render(
  <ChangedFilesFullScreen
    source="branch"
    base="main"
    onBaseChange={() => {}}
    branches={["main", "oga/changed-files-picker"]}
    onSourceChange={() => {}}
    groupByTurn={false}
    onGroupByTurn={() => {}}
    onReload={() => {}}
    changes={changes}
    loading={false}
    live={false}
    hasEarlier={false}
    loadingEarlier={false}
    onLoadEarlier={() => {}}
    onClose={() => {}}
  />,
);
