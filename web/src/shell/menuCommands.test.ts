import { afterEach, describe, expect, it, mock } from "bun:test";
import { fireEvent } from "@testing-library/react";
import type { Transport } from "@/bridge/transport";
import { MENU_EVENT } from "@/bridge/types";

afterEach(() => {
  localStorage.removeItem("oga:zoom-index");
});

function fakeTransport() {
  const listeners = new Map<string, Array<(payload: unknown) => void>>();
  const transport: Transport = {
    invoke: mock(),
    listen: mock((event, handle) => {
      const bucket = listeners.get(event) ?? [];
      bucket.push(handle as (payload: unknown) => void);
      listeners.set(event, bucket);
    }),
  };
  return {
    transport,
    emit(event: string, payload: unknown) {
      for (const handle of listeners.get(event) ?? []) handle(payload);
    },
  };
}

// menuCommands.ts imports the module-level event Feed singleton, so each
// test needs a fresh module graph bound to its own fake transport.
async function freshMenuCommands(transport: Transport) {
  const { resetFeedsForTests } = await import("@/bridge/events");
  resetFeedsForTests();
  const { setTransport } = await import("@/bridge/transport");
  setTransport(transport);
  return import("./menuCommands");
}

function fakeSidebar() {
  return {
    toggleSidebar: mock(),
    refresh: mock(),
    clearSelection: mock(),
  };
}

