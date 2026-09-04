import { describe, expect, it } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { SyntaxCode } from "./SyntaxCode";

describe("SyntaxCode", () => {
  it("uses the file extension when no language is supplied", () => {
    const markup = renderToStaticMarkup(<SyntaxCode source="const view = <Button />" path="src/App.tsx" />);

    expect(markup).toContain('data-language="tsx"');
  });

  it("escapes source instead of treating it as markup", () => {
    const markup = renderToStaticMarkup(<SyntaxCode source="<script>alert(1)</script>" language="html" />);

    expect(markup).toContain("&lt;script&gt;");
    expect(markup).not.toContain("<script>");
  });
});
