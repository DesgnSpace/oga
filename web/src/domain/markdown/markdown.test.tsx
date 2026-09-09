import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, it, expect } from "bun:test";
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

afterEach(cleanup);

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
  it("updates active text when the source changes", () => {
    const view = render(<MarkdownContent source="first update" />);
    expect(view.container.textContent).toContain("first update");

    view.rerender(<MarkdownContent source="second update" />);

    expect(view.container.textContent).toContain("second update");
    expect(view.container.textContent).not.toContain("first update");
  });

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

// ---- GFM additions ----
describe("strikethrough", () => {
  it("parses ~~text~~", () => {
    expect(parseInline("~~gone~~")).toEqual([{ type: "strike", text: "gone" }]);
    expect(html("~~gone~~")).toContain("<del>gone</del>");
  });
  it("unclosed and empty stay text", () => {
    expect(hasInline(parseInline("a ~~b"), "strike")).toBe(false);
    expect(hasInline(parseInline("~~~~"), "strike")).toBe(false);
  });
  it("code wins over strikethrough", () => {
    expect(parseInline("`~~x~~`")).toEqual([{ type: "code", text: "~~x~~" }]);
  });
});

describe("thematic breaks", () => {
  it("parses ---, *** and ___ with spaces", () => {
    for (const source of ["---", "***", "___", "- - -", "  ***  "]) {
      expect(parseBlocks(source).blocks[0]).toEqual({ type: "thematicBreak" });
    }
    expect(html("a\n\n---\n\nb")).toContain("<hr/>");
  });
  it("needs three markers and nothing else", () => {
    expect(parseBlocks("--").blocks[0].type).toBe("paragraph");
    expect(parseBlocks("--- text").blocks[0].type).toBe("paragraph");
  });
  it("breaks a paragraph when it stands on its own", () => {
    const { blocks } = parseBlocks("para\n\n---\n\nafter");
    expect(blocks.map((b) => b.type)).toEqual(["paragraph", "thematicBreak", "paragraph"]);
  });
  it("underlines the paragraph above it instead", () => {
    const { blocks } = parseBlocks("para\n---\nafter");
    expect(blocks[0]).toEqual({ type: "heading", level: 2, text: "para" });
    expect(blocks[1].type).toBe("paragraph");
  });
});

describe("nested lists", () => {
  it("nests a deeper item under the item above it", () => {
    const { blocks } = parseBlocks("- a\n  - b\n    - c\n- d");
    expect(blocks.length).toBe(1);
    if (blocks[0].type !== "bulletList") throw new Error("expected bulletList");
    expect(blocks[0].items.length).toBe(2);
    expect(blocks[0].items[0].children?.items[0].text).toBe("b");
    expect(blocks[0].items[0].children?.items[0].children?.items[0].text).toBe("c");
    expect(blocks[0].items[1].text).toBe("d");
  });
  it("keeps the nested list's own kind", () => {
    const { blocks } = parseBlocks("- a\n  1. one\n  2. two");
    if (blocks[0].type !== "bulletList") throw new Error("expected bulletList");
    expect(blocks[0].items[0].children?.ordered).toBe(true);
    expect(blocks[0].items[0].children?.items.length).toBe(2);
    expect(html("- a\n  1. one")).toContain("<ol>");
  });
  it("a sibling of the other kind starts a new list", () => {
    const { blocks } = parseBlocks("- a\n1. b");
    expect(blocks.map((b) => b.type)).toEqual(["bulletList", "orderedList"]);
  });
  it("accepts + as a bullet marker", () => {
    expect(parseBlocks("+ a\n+ b").blocks[0].type).toBe("bulletList");
  });
});

describe("task lists", () => {
  it("reads the checkbox state and strips the marker", () => {
    const { blocks } = parseBlocks("- [x] done\n- [ ] todo\n- plain");
    if (blocks[0].type !== "bulletList") throw new Error("expected bulletList");
    expect(blocks[0].items.map((i) => [i.text, i.checked])).toEqual([
      ["done", true],
      ["todo", false],
      ["plain", null],
    ]);
  });
  it("renders a disabled checkbox only for task items", () => {
    const out = html("- [x] done\n- plain");
    expect(out).toContain('<li data-task="true">');
    expect(out).toContain('type="checkbox"');
    expect(out).toContain("disabled");
    expect(out).toContain("<li><span>plain</span></li>");
  });
  it("uppercase X counts as checked and [y] does not", () => {
    const { blocks } = parseBlocks("- [X] a\n- [y] b");
    if (blocks[0].type !== "bulletList") throw new Error("expected bulletList");
    expect(blocks[0].items[0].checked).toBe(true);
    expect(blocks[0].items[1].checked).toBeNull();
  });
});

