import * as React from "react";
import { SyntaxCode } from "@/components/SyntaxCode";
import {
  languageFromFence,
  parseBlocks,
  parseInline,
  type Block,
  type Inline,
} from "./parse";

function InlineNodes({ inlines }: { inlines: Inline[] }) {
  return (
    <>
      {inlines.map((node, idx) => {
        switch (node.type) {
          case "text":
            return <span key={idx}>{node.text}</span>;
          case "bold":
            return <strong key={idx}>{node.text}</strong>;
          case "italic":
            return <em key={idx}>{node.text}</em>;
          case "code":
            return <SyntaxCode key={idx} source={node.text} language="plain" inline />;
          case "link":
            return (
              <a key={idx} href={node.href} target="_blank" rel="noopener noreferrer">
                {node.text}
              </a>
            );
        }
      })}
    </>
  );
}

function renderBlock(block: Block, index: number): React.ReactNode {
  switch (block.type) {
    case "heading": {
      const inlines = parseInline(block.text);
      const children = <InlineNodes inlines={inlines} />;
      switch (block.level) {
        case 1:
          return <h1 key={index}>{children}</h1>;
        case 2:
          return <h2 key={index}>{children}</h2>;
        case 3:
          return <h3 key={index}>{children}</h3>;
        case 4:
          return <h4 key={index}>{children}</h4>;
        case 5:
          return <h5 key={index}>{children}</h5>;
        default:
          return <h6 key={index}>{children}</h6>;
      }
    }
    case "paragraph": {
      const inlines = parseInline(block.text);
      return (
        <p key={index}>
          <InlineNodes inlines={inlines} />
        </p>
      );
    }
    case "bulletList":
      return (
        <ul key={index}>
          {block.items.map((item, liIdx) => (
            <li key={liIdx}>
              <InlineNodes inlines={parseInline(item)} />
            </li>
          ))}
        </ul>
      );
    case "orderedList":
      return (
        <ol key={index}>
          {block.items.map((item, liIdx) => (
            <li key={liIdx}>
              <InlineNodes inlines={parseInline(item)} />
            </li>
          ))}
        </ol>
      );
    case "blockquote": {
      const inlines = parseInline(block.text);
      return (
        <blockquote key={index}>
          <InlineNodes inlines={inlines} />
        </blockquote>
      );
    }
    case "codeBlock": {
      const raw = block.language ?? "";
      const language = languageFromFence(raw);
      const className = `review-content review-language-${language}`;
      return (
        <div key={index} className={className}>
          <pre data-language={language}>
            <SyntaxCode source={block.code} language={language} />
          </pre>
        </div>
      );
    }
  }
}

export const MarkdownContent = React.memo(function MarkdownContent({ source }: { source: string }) {
  const { blocks, truncated } = parseBlocks(source);
  return (
    <div className="markdown-content">
      {blocks.map((b, idx) => renderBlock(b, idx))}
      {truncated ? <p className="review-token-comment">… truncated</p> : null}
    </div>
  );
});

export function renderMarkdownToNodes(source: string): React.ReactNode {
  const { blocks, truncated } = parseBlocks(source);
  const nodes = blocks.map((b, idx) => renderBlock(b, idx));
  if (truncated) {
    nodes.push(
      <p key="truncated" className="review-token-comment">
        … truncated
      </p>,
    );
  }
  return nodes;
}
