// Event payloads parsed once and shared by every reader, since a live task
// re-reads each payload on every update. Callers must not mutate the result.

// About the payload text of a 5,000-event task.
const HELD_TEXT_LIMIT = 32_000_000;

const INVALID = Symbol("invalid JSON");

// Never holds `undefined`, since `JSON.parse` cannot return it. Oldest read first.
const parsed = new Map<string, unknown>();
let heldText = 0;

/** `undefined` when the text is not JSON. */
export function parseRawJson(raw: string): unknown {
  const held = parsed.get(raw);
  if (held !== undefined) {
    parsed.delete(raw);
    parsed.set(raw, held);
    return held === INVALID ? undefined : held;
  }
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    value = INVALID;
  }
  parsed.set(raw, value);
  heldText += raw.length;
  for (const text of parsed.keys()) {
    if (heldText <= HELD_TEXT_LIMIT) break;
    parsed.delete(text);
    heldText -= text.length;
  }
  return value === INVALID ? undefined : value;
}
