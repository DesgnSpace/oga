export interface FontOption {
  id: string;
  label: string;
  stack: string;
}

export const FONT_OPTIONS: FontOption[] = [
  {
    id: "plex-sans",
    label: "IBM Plex Sans",
    stack: '"IBM Plex Sans", ui-sans-serif, system-ui, -apple-system, sans-serif',
  },
  { id: "system", label: "System font", stack: "ui-sans-serif, system-ui, -apple-system, sans-serif" },
];

export const DEFAULT_FONT = FONT_OPTIONS[0];

const FONT_CACHE_KEY = "oga.appearance.font";

/** Unknown ids, such as one saved by a newer build, fall back to the default. */
export function resolveFont(id: string | null | undefined): FontOption {
  return FONT_OPTIONS.find((option) => option.id === id) ?? DEFAULT_FONT;
}

/** Sets the interface font and remembers it for the next launch. */
export function applyFont(id: string | null | undefined): FontOption {
  const font = resolveFont(id);
  document.documentElement.style.setProperty("--font-sans", font.stack);
  try {
    if (id) localStorage.setItem(FONT_CACHE_KEY, id);
    else localStorage.removeItem(FONT_CACHE_KEY);
  } catch {
    // Storage can be unavailable; the font still applies for this session.
  }
  return font;
}

export function readCachedFont(): string | null {
  try {
    return localStorage.getItem(FONT_CACHE_KEY);
  } catch {
    return null;
  }
}
