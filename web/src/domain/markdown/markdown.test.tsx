import { describe, it, expect } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import {
  parseInline,
  parseBlocks,
  sanitizeHref,
  languageFromFence,
  MAX_MARKDOWN_CHARS,
  MAX_BLOCKS,
} from "./parse";
import { MarkdownContent } from "./MarkdownContent";

function html(source: string): string {
  return renderToStaticMarkup(<MarkdownContent source={source} />);
}

function hasInline(inlines: ReturnType<typeof parseInline>, type: string): boolean {
  return inlines.some((n) => n.type === type);
}

// ---- sanitizeHref ----
describe("sanitizeHref", () => {
  it("trims and allows https", () => {
    expect(sanitizeHref(" https://example.com ")).toBe("https://example.com");
  });
  it("rejects javascript:", () => {
    expect(sanitizeHref("javascript:alert(1)")).toBeNull();
    expect(sanitizeHref("  JaVaScRiPt:alert(1)")).toBeNull();
  });
  it("rejects data: and vbscript:", () => {
    expect(sanitizeHref("data:text/html,hi")).toBeNull();
    expect(sanitizeHref("vbscript:msgbox")).toBeNull();
    expect(sanitizeHref("DATA:text/html")).toBeNull();
  });
  it("rejects empty after trim", () => {
    expect(sanitizeHref("   ")).toBeNull();
  });
  it("keeps relative and http urls", () => {
    expect(sanitizeHref("/path?q=1")).toBe("/path?q=1");
    expect(sanitizeHref("http://example.com")).toBe("http://example.com");
  });
});

// ---- languageFromFence ----
describe("languageFromFence", () => {
  it("maps common fence aliases", () => {
    expect(languageFromFence("rust")).toBe("rust");
    expect(languageFromFence("Rs")).toBe("rust");
    expect(languageFromFence("ts")).toBe("typescript");
    expect(languageFromFence("tsx")).toBe("tsx");
    expect(languageFromFence("typescript")).toBe("typescript");
    expect(languageFromFence("js")).toBe("javascript");
    expect(languageFromFence("jsx")).toBe("tsx");
    expect(languageFromFence("json")).toBe("json");
    expect(languageFromFence("yaml")).toBe("yaml");
    expect(languageFromFence("yml")).toBe("yaml");
    expect(languageFromFence("toml")).toBe("toml");
    expect(languageFromFence("bash")).toBe("shell");
    expect(languageFromFence("python")).toBe("python");
    expect(languageFromFence("css")).toBe("css");
    expect(languageFromFence("html")).toBe("html");
    expect(languageFromFence("sql")).toBe("sql");
    expect(languageFromFence("md")).toBe("markdown");
  });
  it("takes first word only and trims", () => {
    expect(languageFromFence("rust foo bar")).toBe("rust");
    expect(languageFromFence("  python  ")).toBe("python");
  });
  it("unknown and empty is plain", () => {
    expect(languageFromFence("")).toBe("plain");
    expect(languageFromFence("unknownLang")).toBe("plain");
    expect(languageFromFence("   ")).toBe("plain");
  });
});

