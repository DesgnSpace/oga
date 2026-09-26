import { readFileSync } from "node:fs";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, jest } from "bun:test";
import { ToastViewport } from "./ToastViewport";
import { Modal } from "@/components/primitives/Modal";
import { toast } from "@/state/toast";

// The stylesheet decides what a pointer can land on and which toasts hide behind the stack.
const stylesheet = document.createElement("style");
stylesheet.textContent = readFileSync(new URL("../oga.css", import.meta.url), "utf8");

beforeAll(() => document.head.append(stylesheet));
afterAll(() => stylesheet.remove());

afterEach(() => {
  act(() => toast.clear());
  cleanup();
  jest.useRealTimers();
});

/**
 * The nearest element in the stack that takes the pointer, else the page beneath.
 * Opacity is ignored, as in a browser: a faded-out box still catches clicks.
 */
function landsOn(element: Element, beneath: Element = document.body): Element {
  const region = screen.getByRole("region", { name: "Notifications" });
  for (let current: Element | null = element; current && region.contains(current); current = current.parentElement) {
    const style = getComputedStyle(current);
    if (style.pointerEvents !== "none" && style.visibility !== "hidden") return current;
  }
  return beneath;
}

function morePill(): HTMLElement | null {
  return screen.queryByRole("button", { name: /more$/ });
}

function showErrors(count: number) {
  act(() => {
    for (let index = 1; index <= count; index += 1) toast.error(`Toast ${index}`);
  });
}

function closeToast(title: string) {
  const card = screen.getByText(title).closest("article")!;
  fireEvent.click(within(card).getByRole("button", { name: "Dismiss notification" }));
}

/** jsdom paints nothing, so a leaving toast's transition never reports its end. */
function waitOutExits() {
  act(() => jest.advanceTimersByTime(1_000));
}

describe("ToastViewport", () => {
  it("counts only the toasts still waiting to be seen and drops the pill at zero", () => {
    jest.useFakeTimers();
    render(<ToastViewport />);
    showErrors(5);
    expect(morePill()?.textContent).toBe("+2 more");

    closeToast("Toast 5");
    waitOutExits();
    expect(morePill()?.textContent).toBe("+1 more");

    closeToast("Toast 4");
    waitOutExits();
    expect(morePill()).toBeNull();
  });

  it("leaves no pill behind for toasts that went before they were ever shown", () => {
    jest.useFakeTimers();
    render(<ToastViewport />);
    for (let index = 1; index <= 30; index += 1) {
      act(() => toast.info(`Task ${index} finished`));
      act(() => {
        for (const record of toast.snapshot) toast.dismiss(record.id);
      });
    }

    expect(morePill()).toBeNull();
    waitOutExits();
    expect(screen.queryAllByText(/finished$/)).toHaveLength(0);
  });

  it("expands the stack when the pill is hovered", () => {
    render(<ToastViewport />);
    showErrors(5);
    expect(screen.getAllByRole("alert")).toHaveLength(3);

    fireEvent.mouseEnter(landsOn(morePill()!));

    expect(screen.getAllByRole("alert")).toHaveLength(5);
  });

  it("expands the stack when the pill is clicked", () => {
    render(<ToastViewport />);
    showErrors(5);

    fireEvent.click(landsOn(morePill()!));

    expect(screen.getAllByRole("alert")).toHaveLength(5);
  });

  it("lets a click on the stack's empty area reach the page beneath", () => {
    render(
      <>
        <button type="button">Open task</button>
        <ToastViewport />
      </>,
    );
    const page = screen.getByRole("button", { name: "Open task" });
    const region = screen.getByRole("region", { name: "Notifications" });
    showErrors(5);

    const front = screen.getByText("Toast 5").closest("article")!;
    const behind = screen.getByText("Toast 1").closest("article")!;
    expect(landsOn(front, page)).toBe(front);
    expect(landsOn(behind, page)).toBe(page);
    expect(landsOn(region, page)).toBe(page);

    act(() => {
      for (const record of toast.snapshot) toast.dismiss(record.id);
    });
    for (const element of [region, ...region.querySelectorAll("*")]) {
      expect(landsOn(element, page)).toBe(page);
    }
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

  it("dismisses a toast when swiped left", () => {
    render(<ToastViewport />);
    act(() => toast.success("Saved"));

    const card = screen.getByRole("status");
    fireEvent.pointerDown(card, { pointerId: 1, clientX: 100 });
    fireEvent.pointerUp(card, { pointerId: 1, clientX: -20 });

    expect(card.classList.contains("toast-card-exiting")).toBe(true);
  });
});