describe("subscribeMenuCommands", () => {
  it("toggles the sidebar on toggle-sidebar", async () => {
    const fake = fakeTransport();
    const { subscribeMenuCommands } = await freshMenuCommands(fake.transport);
    const sidebar = fakeSidebar();
    const navigate = mock();
    subscribeMenuCommands(() => ({ sidebar: sidebar as never, route: { kind: "home" }, navigate }));

    fake.emit(MENU_EVENT, "toggle-sidebar");

    expect(sidebar.toggleSidebar).toHaveBeenCalledOnce();
  });

  it("refreshes tasks on refresh-tasks", async () => {
    const fake = fakeTransport();
    const { subscribeMenuCommands } = await freshMenuCommands(fake.transport);
    const sidebar = fakeSidebar();
    subscribeMenuCommands(() => ({ sidebar: sidebar as never, route: { kind: "home" }, navigate: mock() }));

    fake.emit(MENU_EVENT, "refresh-tasks");

    expect(sidebar.refresh).toHaveBeenCalledOnce();
  });

  it("clears the selection and navigates home from a task route on clear-selection", async () => {
    const fake = fakeTransport();
    const { subscribeMenuCommands } = await freshMenuCommands(fake.transport);
    const sidebar = fakeSidebar();
    const navigate = mock();
    subscribeMenuCommands(() => ({ sidebar: sidebar as never, route: { kind: "task", id: "t1" }, navigate }));

    fake.emit(MENU_EVENT, "clear-selection");

    expect(sidebar.clearSelection).toHaveBeenCalledOnce();
    expect(navigate).toHaveBeenCalledWith({ kind: "home" });
  });

  it("does not navigate on clear-selection when already home", async () => {
    const fake = fakeTransport();
    const { subscribeMenuCommands } = await freshMenuCommands(fake.transport);
    const sidebar = fakeSidebar();
    const navigate = mock();
    subscribeMenuCommands(() => ({ sidebar: sidebar as never, route: { kind: "home" }, navigate }));

    fake.emit(MENU_EVENT, "clear-selection");

    expect(navigate).not.toHaveBeenCalled();
  });

  it("navigates to settings on open-settings", async () => {
    const fake = fakeTransport();
    const { subscribeMenuCommands } = await freshMenuCommands(fake.transport);
    const sidebar = fakeSidebar();
    const navigate = mock();
    subscribeMenuCommands(() => ({ sidebar: sidebar as never, route: { kind: "home" }, navigate }));

    fake.emit(MENU_EVENT, "open-settings");

    expect(navigate).toHaveBeenCalledWith({ kind: "settings" });
  });

  it("handles zoom-in, zoom-out and zoom-reset without a DOM present", async () => {
    const fake = fakeTransport();
    const { subscribeMenuCommands } = await freshMenuCommands(fake.transport);
    const sidebar = fakeSidebar();
    subscribeMenuCommands(() => ({ sidebar: sidebar as never, route: { kind: "home" }, navigate: mock() }));

    expect(() => {
      fake.emit(MENU_EVENT, "zoom-in");
      fake.emit(MENU_EVENT, "zoom-out");
      fake.emit(MENU_EVENT, "zoom-reset");
    }).not.toThrow();
  });

  it("unsubscribes when the returned callback runs", async () => {
    const fake = fakeTransport();
    const { subscribeMenuCommands } = await freshMenuCommands(fake.transport);
    const sidebar = fakeSidebar();
    const unsubscribe = subscribeMenuCommands(() => ({ sidebar: sidebar as never, route: { kind: "home" }, navigate: mock() }));

    unsubscribe();
    fake.emit(MENU_EVENT, "refresh-tasks");

    expect(sidebar.refresh).not.toHaveBeenCalled();
  });

  it("zooms in on Cmd+Shift+=, which has no native menu accelerator of its own", async () => {
    const fake = fakeTransport();
    const { subscribeMenuCommands } = await freshMenuCommands(fake.transport);
    const sidebar = fakeSidebar();
    subscribeMenuCommands(() => ({ sidebar: sidebar as never, route: { kind: "home" }, navigate: mock() }));
    fake.emit(MENU_EVENT, "zoom-reset");
    const before = document.documentElement.style.zoom;

    const notPrevented = fireEvent.keyDown(window, { key: "+", code: "Equal", metaKey: true, shiftKey: true });

    expect(notPrevented).toBe(false);
    expect(document.documentElement.style.zoom).not.toBe(before);
  });

  it("leaves Cmd+= (no Shift) to the native menu accelerator instead of double-zooming", async () => {
    const fake = fakeTransport();
    const { subscribeMenuCommands } = await freshMenuCommands(fake.transport);
    const sidebar = fakeSidebar();
    subscribeMenuCommands(() => ({ sidebar: sidebar as never, route: { kind: "home" }, navigate: mock() }));
    fake.emit(MENU_EVENT, "zoom-reset");
    const before = document.documentElement.style.zoom;

    const notPrevented = fireEvent.keyDown(window, { key: "=", code: "Equal", metaKey: true });

    expect(notPrevented).toBe(true);
    expect(document.documentElement.style.zoom).toBe(before);
  });

  it("persists the zoom level to storage so it survives a reload", async () => {
    const fake = fakeTransport();
    const { subscribeMenuCommands } = await freshMenuCommands(fake.transport);
    const sidebar = fakeSidebar();
    subscribeMenuCommands(() => ({ sidebar: sidebar as never, route: { kind: "home" }, navigate: mock() }));
    fake.emit(MENU_EVENT, "zoom-reset");

    fake.emit(MENU_EVENT, "zoom-in");

    // Default zoom index is 2 (the 1.0 step); zoom-in advances it to 3.
    expect(localStorage.getItem("oga:zoom-index")).toBe("3");
  });

  it("stops reacting to the Cmd+Shift+= shortcut once unsubscribed", async () => {
    const fake = fakeTransport();
    const { subscribeMenuCommands } = await freshMenuCommands(fake.transport);
    const sidebar = fakeSidebar();
    const unsubscribe = subscribeMenuCommands(() => ({ sidebar: sidebar as never, route: { kind: "home" }, navigate: mock() }));
    fake.emit(MENU_EVENT, "zoom-reset");
    unsubscribe();
    const before = document.documentElement.style.zoom;

    fireEvent.keyDown(window, { key: "+", code: "Equal", metaKey: true, shiftKey: true });

    expect(document.documentElement.style.zoom).toBe(before);
  });
});
