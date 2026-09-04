import { describe, expect, test } from "bun:test";
import { isResume, isSendDisabled, routingForState } from "./Composer";

describe("routingForState", () => {
  test("a running task with a steerable worker can steer and queue", () => {
    expect(routingForState("running", true, undefined)).toEqual({ type: "steer-and-queue" });
  });

  test("a running task without a steerable worker can only queue", () => {
    expect(routingForState("running", false, undefined)).toEqual({ type: "queue" });
  });

  test("a task needing input routes to reply, carrying the question", () => {
    expect(routingForState("needs_input", false, "question")).toEqual({
      type: "reply",
      question: "question",
    });
  });

  test("a completed task requires text to resume", () => {
    expect(routingForState("completed", false, undefined)).toEqual({
      type: "resume",
      textRequired: true,
    });
  });

  test("a failed or cancelled task can resume with an empty message", () => {
    expect(routingForState("failed", false, undefined)).toEqual({ type: "resume", textRequired: false });
    expect(routingForState("cancelled", false, undefined)).toEqual({ type: "resume", textRequired: false });
  });
});

describe("isSendDisabled", () => {
  test("an empty draft is allowed only for a resume that does not require text", () => {
    expect(isSendDisabled({ type: "resume", textRequired: false }, "")).toBe(false);
    expect(isSendDisabled({ type: "resume", textRequired: true }, " ")).toBe(true);
    expect(isSendDisabled({ type: "queue" }, "\n")).toBe(true);
  });

  test("a non-empty draft is always sendable except when routing is none", () => {
    expect(isSendDisabled({ type: "queue" }, "hello")).toBe(false);
    expect(isSendDisabled({ type: "reply", question: "" }, "hello")).toBe(false);
    expect(isSendDisabled({ type: "none" }, "hello")).toBe(true);
  });
});

describe("isResume", () => {
  test("only the resume routing counts as a resume", () => {
    expect(isResume({ type: "resume", textRequired: false })).toBe(true);
    expect(isResume({ type: "queue" })).toBe(false);
  });
});