describe("tables", () => {
  it("parses header, alignment and body", () => {
    const { blocks } = parseBlocks("| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |");
    expect(blocks[0]).toEqual({
      type: "table",
      align: ["left", "center", "right"],
      header: ["a", "b", "c"],
      rows: [["1", "2", "3"]],
    });
  });
  it("needs a delimiter row matching the header width", () => {
    expect(parseBlocks("cost | benefit").blocks[0].type).toBe("paragraph");
    expect(parseBlocks("| a | b |\n|---|\n| 1 | 2 |").blocks[0].type).toBe("paragraph");
    expect(parseBlocks("| a |\n| b |\n| c |").blocks[0].type).toBe("paragraph");
  });
  it("pads short rows and drops extra cells", () => {
    const { blocks } = parseBlocks("| a | b |\n|---|---|\n| 1 |\n| 1 | 2 | 3 |");
    if (blocks[0].type !== "table") throw new Error("expected table");
    expect(blocks[0].rows).toEqual([["1", ""], ["1", "2"]]);
  });
  it("treats an escaped pipe as cell text", () => {
    const { blocks } = parseBlocks("| a | b |\n|---|---|\n| x \\| y | z |");
    if (blocks[0].type !== "table") throw new Error("expected table");
    expect(blocks[0].rows).toEqual([["x | y", "z"]]);
  });
  it("renders inline formatting and alignment in cells", () => {
    const out = html("| a | b |\n|---|--:|\n| `x` | **y** |");
    expect(out).toContain('class="markdown-table"');
    expect(out).toContain("<thead>");
    expect(out).toContain('<th data-align="right">');
    expect(out).toContain("<code>x</code>");
    expect(out).toContain("<strong>y</strong>");
  });
  it("ends at a blank line", () => {
    const { blocks } = parseBlocks("| a |\n|---|\n| 1 |\n\nafter");
    expect(blocks.map((b) => b.type)).toEqual(["table", "paragraph"]);
  });
});

describe("new blocks stay safe", () => {
  it("escapes html in table cells", () => {
    const out = html("| a |\n|---|\n| <img src=x onerror=alert(1)> |");
    expect(out).not.toContain("<img");
    expect(out).toContain("&lt;img");
  });
  it("rejects unsafe hrefs in table cells", () => {
    const out = html("| a |\n|---|\n| [x](javascript:alert(1)) |");
    expect(out).not.toContain("<a");
    expect(out).toContain("javascript:alert(1)");
  });
  it("never puts a fence info string in the markup", () => {
    const out = html('```a"><img src=x onerror=alert(1)>\ncode\n```');
    expect(out).not.toContain("<img");
    expect(out).not.toContain("onerror");
    expect(out).toContain('data-language="plain"');
  });
});

// ---- escapes, entities, setext, references ----
describe("backslash escapes", () => {
  it("keeps escaped punctuation literal", () => {
    expect(parseInline("\\*not italic\\*")).toEqual([
      { type: "text", text: "*not italic*" },
    ]);
    expect(parseInline("\\[not a link\\]")).toEqual([
      { type: "text", text: "[not a link]" },
    ]);
  });
  it("leaves a backslash before anything else alone", () => {
    expect(parseInline("C:\\path and \\d+")).toEqual([
      { type: "text", text: "C:\\path and \\d+" },
    ]);
  });
  it("does not let an escaped delimiter close emphasis", () => {
    expect(parseInline("*5 \\* 3*")).toEqual([{ type: "italic", text: "5 * 3" }]);
    expect(parseInline("**a \\** b**")).toEqual([{ type: "bold", text: "a ** b" }]);
  });
});

describe("character entities", () => {
  it("resolves named and numeric entities", () => {
    expect(parseInline("AT&amp;T")).toEqual([{ type: "text", text: "AT&T" }]);
    expect(parseInline("&#65;&#x42;")).toEqual([{ type: "text", text: "AB" }]);
    expect(parseInline("&mdash;")).toEqual([{ type: "text", text: "\u2014" }]);
  });
  it("leaves anything it does not know literal", () => {
    expect(parseInline("&notreal; &amp &#;")).toEqual([
      { type: "text", text: "&notreal; &amp &#;" },
    ]);
  });
  it("rejects out-of-range and surrogate code points", () => {
    expect(parseInline("&#999999999; &#xD800;")).toEqual([
      { type: "text", text: "&#999999999; &#xD800;" },
    ]);
  });
  it("leaves entities inside code spans alone", () => {
    expect(parseInline("`a &amp; b`")).toEqual([{ type: "code", text: "a &amp; b" }]);
  });
  it("an escaped ampersand stops the entity resolving", () => {
    expect(parseInline("\\&amp;")).toEqual([{ type: "text", text: "&amp;" }]);
  });
  it("a decoded angle bracket reaches the page as text", () => {
    const out = html("&lt;script&gt;alert(1)&lt;/script&gt;");
    expect(out).not.toContain("<script>");
    expect(out).toContain("&lt;script&gt;");
  });
});

