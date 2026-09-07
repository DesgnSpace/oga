import { codeLanguageFromPath, type CodeLanguage } from "@/domain/trace";

export type { CodeLanguage };
export { codeLanguageFromPath };

export interface CodeToken {
  text: string;
  className?: string;
  changed: boolean;
}

/** Caps highlighting work for large sources. */
export const MAX_HIGHLIGHTED_CHARS = 20_000;

export function resolveCodeLanguage(language: CodeLanguage | undefined, path: string | undefined): CodeLanguage {
  return language !== undefined && language !== "plain" ? language : codeLanguageFromPath(path);
}

export function limitHighlightedSource(source: string): string {
  const limited = source.slice(0, MAX_HIGHLIGHTED_CHARS);
  if (limited.length >= source.length) return limited;
  const last = limited.charCodeAt(limited.length - 1);
  const safe = last >= 0xd800 && last <= 0xdbff ? limited.slice(0, -1) : limited;
  return `${safe}\n… truncated`;
}
