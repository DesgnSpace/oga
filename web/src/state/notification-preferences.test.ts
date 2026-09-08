import { describe, expect, it } from "bun:test";
import { loadTaskNotifications, storeTaskNotifications } from "./notification-preferences";

describe("task notifications", () => {
  it("is on for a fresh install", () => {
    expect(loadTaskNotifications()).toBe(true);
  });

  it("remembers being turned off, and back on", () => {
    storeTaskNotifications(false);
    expect(loadTaskNotifications()).toBe(false);

    storeTaskNotifications(true);
    expect(loadTaskNotifications()).toBe(true);
  });
});
