import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "bun:test";
import { ToastViewport } from "./ToastViewport";
import { Modal } from "@/components/primitives/Modal";
import { toast } from "@/state/toast";

afterEach(() => {
  act(() => toast.clear());
  cleanup();
});

describe("ToastViewport", () => {
  it("renders a collapsed stack, expands on hover, and dismisses a toast", () => {
    render(<ToastViewport />);
    act(() => {
      for (let index = 1; index <= 5; index += 1) toast.error(`Toast ${index}`);
    });

    const viewport = screen.getByRole("region", { name: "Notifications" });
    expect(viewport.querySelectorAll(".toast-item")).toHaveLength(5);
    expect(viewport.querySelectorAll(".toast-item-hidden")).toHaveLength(2);

    fireEvent.mouseEnter(viewport);
    expect(viewport.querySelectorAll(".toast-item-hidden")).toHaveLength(0);

    const newest = viewport.querySelector<HTMLElement>('[data-toast-id="5"]')!;
    fireEvent.click(newest.querySelector(".toast-close")!);

    expect(newest.classList.contains("toast-card-exiting")).toBe(true);
  });

  it("keeps a toast visible while an overlay is open", () => {
    render(
      <>
        <Modal open onClose={() => undefined} labelledBy="settings-title">
          <h2 id="settings-title">Settings</h2>
          <button type="button" onClick={() => toast.success("Settings saved")}>Save</button>
        </Modal>
        <ToastViewport />
      </>,
    );

    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(screen.getByRole("status").textContent).toContain("Settings saved");
  });
});
