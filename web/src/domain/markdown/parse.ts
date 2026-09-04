export const MAX_MARKDOWN_CHARS = 20_000;
export const MAX_BLOCKS = 400;

export type Inline =
  | { type: "text"; text: string }
  | { type: "bold"; text: string }
  | { type: "italic"; text: string }
  | { type: "code"; text: string }
  | { type: "link"; text: string; href: string };

export type Block =
  | { type: "heading"; level: number; text: string }
  | { type: "paragraph"; text: string }
  | { type: "bulletList"; items: string[] }
  | { type: "orderedList"; items: string[] }
  | { type: "blockquote"; text: string }
  | { type: "codeBlock"; language: string | null; code: string };

export type CodeLanguage =
  | "plain"
  | "rust"
  | "typescript"
  | "tsx"
  | "javascript"
  | "json"
  | "markdown"
  | "yaml"
  | "toml"
  | "shell"
  | "python"
  | "css"
  | "html"
  | "sql"
  | "swift"
  | "go"
  | "ruby"
  | "java"
  | "kotlin"
  | "php"
  | "c"
  | "cpp"
  | "csharp";

export function sanitizeHref(raw: string): string | null {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return null;
  const lower = trimmed.toLowerCase();
  if (
    lower.startsWith("javascript:") ||
    lower.startsWith("data:") ||
    lower.startsWith("vbscript:")
  ) {
    return null;
  }
  return trimmed;
}

export function languageFromFence(info: string): CodeLanguage {
  const lang = info.trim().toLowerCase();
  const first = lang.split(/\s+/)[0] ?? "";
  switch (first) {
    case "rs":
    case "rust":
      return "rust";
    case "ts":
    case "typescript":
      return "typescript";
    case "tsx":
    case "jsx":
      return "tsx";
    case "js":
    case "mjs":
    case "cjs":
    case "javascript":
      return "javascript";
    case "json":
    case "jsonl":
      return "json";
    case "yml":
    case "yaml":
      return "yaml";
    case "toml":
      return "toml";
    case "sh":
    case "bash":
    case "zsh":
    case "fish":
    case "shell":
      return "shell";
    case "py":
    case "python":
      return "python";
    case "css":
    case "scss":
      return "css";
    case "html":
    case "htm":
      return "html";
    case "sql":
      return "sql";
    case "swift":
      return "swift";
    case "go":
      return "go";
    case "rb":
    case "ruby":
      return "ruby";
    case "java":
      return "java";
    case "kt":
    case "kts":
    case "kotlin":
      return "kotlin";
    case "php":
      return "php";
    case "c":
      return "c";
    case "cpp":
    case "c++":
    case "cc":
    case "cxx":
      return "cpp";
    case "cs":
    case "csharp":
      return "csharp";
    case "md":
    case "markdown":
      return "markdown";
    case "":
      return "plain";
    default:
      return "plain";
  }
}

export function parseInline(text: string): Inline[] {
  const chars = [...text];
  const out: Inline[] = [];
  let i = 0;

  function autolinkAt(index: number): { href: string; end: number } | null {
    const match = chars.slice(index).join("").match(/^https?:\/\/[^\s<]+/i);
    if (!match) return null;

    let href = match[0];
    while (/[.,)>]$/.test(href)) href = href.slice(0, -1);
    if (href.length === 0) return null;
    return { href, end: index + [...href].length };
  }

  while (i < chars.length) {
    // inline code `code`
    if (chars[i] === "`") {
      const rest = chars.slice(i + 1);
      const pos = rest.indexOf("`");
      if (pos !== -1) {
        const inner = chars.slice(i + 1, i + 1 + pos).join("");
        out.push({ type: "code", text: inner });
        i += pos + 2;
        continue;
      }
    }

    const autolink = autolinkAt(i);
    if (autolink) {
      out.push({ type: "link", text: autolink.href, href: autolink.href });
      i = autolink.end;
      continue;
    }

    // link [text](url)
    if (chars[i] === "[") {
      const afterOpen = chars.slice(i + 1);
      const closeBracket = afterOpen.indexOf("]");
      if (closeBracket !== -1) {
        const textEnd = i + 1 + closeBracket;
        if (textEnd + 1 < chars.length && chars[textEnd + 1] === "(") {
          const afterParen = chars.slice(textEnd + 2);
          const closeParen = afterParen.indexOf(")");
          if (closeParen !== -1) {
            const linkText = chars.slice(i + 1, textEnd).join("");
            const hrefRaw = chars
              .slice(textEnd + 2, textEnd + 2 + closeParen)
              .join("");
            const href = sanitizeHref(hrefRaw);
            if (href !== null) {
              out.push({ type: "link", text: linkText, href });
              i = textEnd + 2 + closeParen + 1;
              continue;
            }
          }
        }
      }
    }

    // bold ** or __
    if (
      i + 1 < chars.length &&
      ((chars[i] === "*" && chars[i + 1] === "*") ||
        (chars[i] === "_" && chars[i + 1] === "_"))
    ) {
      const delim = chars[i];
      let j = i + 2;
      let found: number | null = null;
      while (j + 1 < chars.length) {
        if (chars[j] === delim && chars[j + 1] === delim) {
          found = j;
          break;
        }
        j += 1;
      }
      if (found !== null) {
        const inner = chars.slice(i + 2, found).join("");
        if (inner.length !== 0 && !inner.includes("\n")) {
          out.push({ type: "bold", text: inner });
          i = found + 2;
          continue;
        }
      }
    }

    // italic * or _
    if (chars[i] === "*" || chars[i] === "_") {
      const delim = chars[i];
      let j = i + 1;
      let found: number | null = null;
      while (j < chars.length) {
        if (chars[j] === delim) {
          const prevIsSame = j > 0 && chars[j - 1] === delim;
          const nextIsSame =
            j + 1 < chars.length && chars[j + 1] === delim;
          if (!prevIsSame && !nextIsSame) {
            found = j;
            break;
          }
        }
        if (chars[j] === "\n") break;
        j += 1;
      }
      if (found !== null) {
        const inner = chars.slice(i + 1, found).join("");
        if (inner.length !== 0 && !inner.includes("\n")) {
          out.push({ type: "italic", text: inner });
          i = found + 1;
          continue;
        }
      }
    }

    // plain text until next special
    const start = i;
    i += 1;
    while (
      i < chars.length &&
      chars[i] !== "`" &&
      chars[i] !== "[" &&
      chars[i] !== "*" &&
      chars[i] !== "_" &&
      !autolinkAt(i)
    ) {
      i += 1;
    }
    const plain = chars.slice(start, i).join("");
    const last = out[out.length - 1];
    if (last && last.type === "text") {
      last.text += plain;
    } else {
      out.push({ type: "text", text: plain });
    }
  }

  return out;
}

