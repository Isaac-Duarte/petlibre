// The repository's docs/ rendered to HTML at build time. The Markdown stays
// plain GitHub-flavored Markdown that reads the same on GitHub; this module is
// the one place that turns it into site pages: relative links become site
// routes or GitHub URLs, code is highlighted, and each page's headings feed
// its table of contents. It runs only during prerender, so none of it ships.
import { posix } from "node:path";
import type { Element, Root as Hast } from "hast";
import { toString } from "hast-util-to-string";
import type { Root as Mdast } from "mdast";
import rehypeSlug from "rehype-slug";
import rehypeStringify from "rehype-stringify";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { createCssVariablesTheme, createHighlighter, type ShikiTransformer } from "shiki";
import { unified, type Plugin } from "unified";
import { SKIP, visit } from "unist-util-visit";
import type { Doc, Heading } from "#lib/doc.ts";
import { blob, edit } from "#lib/links.ts";

const sources = import.meta.glob<string>("../../../../../../../docs/**/*.md", {
  query: "?raw",
  import: "default",
  eager: true,
});

const files = new Map(Object.entries(sources).map(([key, text]) => [key.replace(/^(\.\.\/)+docs\//, ""), text]));

/** `firmware/README.md` → `firmware`, `README.md` → the root. */
export function slugOf(file: string): string {
  return file.replace(/\.md$/, "").replace(/(^|\/)README$/, "");
}

/**
 * A link as written in docs/ is relative to the file and works on GitHub.
 * On the site, a link to another doc becomes its route (its #anchor kept, and
 * checked by the prerender crawl), and a link anywhere else in the repository
 * becomes that file on GitHub.
 */
export function resolveLink(url: string, file: string): string {
  if (url === "" || url.startsWith("#") || /^[a-z][a-z\d+.-]*:/i.test(url)) return url;

  const [target = "", ...rest] = url.split("#");
  const hash = rest.length > 0 ? `#${rest.join("#")}` : "";
  const path = posix.normalize(posix.join("docs", posix.dirname(file), target));

  if (path.startsWith("docs/")) {
    const inDocs = path.slice("docs/".length);
    const folderReadme = posix.join(inDocs, "README.md");
    if (inDocs.endsWith(".md")) return `/${slugOf(inDocs)}${hash}`;
    if (files.has(folderReadme)) return `/${slugOf(folderReadme)}${hash}`;
  }
  return `${blob(path.replace(/\/$/, ""))}${hash}`;
}

const rewriteLinks: Plugin<[string], Mdast> = (file) => (tree) => {
  visit(tree, (node) => {
    if (node.type === "link" || node.type === "definition") node.url = resolveLink(node.url, file);
  });
};

// GitHub's alert syntax, `> [!WARNING]` as a blockquote's first line, becomes
// an <aside> with its kind as a title, as GitHub renders it.
const ALERTS = { NOTE: "Note", TIP: "Tip", IMPORTANT: "Important", WARNING: "Warning", CAUTION: "Caution" };
const ALERT = /^\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*/;

function isAlertKind(kind: string): kind is keyof typeof ALERTS {
  return kind in ALERTS;
}

const alerts: Plugin<[], Mdast> = () => (tree) => {
  visit(tree, "blockquote", (node) => {
    const [first] = node.children;
    if (first?.type !== "paragraph") return;
    const [lead] = first.children;
    if (lead?.type !== "text") return;
    const kind = ALERT.exec(lead.value)?.[1];
    if (kind === undefined || !isAlertKind(kind)) return;

    lead.value = lead.value.replace(ALERT, "");
    if (lead.value === "") first.children.shift();
    if (first.children.length === 0) node.children.shift();
    node.data = { hName: "aside", hProperties: { className: ["alert", `alert-${kind.toLowerCase()}`] } };
    node.children.unshift({
      type: "paragraph",
      data: { hProperties: { className: ["alert-title"] } },
      children: [{ type: "text", value: ALERTS[kind] }],
    });
  });
};

// One theme whose colors are CSS variables, so code follows the page's light
// or dark tokens. The transformer turns each inline color into a class, since
// the site's CSP allows no inline styles; docs.css colors the classes.
const theme = createCssVariablesTheme({ name: "wellfed", variablePrefix: "--code-" });
const highlighter = await createHighlighter({
  themes: [theme],
  langs: ["shellscript", "rust", "toml", "json", "markdown", "http"],
});
const languages = new Set(highlighter.getLoadedLanguages());

const tokenClasses: ShikiTransformer = {
  pre(node) {
    delete node.properties.style;
    delete node.properties.tabindex;
  },
  span(node) {
    const style = node.properties.style;
    delete node.properties.style;
    const token = typeof style === "string" ? /var\(--code-(?:token-)?([a-z-]+)\)/.exec(style)?.[1] : undefined;
    if (token !== undefined && token !== "foreground") this.addClassToHast(node, `tok-${token}`);
  },
};

function highlight(pre: Element): Element | undefined {
  const [code] = pre.children;
  if (code?.type !== "element" || code.tagName !== "code") return undefined;
  const classes = code.properties.className;
  const declared = Array.isArray(classes)
    ? classes
        .map(String)
        .find((name) => name.startsWith("language-"))
        ?.slice("language-".length)
    : undefined;
  const lang = declared !== undefined && languages.has(declared) ? declared : "text";

  const root = highlighter.codeToHast(toString(code).replace(/\n$/, ""), {
    lang,
    theme: "wellfed",
    transformers: [tokenClasses],
  });
  const [highlighted] = root.children;
  if (highlighted?.type !== "element") return undefined;
  if (declared !== undefined) highlighted.properties.dataLang = declared;
  // Diagrams and plain output read as noise in a search excerpt.
  if (lang === "text") highlighted.properties.dataPagefindIgnore = "";
  return highlighted;
}

function excerpt(text: string): string {
  const flat = text.replace(/\s+/g, " ").trim();
  if (flat.length <= 160) return flat;
  return `${flat.slice(0, flat.lastIndexOf(" ", 157))}…`;
}

interface Collected {
  title: string;
  description: string;
  headings: Heading[];
}

// After headings have ids: collect the title, the first paragraph as the
// description and the h2/h3 outline, give each heading a self-link, highlight
// code, and let wide tables scroll inside the column instead of the page.
const decorate: Plugin<[Collected], Hast> = (out) => (tree) => {
  visit(tree, "element", (node, index, parent) => {
    if (node.tagName === "h1" && out.title === "") out.title = toString(node);
    if (node.tagName === "p" && out.description === "" && parent?.type === "root")
      out.description = excerpt(toString(node));

    if (node.tagName === "h2" || node.tagName === "h3") {
      const id = typeof node.properties.id === "string" ? node.properties.id : "";
      out.headings.push({ depth: node.tagName === "h2" ? 2 : 3, id, text: toString(node) });
      node.children.push({
        type: "element",
        tagName: "a",
        properties: {
          className: ["anchor"],
          href: `#${id}`,
          ariaLabel: "Link to this section",
          dataPagefindIgnore: "",
        },
        children: [{ type: "text", value: "#" }],
      });
      return SKIP;
    }

    if (parent === undefined || index === undefined) return undefined;

    if (node.tagName === "pre") {
      const highlighted = highlight(node);
      if (highlighted !== undefined) parent.children[index] = highlighted;
      return SKIP;
    }

    if (node.tagName === "table") {
      parent.children[index] = {
        type: "element",
        tagName: "div",
        properties: { className: ["table-scroll"] },
        children: [node],
      };
      return SKIP;
    }

    return undefined;
  });
};

function render(file: string, markdown: string): Doc {
  const out: Collected = { title: "", description: "", headings: [] };
  const html = unified()
    .use(remarkParse)
    .use(remarkGfm)
    .use(rewriteLinks, file)
    .use(alerts)
    .use(remarkRehype)
    .use(rehypeSlug)
    .use(decorate, out)
    .use(rehypeStringify)
    .processSync(markdown);

  return {
    file,
    slug: slugOf(file),
    title: out.title || slugOf(file),
    description: out.description,
    html: String(html),
    headings: out.headings,
    editUrl: edit(`docs/${file}`),
  };
}

/** Every doc, keyed by its path under docs/. */
export const docs: ReadonlyMap<string, Doc> = new Map(
  [...files].map(([file, markdown]) => [file, render(file, markdown)]),
);

export const bySlug: ReadonlyMap<string, Doc> = new Map([...docs.values()].map((doc) => [doc.slug, doc]));
