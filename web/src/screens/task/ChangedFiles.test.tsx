import { afterEach, describe, expect, it } from "bun:test";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
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

/** A file with no diff text in it, for the keyboard test that opens several. */
function plainFile(path: string, added: number): ChangedFileView {
  return { path, change: { path, blocks: [] }, added, removed: 0, hiddenLines: 0, shortened: false, status: "modified" };
}

/** The row's disclosure button, not the reviewed mark that sits beside it. */
function heading(path: string): HTMLElement {
  const matching = screen
    .getAllByRole("button", { name: new RegExp(path.replaceAll(".", "\\.")) })
    .filter((button) => button.hasAttribute("aria-expanded"));
  expect(matching).toHaveLength(1);
  return matching[0];
}

/** The file the header names as the one on screen, or nothing while it is unset. */
function onScreen(): string | null {
  return document.querySelector(".changed-files-active-file")?.textContent ?? null;
}

/** Lets the panel's next frame run, which is when it settles on a file. */
async function settle(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

/** Presses a key wherever the focus already is, as a person reviewing would. */
async function press(key: string, init: KeyboardEventInit = {}): Promise<void> {
  fireEvent.keyDown(document.activeElement ?? document.body, { key, ...init });
  await settle();
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

  it("walks the files, marks them read, and leaves typing and app keys alone", async () => {
    render(<ChangedFilesPanel {...panel(changes([
      plainFile("web/src/one.ts", 2),
      plainFile("web/src/two.ts", 1),
      plainFile("web/src/three.ts", 4),
    ]))} />);
    heading("one.ts").focus();

    await press("j");
    expect(onScreen()).toBe("web/src/one.ts");
    expect(heading("one.ts").getAttribute("aria-expanded")).toBe("true");
    expect(document.activeElement).toBe(heading("one.ts"));

    await press("r");
    expect(screen.getByText("1 of 3 reviewed")).toBeTruthy();
    expect(heading("one.ts").getAttribute("aria-expanded")).toBe("false");

    await press("j");
    expect(onScreen()).toBe("web/src/two.ts");
    await press("n");
    expect(onScreen()).toBe("web/src/three.ts");

    await press("k");
    expect(onScreen()).toBe("web/src/two.ts");
    expect(heading("two.ts").getAttribute("aria-expanded")).toBe("true");

    await press("o");
    expect(heading("two.ts").getAttribute("aria-expanded")).toBe("false");
    await press("o");
    expect(heading("two.ts").getAttribute("aria-expanded")).toBe("true");

    fireEvent.scroll(document.querySelector(".changed-files-diffs")!);
    await settle();
    expect(onScreen()).toBe("web/src/two.ts");

    fireEvent.wheel(document.querySelector(".changed-files-diffs")!);
    fireEvent.scroll(document.querySelector(".changed-files-diffs")!);
    await settle();
    expect(onScreen()).toBe("web/src/one.ts");

    await press("j", { metaKey: true });
    expect(onScreen()).toBe("web/src/one.ts");

    screen.getByRole("checkbox").focus();
    await press("j");
    expect(onScreen()).toBe("web/src/one.ts");
  });
});