function headingLevel(line: string): { level: number; text: string } | null {
  const trimmed = line.trimStart();
  let count = 0;
  for (const c of trimmed) {
    if (c === "#" && count < 6) count += 1;
    else break;
  }
  if (count === 0) return null;
  const rest = trimmed.slice(count);
  if (rest.startsWith(" ") || rest.startsWith("\t")) {
    return { level: count, text: rest.trimStart() };
  }
  if (rest.length === 0) {
    return { level: count, text: "" };
  }
  return null;
}

function isBullet(line: string): string | null {
  const t = line.trimStart();
  if (t.startsWith("- ")) return t.slice(2);
  if (t.startsWith("* ")) return t.slice(2);
  return null;
}

function orderedItem(line: string): string | null {
  const t = line.trimStart();
  let idx = 0;
  for (const c of t) {
    if (c >= "0" && c <= "9") idx += 1;
    else break;
  }
  if (idx === 0 || idx > 9) return null;
  const rest = t.slice(idx);
  if (rest.startsWith(". ")) return rest.slice(2);
  return null;
}

function isBlockquote(line: string): string | null {
  const t = line.trimStart();
  if (t.startsWith("> ")) return t.slice(2);
  if (t === ">") return "";
  return null;
}

export function parseBlocks(source: string): {
  blocks: Block[];
  truncated: boolean;
} {
  const chars = [...source];
  const truncated = chars.length > MAX_MARKDOWN_CHARS;
  const limited = chars.slice(0, MAX_MARKDOWN_CHARS).join("");
  const lines = limited.split(/\r?\n/);
  // Rust `lines()` drops a trailing empty after a final newline; `split` keeps it. Drop one trailing "".
  if (limited.endsWith("\n") && lines[lines.length - 1] === "") {
    lines.pop();
  }

  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length && blocks.length < MAX_BLOCKS) {
    const line = lines[i];
    const trimmed = line.trim();
    if (trimmed.length === 0) {
      i += 1;
      continue;
    }

    // fenced code
    if (trimmed.startsWith("```")) {
      const infoRaw = trimmed.slice(3);
      const info = infoRaw.trim();
      const language = info.length === 0 ? null : info;
      const codeLines: string[] = [];
      i += 1;
      while (i < lines.length) {
        if (lines[i].trim().startsWith("```")) {
          i += 1;
          break;
        }
        codeLines.push(lines[i]);
        i += 1;
      }
      blocks.push({ type: "codeBlock", language, code: codeLines.join("\n") });
      continue;
    }

    const heading = headingLevel(line);
    if (heading) {
      blocks.push({ type: "heading", level: heading.level, text: heading.text });
      i += 1;
      continue;
    }

    const bq = isBlockquote(line);
    if (bq !== null) {
      const parts: string[] = [bq];
      i += 1;
      while (i < lines.length) {
        const nxt = isBlockquote(lines[i]);
        if (nxt !== null) {
          parts.push(nxt);
          i += 1;
        } else break;
      }
      blocks.push({ type: "blockquote", text: parts.join("\n") });
      continue;
    }

    if (isBullet(line) !== null) {
      const items: string[] = [];
      while (i < lines.length) {
        const item = isBullet(lines[i]);
        if (item !== null) {
          items.push(item);
          i += 1;
        } else break;
        if (items.length >= MAX_BLOCKS) break;
      }
      blocks.push({ type: "bulletList", items });
      continue;
    }

    if (orderedItem(line) !== null) {
      const items: string[] = [];
      while (i < lines.length) {
        const item = orderedItem(lines[i]);
        if (item !== null) {
          items.push(item);
          i += 1;
        } else break;
        if (items.length >= MAX_BLOCKS) break;
      }
      blocks.push({ type: "orderedList", items });
      continue;
    }

    // paragraph: collect consecutive non-block lines
    const para: string[] = [line.trim()];
    i += 1;
    while (i < lines.length) {
      const nxt = lines[i];
      if (
        nxt.trim().length === 0 ||
        headingLevel(nxt) !== null ||
        nxt.trim().startsWith("```") ||
        isBlockquote(nxt) !== null ||
        isBullet(nxt) !== null ||
        orderedItem(nxt) !== null
      ) {
        break;
      }
      para.push(nxt.trim());
      i += 1;
    }
    blocks.push({ type: "paragraph", text: para.join(" ") });
  }

  return { blocks, truncated };
}