// ---- parseInline ----
describe("parseInline", () => {
  it("parses code and coalesces surrounding text", () => {
    const inlines = parseInline("a `code` b");
    expect(inlines).toEqual([
      { type: "text", text: "a " },
      { type: "code", text: "code" },
      { type: "text", text: " b" },
    ]);
  });
  it("unclosed backtick stays as text", () => {
    const inlines = parseInline("hello `unclosed");
    expect(inlines.every((n) => n.type === "text")).toBe(true);
  });
  it("empty code `` is empty string code node", () => {
    const inlines = parseInline("``");
    expect(inlines[0]).toEqual({ type: "code", text: "" });
  });
  it("parses links and sanitizes href", () => {
    const inlines = parseInline("[x](https://example.com)");
    expect(inlines[0]).toEqual({ type: "link", text: "x", href: "https://example.com" });
    const bad = parseInline("[x](javascript:alert(1))");
    expect(bad[0].type).toBe("text");
    // link text may be empty
    const emptyText = parseInline("[](https://example.com)");
    expect(emptyText[0]).toEqual({ type: "link", text: "", href: "https://example.com" });
  });
  it("link without closing paren is text", () => {
    const inlines = parseInline("[x](https://example.com");
    expect(inlines[0].type).toBe("text");
  });
  it("link with missing bracket paren structure is text", () => {
    const inlines = parseInline("[x] https://example.com)");
    expect(inlines[0].type).toBe("text");
  });
  it("bold ** and __", () => {
    expect(hasInline(parseInline("**bold**"), "bold")).toBe(true);
    expect(hasInline(parseInline("__bold__"), "bold")).toBe(true);
  });
  it("bold rejects empty inner", () => {
    const inlines = parseInline("****");
    expect(hasInline(inlines, "bold")).toBe(false);
  });
  it("bold rejects newline inner", () => {
    const inlines = parseInline("**a\nb**");
    // newline in inner is rejected — parser treats as text (newline remains inside plain chunk)
    // parseInline receives single line strings mostly; direct test: bold with newline should not match
    // We simulate: the parser checks !inner.contains('\n') so this would be rejected
    expect(hasInline(inlines, "bold")).toBe(false);
  });
  it("bold unclosed stays text", () => {
    const inlines = parseInline("**unclosed");
    expect(hasInline(inlines, "bold")).toBe(false);
  });
  it("italic * and _", () => {
    expect(hasInline(parseInline("*italic*"), "italic")).toBe(true);
    expect(hasInline(parseInline("_italic_"), "italic")).toBe(true);
    expect(hasInline(parseInline("*a* and _b_"), "italic")).toBe(true);
  });
  it("italic avoids matching inside bold delimiters", () => {
    // **bold** should be bold, not italic fragments. The italic scanner guards prev/next same delim.
    const inlines = parseInline("**bold**");
    expect(hasInline(inlines, "italic")).toBe(false);
    expect(hasInline(inlines, "bold")).toBe(true);
  });
  it("italic rejects empty and newline", () => {
    expect(hasInline(parseInline("**"), "italic")).toBe(false);
    // single char pair with empty
    expect(hasInline(parseInline("**"), "bold")).toBe(false);
  });
  it("italic stops at newline", () => {
    const inlines = parseInline("*a\nb*");
    expect(hasInline(inlines, "italic")).toBe(false);
  });
  it("plain coalesces consecutive text spans", () => {
    const inlines = parseInline("hello world [x](https://example.com) after");
    // first is text, second link, third text — but surrounding plains after link coalesced
    expect(inlines.filter((n) => n.type === "text").length).toBe(2);
  });
  it("code takes precedence over bold/link", () => {
    const inlines = parseInline("`**not bold**`");
    expect(inlines[0]).toEqual({ type: "code", text: "**not bold**" });
  });
  it("multiple inline types together", () => {
    const inlines = parseInline("**Verdict**: `code` and *italic* and [link](https://example.com)");
    expect(hasInline(inlines, "bold")).toBe(true);
    expect(hasInline(inlines, "code")).toBe(true);
    expect(hasInline(inlines, "italic")).toBe(true);
    expect(hasInline(inlines, "link")).toBe(true);
  });
});

