import { error } from "@sveltejs/kit";
import { markdownPath } from "#lib/links.ts";
import { bySlug, portableMarkdown } from "#lib/server/content.ts";
import type { EntryGenerator, RequestHandler } from "./$types";

// Each page's Markdown, prerendered beside it, for "Copy as Markdown" and for
// anyone (or any agent) that would rather read the source than the page.
// An endpoint doesn't inherit the layout's prerender.
export const prerender = true;
export const entries: EntryGenerator = () => [...bySlug.keys()].map((slug) => ({ file: markdownPath(slug).slice(1) }));

export const GET: RequestHandler = ({ params }) => {
  const slug = params.file.replace(/\.md$/, "");
  const doc = bySlug.get(slug === "index" ? "" : slug);
  const markdown = doc === undefined ? undefined : portableMarkdown(doc.file);
  if (markdown === undefined) error(404, "No such page");

  return new Response(markdown, { headers: { "content-type": "text/markdown; charset=utf-8" } });
};
