import { describe, expect, it } from "bun:test";
import { relativeTime } from "./time";

const now = new Date("2026-09-02T12:00:00Z");

describe("relativeTime", () => {
  it("uses short relative units", () => {
    expect(relativeTime(new Date("2026-09-02T11:59:40Z"), now)).toBe("just now");
    expect(relativeTime(new Date("2026-09-02T11:55:00Z"), now)).toBe("5 min ago");
    expect(relativeTime(new Date("2026-09-02T10:00:00Z"), now)).toBe("2 h ago");
    expect(relativeTime(new Date("2026-08-30T12:00:00Z"), now)).toBe("3 d ago");
  });

  it("falls back to a short date", () => {
    expect(relativeTime(new Date("2026-08-01T12:00:00Z"), now)).toBe(
      new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(new Date("2026-08-01T12:00:00Z")),
    );
  });
});