// ---- parseBlocks ----
describe("parseBlocks", () => {
  it("parses headings with space and empty", () => {
    const { blocks } = parseBlocks("## TL;DR\nhello");
    expect(blocks[0]).toEqual({ type: "heading", level: 2, text: "TL;DR" });
    const { blocks: b2 } = parseBlocks("# ");
    expect(b2[0]).toEqual({ type: "heading", level: 1, text: "" });
    const { blocks: b3 } = parseBlocks("#");
    expect(b3[0]).toEqual({ type: "heading", level: 1, text: "" });
  });
  it("heading requires space or tab after hashes", () => {
    const { blocks } = parseBlocks("##no-space");
    expect(blocks[0].type).toBe("paragraph");
  });
  it("heading respects up to 6 hashes and leading spaces", () => {
    const { blocks } = parseBlocks("   ### hi");
    expect(blocks[0]).toEqual({ type: "heading", level: 3, text: "hi" });
    const { blocks: b2 } = parseBlocks("####### too many");
    expect(b2[0].type).toBe("paragraph");
    const { blocks: b3 } = parseBlocks("###### six");
    expect(b3[0].type).toBe("heading");
  });
  it("bullet lists with - and * and indentation", () => {
    const { blocks } = parseBlocks("- **Verdict**: ok\n- second\n  - indented? actually not bullet due to leading spaces? it is");
    // is_bullet uses trim_start, so indented still qualifies
    expect(blocks[0].type).toBe("bulletList");
    // bullet list terminates before paragraph
    const { blocks: b2 } = parseBlocks("- a\n- b\nnot bullet");
    expect(b2[0].type).toBe("bulletList");
    expect(b2[1].type).toBe("paragraph");
  });
  it("ordered lists with 1-9 digit numbers and dot-space", () => {
    const { blocks } = parseBlocks("1. first\n2. second");
    expect(blocks[0].type).toBe("orderedList");
    const { blocks: b2 } = parseBlocks("10. ok\n2. ok");
    // "10." has 2 digits -> still allowed, but need ". " after. So 10-digit-like but 2 digits okay
    expect(b2[0].type).toBe("orderedList");
    const { blocks: b3 } = parseBlocks("1234567890. too long");
    // 10 digits -> idx >9 -> not ordered
    expect(b3[0].type).toBe("paragraph");
    const { blocks: b4 } = parseBlocks("1.first no space");
    expect(b4[0].type).toBe("paragraph");
  });
  it("blockquotes merged across consecutive lines", () => {
    const { blocks } = parseBlocks("> hello\n> world\nnot bq");
    expect(blocks[0].type).toBe("blockquote");
    if (blocks[0].type === "blockquote") expect(blocks[0].text).toBe("hello\nworld");
    expect(blocks[1].type).toBe("paragraph");
  });
  it("blockquote with bare > line", () => {
    const { blocks } = parseBlocks(">\n> a");
    expect(blocks[0].type).toBe("blockquote");
  });
  it("fenced code keeps language and joins lines", () => {
    const { blocks } = parseBlocks("```rust\nfn main(){}\n```\nafter");
    expect(blocks[0]).toMatchObject({ type: "codeBlock", language: "rust", code: "fn main(){}" });
    expect(blocks[1].type).toBe("paragraph");
  });
  it("fenced code with empty language and indented open", () => {
    const { blocks } = parseBlocks("   ```\ncode\n   ```");
    expect(blocks[0]).toMatchObject({ type: "codeBlock", language: null, code: "code" });
  });
  it("fenced code closing fence is any line starting with ``` after trim", () => {
    const { blocks } = parseBlocks("```js\nhello\n```extra closing\nnext");
    expect(blocks[0].type).toBe("codeBlock");
    expect(blocks[1].type).toBe("paragraph");
  });
  it("unclosed fenced code collects to end", () => {
    const { blocks } = parseBlocks("```py\nline1\nline2");
    expect(blocks[0].type).toBe("codeBlock");
    if (blocks[0].type === "codeBlock") expect(blocks[0].code).toBe("line1\nline2");
  });
  it("fenced code language first word only", () => {
    const { blocks } = parseBlocks("```typescript extra\ncode\n```");
    expect(blocks[0].type).toBe("codeBlock");
  });
  it("paragraph joins consecutive lines with space and trims", () => {
    const { blocks } = parseBlocks("hello\nworld\nnext para\n\nsecond");
    expect(blocks[0]).toEqual({ type: "paragraph", text: "hello world next para" });
    expect(blocks[1].type).toBe("paragraph");
  });
  it("paragraph stops at block starts", () => {
    const { blocks } = parseBlocks("para\n## heading");
    expect(blocks[0].type).toBe("paragraph");
    expect(blocks[1].type).toBe("heading");
  });
  it("blank lines are skipped and not emitted", () => {
    const { blocks } = parseBlocks("\n\nhello\n\n\nworld\n\n");
    expect(blocks.length).toBe(2);
  });
  it("truncation flag and MAX_BLOCKS limit", () => {
    const long = "a ".repeat(MAX_MARKDOWN_CHARS + 100);
    const { blocks, truncated } = parseBlocks(long);
    expect(truncated).toBe(true);
    expect(blocks.length).toBeLessThanOrEqual(MAX_BLOCKS);
    const notTrunc = parseBlocks("hello");
    expect(notTrunc.truncated).toBe(false);
  });
  it("MAX_BLOCKS caps block count including list items separately capped", () => {
    const manyHeadings = Array.from({ length: 500 }, (_, i) => `# h${i}`).join("\n\n");
    const { blocks } = parseBlocks(manyHeadings);
    expect(blocks.length).toBeLessThanOrEqual(MAX_BLOCKS);
  });
  it("raw html is treated as paragraph text, not heading", () => {
    const { blocks } = parseBlocks("<script>alert(1)</script>");
    expect(blocks[0].type).toBe("paragraph");
    if (blocks[0].type === "paragraph") {
      const inlines = parseInline(blocks[0].text);
      expect(inlines[0].type).toBe("text");
      expect((inlines[0] as { type: "text"; text: string }).text).toContain("<script>");
    }
  });
  it("windows line endings \\r\\n handled", () => {
    const { blocks } = parseBlocks("hello\r\nworld\r\n\r\n# h");
    expect(blocks[0]).toEqual({ type: "paragraph", text: "hello world" });
    expect(blocks[1].type).toBe("heading");
  });
});

