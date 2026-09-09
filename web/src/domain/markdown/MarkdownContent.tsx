import * as React from "react";
import { SyntaxCode } from "@/components/SyntaxCode";
import {
  languageFromFence,
  parseBlocks,
  parseInline,
  type Block,
  type Inline,
  type ListItem,
  type NestedList,
  type ReferenceMap,
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
          case "strike":
            return <del key={idx}>{node.text}</del>;
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

function ListItems({ items, refs }: { items: ListItem[]; refs: ReferenceMap }) {
  return (
    <>
      {items.map((item, idx) => (
        <li key={idx} data-task={item.checked === null ? undefined : "true"}>
          {item.checked === null ? null : (
            <input type="checkbox" defaultChecked={item.checked} disabled />
          )}
          <InlineNodes inlines={parseInline(item.text, refs)} />
          {item.children ? <SubList list={item.children} refs={refs} /> : null}
        </li>
      ))}
    </>
  );
}

function SubList({ list, refs }: { list: NestedList; refs: ReferenceMap }) {
  if (list.ordered) {
    return (
      <ol>
        <ListItems items={list.items} refs={refs} />
      </ol>
    );
  }
  return (
    <ul>
      <ListItems items={list.items} refs={refs} />
    </ul>
  );
}

function renderBlock(
  block: Block,
  index: number,
  refs: ReferenceMap,
): React.ReactNode {
  switch (block.type) {
    case "heading": {
      const inlines = parseInline(block.text, refs);
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
      const inlines = parseInline(block.text, refs);
      return (
        <p key={index}>
          <InlineNodes inlines={inlines} />
        </p>
      );
    }
    case "bulletList":
      return (
        <ul key={index}>
          <ListItems items={block.items} refs={refs} />
        </ul>
      );
    case "orderedList":
      return (
        <ol key={index}>
          <ListItems items={block.items} refs={refs} />
        </ol>
      );
    case "blockquote": {
      const inlines = parseInline(block.text, refs);
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
    case "thematicBreak":
      return <hr key={index} />;
    case "table":
      return (
        <div key={index} className="markdown-table">
          <table>
            <thead>
              <tr>
                {block.header.map((cell, cellIdx) => (
                  <th key={cellIdx} data-align={block.align[cellIdx] ?? undefined}>
                    <InlineNodes inlines={parseInline(cell, refs)} />
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {block.rows.map((row, rowIdx) => (
                <tr key={rowIdx}>
                  {row.map((cell, cellIdx) => (
                    <td key={cellIdx} data-align={block.align[cellIdx] ?? undefined}>
                      <InlineNodes inlines={parseInline(cell, refs)} />
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
  }
}

export const MarkdownContent = React.memo(function MarkdownContent({ source }: { source: string }) {
  const { blocks, truncated, refs } = parseBlocks(source);
  return (
    <div className="markdown-content">
      {blocks.map((b, idx) => renderBlock(b, idx, refs))}
      {truncated ? <p className="review-token-comment">… truncated</p> : null}
    </div>
  );
});

export function renderMarkdownToNodes(source: string): React.ReactNode {
  const { blocks, truncated, refs } = parseBlocks(source);
  const nodes = blocks.map((b, idx) => renderBlock(b, idx, refs));
  if (truncated) {
    nodes.push(
      <p key="truncated" className="review-token-comment">
        … truncated
      </p>,
    );
  }
  return nodes;
}
