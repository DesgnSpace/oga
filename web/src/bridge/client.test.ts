import { beforeEach, describe, expect, it, mock } from "bun:test";
import { broker } from "./client";
import { setTransport, type Transport } from "./transport";
import type { BridgeError } from "./types";

function fakeTransport(invoke: Transport["invoke"]): Transport {
  return { invoke, listen: mock() };
}

beforeEach(() => {
  setTransport(fakeTransport(mock()));
});

describe("broker call failure", () => {
  it("surfaces a rejection as a typed result instead of throwing", async () => {
    const error: BridgeError = { message: "stale revision", status: 409 };
    const invoke = mock().mockRejectedValue(error);
    setTransport(fakeTransport(invoke));

    const result = await broker.archiveTask("task-1", true);

    expect(result).toEqual({ ok: false, error });
  });
});
