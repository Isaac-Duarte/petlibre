import { describe, expect, test } from "vitest";
import { docs, portableMarkdown } from "#lib/server/content.ts";

function doc(file: string) {
  const found = docs.get(file);
  if (found === undefined) throw new Error(`no doc ${file}`);
  return found;
}

describe("overviews", () => {
  test("a page's overview ends at its details marker", () => {
    const page = doc("devices/plaf103/README.md");

    expect(page.hasDetails).toBe(true);
    expect(page.html).toContain(
      '<p class="details-more" data-pagefind-ignore=""><button type="button" data-show-details="">',
    );
    expect(page.headings.find((h) => h.text === "How it feeds")?.detail).toBe(false);
    expect(page.headings.find((h) => h.text === "Sold as")?.detail).toBe(true);
  });

  test("a page without the marker shows in full", () => {
    const page = doc("firmware/motor-pulse.md");

    expect(page.hasDetails).toBe(false);
    expect(page.html).not.toContain('class="details"');
    expect(page.headings.every((h) => !h.detail)).toBe(true);
  });
});

describe("diagrams", () => {
  test("a mermaid block becomes the drawing it names, inline", () => {
    const html = doc("devices/plaf103/README.md").html;

    expect(html).toContain('<div class="diagram"><svg xmlns="http://www.w3.org/2000/svg"');
    expect(html).toContain('<title id="food-path-title">');
    expect(html).toContain('<title id="parts-title">');
    expect(html).not.toContain("language-mermaid");
  });

  test("pages in different folders draw the same file", () => {
    for (const file of ["devices/plaf103/flash-layout.md", "firmware/bank-policy.md"])
      expect(doc(file).html, file).toContain('<title id="boot-flow-title">');
  });

  test("a mermaid fence quoted in a longer one stays code", () => {
    const html = doc("development/writing.md").html;

    expect(html).toContain("%% svg: boot-flow.svg");
    expect(html).not.toContain('class="diagram"');
  });
});

describe("portable markdown", () => {
  const markdown = portableMarkdown("devices/plaf103/README.md") ?? "";

  test("links point at the site or GitHub, not the repository's files", () => {
    expect(markdown).toContain("[RTL8721DM](https://docs.well-fed.dev/chips/rtl8721dm)");
    expect(markdown).toContain("(https://github.com/Isaac-Duarte/wellfed/blob/master/src/firmware/devices/plaf103)");
    expect(markdown).not.toMatch(/\]\((?!https?:|#)/);
  });

  test("the overview marker is dropped and code is left alone", () => {
    expect(markdown).not.toContain("<!-- details -->");
    expect(markdown).toContain("%% svg: food-path.svg");
    expect(portableMarkdown("development/writing.md")).toContain("%% svg: boot-flow.svg");
  });

  test("a file that isn't a doc has none", () => {
    expect(portableMarkdown("nope.md")).toBeUndefined();
  });
});
