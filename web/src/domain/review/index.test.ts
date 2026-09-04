// Ported from rust/crates/oga-ui/src/review/mod.rs `#[cfg(test)] mod tests`.

import { describe, expect, it } from "bun:test";
import { codeLanguageFromPath } from "@/domain/trace";
import { highlight, MAX_HIGHLIGHTED_CHARS, resolveCodeLanguage } from "./index";

describe("codeLanguageFromPath", () => {
  it("uses the file extension", () => {
    expect(codeLanguageFromPath("src/app.tsx")).toBe("tsx");
    expect(codeLanguageFromPath("src/app.jsx")).toBe("tsx");
    expect(codeLanguageFromPath("README.md")).toBe("markdown");
    expect(codeLanguageFromPath("Cargo.toml")).toBe("toml");
  });

  it("stays plain for unknown paths", () => {
    expect(codeLanguageFromPath(undefined)).toBe("plain");
    expect(codeLanguageFromPath("LICENSE")).toBe("plain");
  });

  it("prefers known language metadata over the file extension", () => {
    expect(resolveCodeLanguage(undefined, "src/App.tsx")).toBe("tsx");
    expect(resolveCodeLanguage("plain", "src/query.sql")).toBe("sql");
    expect(resolveCodeLanguage("json", "src/query.sql")).toBe("json");
  });
});

describe("highlight", () => {
  it("preserves punctuation while marking syntax nodes", () => {
    const source = "{\n  [ , : ] }\n";
    const tokens = highlight(source, "json");
    expect(tokens.map((token) => token.text).join("")).toBe(source);
    expect(tokens.some((token) => token.className === "review-token-punctuation")).toBe(true);

    const json = highlight('{"a": 1}', "json");
    expect(json.some((token) => token.className === "review-token-property")).toBe(true);
    expect(json.some((token) => token.className === "review-token-number")).toBe(true);
  });

  it("stops at the token cap for alternating text", () => {
    const source = "a ".repeat(MAX_HIGHLIGHTED_CHARS);
    const tokens = highlight(source, "plain");
    expect(tokens.length).toBeLessThanOrEqual(8_000 + 1);
    expect(tokens.at(-1)?.text.includes("truncated")).toBe(true);
  });

  it("stops highlighting a huge source", () => {
    const source = "x ".repeat(MAX_HIGHLIGHTED_CHARS);
    const tokens = highlight(source, "plain");
    const rendered = tokens.reduce((sum, token) => sum + token.text.length, 0);
    expect(rendered).toBeLessThan(source.length);
    expect(tokens.at(-1)?.text.includes("truncated")).toBe(true);
  });

  it("marks keywords and strings", () => {
    const tokens = highlight('fn main() { let name = "oga"; }', "rust");
    expect(tokens.some((token) => token.className === "review-token-keyword")).toBe(true);
    expect(tokens.some((token) => token.className === "review-token-string")).toBe(true);
  });
});
