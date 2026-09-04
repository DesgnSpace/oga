import { useState } from "react";
import { afterEach, describe, expect, it } from "bun:test";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { Modal } from "./Modal";

afterEach(() => {
  cleanup();
  document.body.style.overflow = "";
});

function Harness() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button onClick={() => setOpen(true)}>Open settings</button>
      <Modal open={open} onClose={() => setOpen(false)} labelledBy="modal-title">
        <h1 id="modal-title">Settings</h1>
        <button>Inside modal</button>
      </Modal>
    </>
  );
}

describe("Modal", () => {
  it("opens as a labelled dialog and locks page scrolling", () => {
    render(<Harness />);
    expect(screen.queryByRole("dialog")).toBeNull();

    fireEvent.click(screen.getByText("Open settings"));

    const dialog = screen.getByRole("dialog");
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(dialog.getAttribute("aria-labelledby")).toBe("modal-title");
    expect(document.body.style.overflow).toBe("hidden");
  });

  it("keeps the app's existing scroll state when it is already locked", () => {
    document.body.style.overflow = "hidden";
    render(<Harness />);

    fireEvent.click(screen.getByText("Open settings"));
    fireEvent.click(screen.getByRole("button", { name: "Close dialog" }));

    expect(document.body.style.overflow).toBe("hidden");
  });

  it("closes on Escape and returns focus to the opener", () => {
    render(<Harness />);
    const opener = screen.getByText("Open settings");
    opener.focus();
    fireEvent.click(opener);
    expect(screen.getByRole("dialog")).toBeTruthy();

    fireEvent.keyDown(document, { key: "Escape" });

    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(opener);
    expect(document.body.style.overflow).toBe("");
  });

  it("closes on an outside click but not on a click inside the dialog", () => {
    render(<Harness />);
    fireEvent.click(screen.getByText("Open settings"));
    expect(screen.getByRole("dialog")).toBeTruthy();

    fireEvent.mouseDown(screen.getByText("Inside modal"));
    expect(screen.getByRole("dialog")).toBeTruthy();

    fireEvent.mouseDown(document.querySelector(".modal-overlay")!);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("closes on the explicit close control", () => {
    render(<Harness />);
    fireEvent.click(screen.getByText("Open settings"));

    fireEvent.click(screen.getByRole("button", { name: "Close dialog" }));

    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
