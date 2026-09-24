// The transport for bridge calls, swappable for tests and non-Tauri hosts.

import type { BridgeError } from "./types";

export interface Transport {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, handle: (payload: T) => void): void;
}

const BRIDGE_MISSING = "Oga's desktop bridge is unavailable.";

function tauriGlobal(): Record<string, any> | undefined {
  if (typeof window === "undefined") return undefined;
  return (window as unknown as { __TAURI__?: Record<string, any> }).__TAURI__;
}

function toBridgeError(error: unknown): BridgeError {
  if (error && typeof error === "object" && "message" in error) {
    const candidate = error as { message: unknown; status?: unknown };
    if (typeof candidate.message === "string") {
      return {
        message: candidate.message,
        status: typeof candidate.status === "number" ? candidate.status : undefined,
      };
    }
  }
  if (typeof error === "string") return { message: error };
  return { message: BRIDGE_MISSING };
}

export const tauriTransport: Transport = {
  async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    const tauri = tauriGlobal();
    if (!tauri?.core?.invoke) {
      throw toBridgeError(BRIDGE_MISSING);
    }
    try {
      return (await tauri.core.invoke(command, args)) as T;
    } catch (error) {
      throw toBridgeError(error);
    }
  },
  listen<T>(event: string, handle: (payload: T) => void): void {
    const tauri = tauriGlobal();
    if (!tauri?.event?.listen) return;
    void tauri.event.listen(event, (raw: { payload: T }) => handle(raw.payload));
  },
};

let activeTransport: Transport = tauriTransport;

export function getTransport(): Transport {
  return activeTransport;
}

export function setTransport(transport: Transport): void {
  activeTransport = transport;
}

export { BRIDGE_MISSING };
