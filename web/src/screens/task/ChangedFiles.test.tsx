import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ChangedFileSet, ChangedFileView } from "@/domain/changes";
import { resetReviewedFilesForTests } from "@/state/reviewed-files";
import { ChangedFilesPanel, type ChangedFilesPanelProps } from "./ChangedFiles";

function file(path: string, added: number, removed: number, body: string): ChangedFileView {
  return {
    path,
    change: { path, blocks: [] },
    patch: `--- a/${path}\n+++ b/${path}\n@@ -1,3 +1,3 @@\n${body}`,
    added,
    removed,
    hiddenLines: 0,
    shortened: false,
    status: "modified",
  };
}

function changes(files: ChangedFileView[]): ChangedFileSet {
  return { files, unmatched: 0 };
}

function panel(changes: ChangedFileSet): ChangedFilesPanelProps {
  return {
    taskId: "task-1",
    source: "run",
    onSourceChange: () => {},
    onBaseChange: () => {},
    branches: [],
    groupByTurn: false,
    onGroupByTurn: () => {},
    onReload: () => {},
    changes,
    loading: false,
    live: false,
    hasEarlier: false,
    loadingEarlier: false,
    onLoadEarlier: () => {},
    onClose: () => {},
    onExpand: () => {},
    width: 380,
    onResizeStart: () => {},
    onResetWidth: () => {},
    onResizeStep: () => {},
  };
}

const firstFile = () => file("web/src/one.ts", 2, 1, "-const answer = 41;\n+const answer = 42;\n unchanged");
const secondFile = () => file("web/src/two.ts", 1, 0, "+export const two = 2;");

/** The row's disclosure button, not the reviewed mark that sits beside it. */
function heading(path: string): HTMLElement {
  const matching = screen
    .getAllByRole("button", { name: new RegExp(path.replaceAll(".", "\\.")) })
    .filter((button) => button.hasAttribute("aria-expanded"));
  expect(matching).toHaveLength(1);
  return matching[0];
}

describe("reviewing changed files", () => {
  afterEach(() => {
    cleanup();
    resetReviewedFilesForTests();
  });

  it("closes a file once reviewed and keeps the mark after the panel reopens", () => {
    const { unmount } = render(<ChangedFilesPanel {...panel(changes([firstFile(), secondFile()]))} />);

    fireEvent.click(heading("one.ts"));
    expect(heading("one.ts").getAttribute("aria-expanded")).toBe("true");

    fireEvent.click(screen.getByRole("button", { name: "Mark web/src/one.ts reviewed" }));

    expect(heading("one.ts").getAttribute("aria-expanded")).toBe("false");
    expect(screen.getByText("1 of 2 reviewed")).toBeTruthy();

    unmount();
    render(<ChangedFilesPanel {...panel(changes([firstFile(), secondFile()]))} />);

    expect(screen.getByRole("button", { name: "Mark web/src/one.ts reviewed" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "Mark web/src/two.ts reviewed" }).getAttribute("aria-pressed")).toBe("false");
    expect(screen.getByText("1 of 2 reviewed")).toBeTruthy();
  });

  it("drops the mark when the worker edits the file again", () => {
    const { rerender } = render(<ChangedFilesPanel {...panel(changes([firstFile(), secondFile()]))} />);
    fireEvent.click(screen.getByRole("button", { name: "Mark web/src/one.ts reviewed" }));
    expect(screen.getByText("1 of 2 reviewed")).toBeTruthy();

    const edited = { ...firstFile(), patch: `${firstFile().patch}\n@@ -8,2 +8,2 @@\n-old tail\n+new tail`, added: 3 };
    rerender(<ChangedFilesPanel {...panel(changes([edited, secondFile()]))} />);

    expect(screen.getByRole("button", { name: "Mark web/src/one.ts reviewed" }).getAttribute("aria-pressed")).toBe("false");
    expect(screen.getByText("0 of 2 reviewed")).toBeTruthy();
  });
});