describe("setext headings", () => {
  it("underlines a paragraph with = or -", () => {
    expect(parseBlocks("Title\n=====").blocks[0]).toEqual({
      type: "heading",
      level: 1,
      text: "Title",
    });
    expect(parseBlocks("Sub\n---").blocks[0]).toEqual({
      type: "heading",
      level: 2,
      text: "Sub",
    });
    expect(html("Title\n===")).toContain("<h1>");
  });
  it("takes the whole paragraph above it", () => {
    expect(parseBlocks("one\ntwo\n===").blocks[0]).toEqual({
      type: "heading",
      level: 1,
      text: "one two",
    });
  });
  it("needs a bare run, so a spaced break stays a break", () => {
    const { blocks } = parseBlocks("para\n- - -");
    expect(blocks.map((b) => b.type)).toEqual(["paragraph", "thematicBreak"]);
  });
  it("does not fire on a list or text under a paragraph", () => {
    expect(parseBlocks("para\n- item").blocks.map((b) => b.type)).toEqual([
      "paragraph",
      "bulletList",
    ]);
    expect(parseBlocks("para\n--- text").blocks[0].type).toBe("paragraph");
  });
});

describe("reference links", () => {
  it("resolves [text][label] and lifts the definition out", () => {
    const { blocks, refs } = parseBlocks("see [the docs][d]\n\n[d]: https://example.com");
    expect(blocks.length).toBe(1);
    expect(refs.get("d")).toBe("https://example.com");
    expect(html("see [the docs][d]\n\n[d]: https://example.com")).toContain(
      '<a href="https://example.com" target="_blank" rel="noopener noreferrer">the docs</a>',
    );
  });
  it("resolves the collapsed form and matches labels case-insensitively", () => {
    expect(html("[Example][]\n\n[example]: https://example.com")).toContain(
      'href="https://example.com"',
    );
  });
  it("accepts a title and an angle-bracketed destination", () => {
    expect(parseBlocks('[a][1]\n\n[1]: https://example.com "A title"').refs.get("1")).toBe(
      "https://example.com",
    );
    expect(parseBlocks("[a][1]\n\n[1]: <https://example.com>").refs.get("1")).toBe(
      "https://example.com",
    );
  });
  it("works from a definition placed above its use", () => {
    expect(html("[1]: https://example.com\n\nsee [a][1]")).toContain(
      'href="https://example.com"',
    );
  });
  it("leaves an undefined label as text", () => {
    expect(html("[a][nope] stays text")).not.toContain("<a");
  });
  it("rejects an unsafe destination", () => {
    const out = html("[x][a]\n\n[a]: javascript:alert(1)");
    expect(out).not.toContain("<a");
    expect(out).toContain("[x][a]");
  });
  it("leaves a definition inside a fence in the code", () => {
    const { blocks } = parseBlocks("```\n[1]: https://example.com\n```");
    expect(blocks[0].type).toBe("codeBlock");
    if (blocks[0].type === "codeBlock") {
      expect(blocks[0].code).toBe("[1]: https://example.com");
    }
  });
});

describe("decoded destinations are still sanitized", () => {
  it("rejects an entity-encoded scheme in an inline link", () => {
    const out = html("[x](&#106;avascript:alert&#40;1&#41;)");
    expect(out).not.toContain("<a");
  });
  it("rejects an entity-encoded scheme in a definition", () => {
    const out = html("[x][a]\n\n[a]: &#106;avascript:alert(1)");
    expect(out).not.toContain("<a");
  });
});

describe("inline scanning stays linear", () => {
  it("parses a long formatting-free paragraph in well under a frame", () => {
    // Guards the scanner against going quadratic again: probing for a token at
    // every position instead of searching forward for the next one took this
    // input tens of milliseconds.
    const line = "word ".repeat(1600);
    const started = performance.now();
    const inlines = parseInline(line);
    expect(performance.now() - started).toBeLessThan(50);
    expect(inlines).toEqual([{ type: "text", text: line }]);
  });
});
