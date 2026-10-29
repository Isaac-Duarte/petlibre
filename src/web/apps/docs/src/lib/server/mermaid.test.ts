// @vitest-environment jsdom
// Every mermaid block in docs/ must parse, since GitHub draws it in place of
// the SVG the site shows. The site never runs Mermaid, so nothing else would
// notice a block that GitHub can't draw. Blocks are found with remark, as the
// site finds them, so a fence quoted inside a longer one stays an example.
import remarkParse from "remark-parse";
import { unified } from "unified";
import { visit } from "unist-util-visit";
import { describe, expect, test } from "vitest";

const sources = import.meta.glob<string>("../../../../../../../docs/**/*.md", {
  query: "?raw",
  import: "default",
  eager: true,
});

const blocks = Object.entries(sources).flatMap(([key, markdown]) => {
  const found: { page: string; source: string }[] = [];
  visit(unified().use(remarkParse).parse(markdown), "code", (node) => {
    if (node.lang === "mermaid") found.push({ page: key.replace(/^(\.\.\/)+/, ""), source: node.value });
  });
  return found;
});

describe("mermaid", () => {
  test("docs/ has diagrams to check", () => {
    expect(blocks.length).toBeGreaterThan(0);
  });

  test.each(blocks)("$page parses", async ({ source }) => {
    const { default: mermaid } = await import("mermaid");
    mermaid.initialize({ startOnLoad: false });

    await expect(mermaid.parse(source)).resolves.toBeTruthy();
  });
});