// ---- renderer ----
describe("MarkdownContent renderer", () => {
  it("wraps in markdown-content and truncated marker", () => {
    expect(html("hello")).toContain('class="markdown-content"');
    const long = "a ".repeat(MAX_MARKDOWN_CHARS + 10);
    const out = html(long);
    expect(out).toContain("review-token-comment");
    expect(out).toContain("… truncated");
  });
  it("renders headings h1..h6", () => {
    expect(html("# h1")).toContain("<h1>");
    expect(html("## h2")).toContain("<h2>");
    expect(html("### h3")).toContain("<h3>");
    expect(html("#### h4")).toContain("<h4>");
    expect(html("##### h5")).toContain("<h5>");
    expect(html("###### h6")).toContain("<h6>");
  });
  it("renders paragraphs, lists, blockquote, code block structures", () => {
    expect(html("hello world")).toContain("<p>");
    expect(html("- a\n- b")).toContain("<ul>");
    expect(html("1. a\n2. b")).toContain("<ol>");
    expect(html("1. a\n2. b")).toContain("<li>");
    expect(html("> hello\n> world")).toContain("<blockquote>");
    const codeHtml = html("```rust\nfn main(){}\n```");
    expect(codeHtml).toContain('class="review-content review-language-rust"');
    expect(codeHtml).toContain('<pre data-language="rust">');
  });
  it("inline code, bold, italic, link inside blocks", () => {
    const out = html("hello **bold** *italic* `code` [x](https://example.com)");
    expect(out).toContain("<strong>bold</strong>");
    expect(out).toContain("<em>italic</em>");
    expect(out).toContain("<code>code</code>");
    expect(out).toContain(
      '<a href="https://example.com" target="_blank" rel="noopener noreferrer">x</a>',
    );
  });
  it("autolinks web URLs but leaves code and punctuation plain", () => {
    const out = html(
      "https://example.com/a, `https://example.com/code` and http://example.com.",
    );
    expect(out).toContain(
      '<a href="https://example.com/a" target="_blank" rel="noopener noreferrer">https://example.com/a</a>',
    );
    expect(out).toContain("><span>, </span>");
    expect(out).toContain("<code>https://example.com/code</code>");
    expect(out).toContain(
      '<a href="http://example.com" target="_blank" rel="noopener noreferrer">http://example.com</a>',
    );
    expect(out).toContain("><span>.</span>");
  });
  it("escapes raw html and does not emit unsafe markup", () => {
    const out = html("<script>alert(1)</script>");
    expect(out).not.toContain("<script>");
    expect(out).toContain("&lt;script&gt;");
    const codeOut = html("```html\n<script>\n```");
    expect(codeOut).toContain("&lt;script&gt;");
  });
  it("sanitized link not rendered as anchor", () => {
    const out = html("[x](javascript:alert(1))");
    expect(out).not.toContain("<a");
    expect(out).toContain("[x](javascript:alert(1))");
  });
  it("keeps quirk: ##no-space is paragraph not heading", () => {
    const out = html("##no-space");
    expect(out).not.toContain("<h2>");
    expect(out).toContain("<p>");
  });
  it("does not use dangerouslySetInnerHTML", async () => {
    const fs = await import("node:fs");
    const src = fs.readFileSync("src/domain/markdown/MarkdownContent.tsx", "utf8");
    expect(src).not.toContain("dangerouslySetInnerHTML");
    const parseSrc = fs.readFileSync("src/domain/markdown/parse.ts", "utf8");
    expect(parseSrc).not.toContain("dangerouslySetInnerHTML");
  });
  it("code block plain language fallback has correct class", () => {
    const out = html("```\nhello\n```");
    expect(out).toContain('review-language-plain');
    expect(out).toContain('data-language="plain"');
  });
  it("heading inline formatting preserved", () => {
    const out = html("## **Bold** heading");
    expect(out).toContain("<h2>");
    expect(out).toContain("<strong>Bold</strong>");
  });
});
