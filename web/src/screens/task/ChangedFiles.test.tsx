import { afterEach, describe, expect, it, mock } from "bun:test";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { setTransport, tauriTransport } from "@/bridge/transport";
import type { ChangedFileSet, ChangedFileView } from "@/domain/changes";
import { resetReviewedFilesForTests } from "@/state/reviewed-files";
import { toast } from "@/state/toast";
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
    sort: "folder",
    onSortChange: () => {},
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

/** The files the panel lists, top to bottom, wherever they sit. */
function rowPaths(): (string | null)[] {
  return [...document.querySelectorAll(".changed-file-path")].map((row) => row.textContent);
}

/** jsdom has no desktop shell, so a test that needs a call sets this. */
function setDesktopBridge(present: boolean): void {
  Reflect.set(window, "__TAURI__", present ? {} : undefined);
}

const originalClipboard = navigator.clipboard;

afterEach(() => {
  act(() => toast.clear());
  cleanup();
  setDesktopBridge(false);
  setTransport(tauriTransport);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: originalClipboard });
  resetReviewedFilesForTests();
});

describe("reviewing changed files", () => {
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

  it("puts the biggest change first on request and keeps generated files in a closed group", async () => {
    const small = plainFile("web/src/one.ts", 2);
    const large = plainFile("web/src/two.ts", 90);
    const lock = plainFile("bun.lock", 400);
    const set = changes([small, lock, large]);

    const { rerender } = render(<ChangedFilesPanel {...panel(set)} sort="folder" />);

    expect(rowPaths()).toEqual(["web/src/one.ts", "web/src/two.ts"]);
    expect(screen.getByText("1 generated file, not part of the review")).toBeTruthy();
    const group = screen.getByRole("button", { name: /Generated/ });
    expect(group.getAttribute("aria-expanded")).toBe("false");

    rerender(<ChangedFilesPanel {...panel(set)} sort="size" />);
    expect(rowPaths()).toEqual(["web/src/two.ts", "web/src/one.ts"]);

    heading("one.ts").focus();
    await press("j");
    expect(onScreen()).toBe("web/src/two.ts");
    await press("j");
    expect(onScreen()).toBe("web/src/one.ts");

    fireEvent.click(screen.getByRole("button", { name: "Mark web/src/two.ts reviewed" }));
    fireEvent.click(screen.getByRole("button", { name: "Mark web/src/one.ts reviewed" }));
    expect(screen.getByText("2 of 2 reviewed")).toBeTruthy();

    fireEvent.click(group);
    expect(rowPaths()).toEqual(["web/src/two.ts", "web/src/one.ts", "bun.lock"]);
    expect(screen.queryByRole("button", { name: "Mark bun.lock reviewed" })).toBeNull();
  });
});

describe("a file's own actions", () => {
  /** How long the copy button holds its tick before going back to offering a copy. */
  const FLASH_MS = 1_600;

  it("takes the path as the diff names it, then puts the button back", async () => {
    const copied: string[] = [];
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: async (value: string) => void copied.push(value) },
    });
    render(<ChangedFilesPanel {...panel(changes([firstFile(), secondFile()]))} />);

    fireEvent.click(screen.getByRole("button", { name: "Copy path to web/src/one.ts" }));

    // The row shows the path from its middle, so a copy built from what is on
    // screen would come back truncated or reversed.
    expect(copied).toEqual(["web/src/one.ts"]);
    await settle();
    screen.getByRole("button", { name: "Copied" });

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, FLASH_MS));
    });
    screen.getByRole("button", { name: "Copy path to web/src/one.ts" });
  });

  it("reports a failed hand-off, without opening the diff behind it", async () => {
    setDesktopBridge(true);
    // SAFETY: this call only ever answers with a failure, which the panel reads
    // as one whatever shape it carries.
    setTransport({
      invoke: mock(async () => {
        throw { message: "not found" };
      }) as never,
      listen: mock(),
    });

    render(<ChangedFilesPanel {...panel(changes([firstFile(), secondFile()]))} />);
    fireEvent.click(screen.getByRole("button", { name: "Open web/src/two.ts in your editor" }));
    await settle();

    // The error reaches the reader only if the press reached the broker, and
    // the row's own disclosure stays where they left it.
    expect(toast.snapshot.map((record) => record.title)).toEqual([
      "Couldn't open web/src/two.ts in your editor",
    ]);
    expect(heading("two.ts").getAttribute("aria-expanded")).toBe("false");
  });

  it("says nothing when the file opens", async () => {
    setDesktopBridge(true);
    const invoke = mock(async () => undefined);
    // SAFETY: the only call this test makes is a broker_call whose answer it never reads.
    setTransport({ invoke: invoke as never, listen: mock() });

    render(<ChangedFilesPanel {...panel(changes([firstFile(), secondFile()]))} />);
    fireEvent.click(screen.getByRole("button", { name: "Open web/src/two.ts in your editor" }));
    await settle();

    expect(toast.snapshot).toEqual([]);
  });

  it("keeps a deleted file's path copyable, with no editor to send it to", () => {
    setDesktopBridge(true);
    const deleted: ChangedFileView = { ...firstFile(), path: "web/src/old.ts", status: "deleted" };
    render(<ChangedFilesPanel {...panel(changes([firstFile(), deleted]))} />);

    expect(screen.queryByRole("button", { name: "Open web/src/old.ts in your editor" })).toBeNull();
    screen.getByRole("button", { name: "Copy path to web/src/old.ts" });
  });

  it("offers no editor at all where there is no desktop app to ask", () => {
    render(<ChangedFilesPanel {...panel(changes([firstFile()]))} />);

    expect(screen.queryByRole("button", { name: "Open web/src/one.ts in your editor" })).toBeNull();
    screen.getByRole("button", { name: "Copy path to web/src/one.ts" });
  });
});
