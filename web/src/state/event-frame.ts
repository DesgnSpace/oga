// Reads one broker stream frame's event name and JSON body into the shape
// sidebar state folds. Ported from `EventFrame::from_parts`
// (rust/crates/oga-client/src/lib.rs).

import type { EventKind, EventPointer } from "@/bridge/types";

interface StreamFrame {
  event: string;
  data: unknown;
}

export interface ReadyFrame {
  version: number;
  cursor: number;
  streamFloor: number;
  tasks: string[];
  kinds: EventKind[];
  agents: boolean;
  stale: boolean;
}

export interface CursorFrame {
  cursor: number;
}

export type EventFrame =
  | { kind: "ready"; frame: ReadyFrame }
  | { kind: "task"; pointer: EventPointer }
  | { kind: "cursor"; frame: CursorFrame }
  | { kind: "keepalive"; frame: CursorFrame }
  | { kind: "unknown"; event: string; data: unknown };

export function parseEventFrame(frame: StreamFrame): EventFrame {
  switch (frame.event) {
    case "ready":
      return { kind: "ready", frame: frame.data as ReadyFrame };
    case "task":
      return { kind: "task", pointer: frame.data as EventPointer };
    case "cursor":
      return { kind: "cursor", frame: frame.data as CursorFrame };
    case "keepalive":
      return { kind: "keepalive", frame: frame.data as CursorFrame };
    default:
      return { kind: "unknown", event: frame.event, data: frame.data };
  }
}
