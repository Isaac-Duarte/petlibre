import { defineParams } from "@sveltejs/kit/params";

// Route parameter matchers. `markdown` is a page's Markdown twin,
// `/devices/plaf103.md`; its route outranks the pages' own `[...slug]`
// because it has a matcher. The value is the segment itself, since Kit
// rebuilds the URL from it with String().
export const params = defineParams({
  markdown: (param: string) => (param.endsWith(".md") ? param : undefined),
});
