// `pnpm generate`: write tokens.gen.css from assets/brand/tokens.json, or
// refuse when a named pair misses its contrast minimum, so a bad value fails
// the build rather than shipping.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parse, problems, render } from "./tokens.ts";

const SOURCE = join(import.meta.dirname, "../../../../../assets/brand/tokens.json");
const OUTPUT = join(import.meta.dirname, "tokens.gen.css");

const tokens = parse(JSON.parse(readFileSync(SOURCE, "utf8")));
const found = problems(tokens);
if (found.length > 0) {
  for (const problem of found) console.error(problem);
  process.exit(1);
}
writeFileSync(OUTPUT, render(tokens));
