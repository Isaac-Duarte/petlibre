import { markdownPath } from "#lib/links.ts";

/** A page's Markdown, from the `.md` file prerendered beside it. */
export async function loadMarkdown(slug: string): Promise<string> {
  const response = await fetch(markdownPath(slug));
  if (!response.ok) throw new Error(`${markdownPath(slug)}: ${String(response.status)}`);
  return response.text();
}
