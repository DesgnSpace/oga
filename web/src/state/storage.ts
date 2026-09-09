// Safe `localStorage` access. Ported preferences read the same keys the Rust
// app wrote, so a user's choices survive the framework switch.

export function readStorage(key: string): string | undefined {
  if (typeof window === "undefined") return undefined;
  try {
    return window.localStorage.getItem(key) ?? undefined;
  } catch {
    return undefined;
  }
}

export function writeStorage(key: string, value: string): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Storage can be unavailable (private browsing quotas, disabled cookies).
  }
}
