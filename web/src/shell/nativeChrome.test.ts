import { afterEach, beforeAll, describe, expect, it } from "bun:test";
import { cleanup, fireEvent } from "@testing-library/react";
import { installNativeChrome } from "./nativeChrome";

beforeAll(() => {
  installNativeChrome();
});

afterEach(cleanup);

describe("installNativeChrome keystroke suppression", () => {
  it("suppresses a plain, unmodified keystroke outside an editable target (the beep case)", () => {
    const notPrevented = fireEvent.keyDown(document.body, { key: "a", code: "KeyA" });
    expect(notPrevented).toBe(false);
  });

  it("leaves a Cmd-held keystroke alone so the native menu accelerator still fires", () => {
    const notPrevented = fireEvent.keyDown(document.body, {
      key: "=",
      code: "Equal",
      metaKey: true,
    });
    expect(notPrevented).toBe(true);
  });

  it("leaves a Ctrl-held keystroke alone", () => {
    const notPrevented = fireEvent.keyDown(document.body, {
      key: "=",
      code: "Equal",
      ctrlKey: true,
    });
    expect(notPrevented).toBe(true);
  });

  it("leaves an Alt-held keystroke alone", () => {
    const notPrevented = fireEvent.keyDown(document.body, {
      key: "a",
      code: "KeyA",
      altKey: true,
    });
    expect(notPrevented).toBe(true);
  });

  it("still leaves typing in an input alone, modifier or not", () => {
    const input = document.createElement("input");
    document.body.appendChild(input);

    const notPrevented = fireEvent.keyDown(input, { key: "a", code: "KeyA" });

    expect(notPrevented).toBe(true);
    input.remove();
  });
});
