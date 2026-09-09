import { describe, expect, test } from "bun:test";
import { copyText, shortId } from "./identifiers";

describe("shortId", () => {
  test("keeps the leading UUID segment", () => {
    expect(shortId("12345678-abcd-4abc-8def-1234567890ab")).toBe("12345678");
  });

  test("does not shorten non-UUID values", () => {
    expect(shortId("main")).toBe("main");
  });
});

describe("copyText", () => {
  test("copies the complete value, not its display form", async () => {
    const writes: string[] = [];
    const originalClipboard = navigator.clipboard;
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: async (value: string) => { writes.push(value); } },
    });
    try {
      await copyText("12345678-abcd-4abc-8def-1234567890ab");
      expect(writes).toEqual(["12345678-abcd-4abc-8def-1234567890ab"]);
    } finally {
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: originalClipboard });
    }
  });
});
