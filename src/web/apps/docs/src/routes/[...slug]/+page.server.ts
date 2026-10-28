import { error } from "@sveltejs/kit";
import { bySlug } from "#lib/server/content.ts";
import { placeOf } from "#lib/server/site.ts";
import type { EntryGenerator, PageServerLoad } from "./$types";

// Every doc is a page, even one the sidebar leaves out: links reach it.
export const entries: EntryGenerator = () => [...bySlug.keys()].map((slug) => ({ slug }));

export const load: PageServerLoad = ({ params }) => {
  const doc = bySlug.get(params.slug);
  if (doc === undefined) error(404, "No such page");

  return { doc, ...placeOf(doc.slug) };
};
