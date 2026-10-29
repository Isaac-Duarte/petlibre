// The repository's docs/ rendered to HTML at build time. The Markdown stays
// plain GitHub-flavored Markdown that reads the same on GitHub; this module is
// the one place that turns it into site pages: relative links become site
// routes or GitHub URLs, code is highlighted, and each page's headings feed
// its table of contents. It runs only during prerender, so none of it ships.
import { posix } from "node:path";
import type { Element, ElementContent, Root as Hast } from "hast";
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
import { SITE, blob, edit } from "#lib/links.ts";

const sources = import.meta.glob<string>("../../../../../../../docs/**/*.md", {
  query: "?raw",
  import: "default",
  eager: true,
});

const drawings = import.meta.glob<string>("../../../../../../../docs/**/*.svg", {
  query: "?raw",
  import: "default",
  eager: true,
});

const underDocs = (key: string) => key.replace(/^(\.\.\/)+docs\//, "");
const files = new Map(Object.entries(sources).map(([key, text]) => [underDocs(key), text]));
const svgs = new Map(Object.entries(drawings).map(([key, text]) => [underDocs(key), text]));

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

// `<!-- details -->` on a line of its own ends a page's overview. GitHub hides
// the comment and shows the whole page. Here it becomes a marked <hr>, and
// wrapDetails puts what follows in .details, which the Overview | Details
// switch hides, after a button that shows it.
const DETAILS = "<!-- details -->";

const markDetails: Plugin<[], Mdast> = () => (tree) => {
  const at = tree.children.findIndex((node) => node.type === "html" && node.value.trim() === DETAILS);
  if (at >= 0) tree.children[at] = { type: "thematicBreak", data: { hProperties: { dataDetails: "" } } };
};

const wrapDetails: Plugin<[Collected], Hast> = (out) => (tree) => {
  const at = tree.children.findIndex(
    (node) => node.type === "element" && node.tagName === "hr" && node.properties.dataDetails !== undefined,
  );
  if (at < 0) return;

  const rest = tree.children.splice(at).slice(1);
  tree.children.push(
    {
      type: "element",
      tagName: "p",
      properties: { className: ["details-more"], dataPagefindIgnore: "" },
      children: [
        {
          type: "element",
          tagName: "button",
          properties: { type: "button", dataShowDetails: "" },
          children: [{ type: "text", value: "Show the details" }],
        },
      ],
    },
    {
      type: "element",
      tagName: "div",
      properties: { className: ["details"] },
      children: rest.filter((node): node is ElementContent => node.type !== "doctype"),
    },
  );
  out.hasDetails = true;
};

// A mermaid block whose first line names its drawing, `%% svg: name.svg` (a
// Mermaid comment, so GitHub hides it), becomes that SVG beside the page.
// GitHub draws the Mermaid instead. The SVG is inlined so docs.css colors its
// classes from the tokens, and `cargo xtask check-repository` holds each pair
// to its hashes in docs/diagrams.lock.
const DRAWING = /^%% svg: (\S+\.svg)\s*$/;

function diagram(source: string, file: string): Element {
  const name = DRAWING.exec(source.split("\n", 1)[0] ?? "")?.[1];
  if (name === undefined) throw new Error(`docs/${file}: a mermaid block must start with "%% svg: <file>.svg"`);
  const path = posix.normalize(posix.join(posix.dirname(file), name));
  const svg = svgs.get(path);
  if (svg === undefined) throw new Error(`docs/${file}: its mermaid block names docs/${path}, which doesn't exist`);

  return {
    type: "element",
    tagName: "div",
    properties: { className: ["diagram"] },
    children: [{ type: "raw", value: svg.trim() }],
  };
}

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

/** The language a fenced block declares, such as `rust` for ```` ```rust ````. */
function languageOf(pre: Element): string | undefined {
  const [code] = pre.children;
  if (code?.type !== "element" || code.tagName !== "code") return undefined;
  const classes = code.properties.className;
  return Array.isArray(classes)
    ? classes
        .map(String)
        .find((name) => name.startsWith("language-"))
        ?.slice("language-".length)
    : undefined;
}

function highlight(pre: Element): Element | undefined {
  const [code] = pre.children;
  if (code?.type !== "element" || code.tagName !== "code") return undefined;
  const declared = languageOf(pre);
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
  hasDetails: boolean;
}

// After headings have ids: collect the title, the first paragraph as the
// description and the h2/h3 outline, give each heading a self-link, draw
// diagrams, highlight code, and let wide tables scroll inside the column
// instead of the page.
const decorate: Plugin<[Collected, string], Hast> = (out, file) => (tree) => {
  // .details is the root's last child and the walk is in document order, so
  // every heading after it is a detail heading.
  let inDetails = false;
  visit(tree, "element", (node, index, parent) => {
    if (
      node.tagName === "div" &&
      Array.isArray(node.properties.className) &&
      node.properties.className.includes("details")
    )
      inDetails = true;
    if (node.tagName === "h1" && out.title === "") out.title = toString(node);
    if (node.tagName === "p" && out.description === "" && parent?.type === "root")
      out.description = excerpt(toString(node));

    if (node.tagName === "h2" || node.tagName === "h3") {
      const id = typeof node.properties.id === "string" ? node.properties.id : "";
      out.headings.push({ depth: node.tagName === "h2" ? 2 : 3, id, text: toString(node), detail: inDetails });
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
      const replaced = languageOf(node) === "mermaid" ? diagram(toString(node), file) : highlight(node);
      if (replaced !== undefined) parent.children[index] = replaced;
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
  const out: Collected = { title: "", description: "", headings: [], hasDetails: false };
  const html = unified()
    .use(remarkParse)
    .use(remarkGfm)
    .use(rewriteLinks, file)
    .use(alerts)
    .use(markDetails)
    .use(remarkRehype)
    .use(rehypeSlug)
    .use(wrapDetails, out)
    .use(decorate, out, file)
    // Only the diagrams are raw HTML: remarkRehype drops any in the Markdown.
    .use(rehypeStringify, { allowDangerousHtml: true })
    .processSync(markdown);

  return {
    file,
    slug: slugOf(file),
    title: out.title || slugOf(file),
    description: out.description,
    html: String(html),
    headings: out.headings,
    hasDetails: out.hasDetails,
    editUrl: edit(`docs/${file}`),
  };
}

/**
 * A doc's Markdown as it reads away from the repository, for "Copy as
 * Markdown" and the `.md` file beside each page: every link made absolute
 * (a doc to its page on this site, anything else to GitHub) and the overview
 * marker dropped. Links are found with remark, so a `](` inside code is left
 * alone.
 */
export function portableMarkdown(file: string): string | undefined {
  const markdown = files.get(file);
  if (markdown === undefined) return undefined;

  const edits: { start: number; end: number; url: string }[] = [];
  visit(unified().use(remarkParse).use(remarkGfm).parse(markdown), (node) => {
    if (node.type !== "link" && node.type !== "definition") return;
    const start = node.position?.start.offset;
    const end = node.position?.end.offset;
    if (start === undefined || end === undefined) return;

    // The URL is the last occurrence in the node's source: after the text of
    // an inline link, after the label of a definition.
    const at = markdown.lastIndexOf(node.url, end);
    if (at < start) return;
    const resolved = resolveLink(node.url, file);
    edits.push({
      start: at,
      end: at + node.url.length,
      url: resolved.startsWith("/") ? `${SITE}${resolved}` : resolved,
    });
  });

  let out = markdown;
  for (const { start, end, url } of edits.sort((a, b) => b.start - a.start))
    out = `${out.slice(0, start)}${url}${out.slice(end)}`;
  return out.replace(/^<!-- details -->\n\n?/m, "");
}

/** Every doc, keyed by its path under docs/. */
export const docs: ReadonlyMap<string, Doc> = new Map(
  [...files].map(([file, markdown]) => [file, render(file, markdown)]),
);

export const bySlug: ReadonlyMap<string, Doc> = new Map([...docs.values()].map((doc) => [doc.slug, doc]));
