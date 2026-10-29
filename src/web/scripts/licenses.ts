// Every installed package, dev tools included, must carry a license on this
// list: deny.toml's permissive set for Rust, plus the permissive licenses
// common on npm (0BSD, BlueOak, and CC-BY-4.0 for data such as browser
// tables, and CC0-1.0, a public-domain dedication more permissive still, for
// jsdom's mdn-data), MIT-0 (MIT without the notice requirement, in jsdom's
// CSS packages), and OFL-1.1 for the bundled Inter and JetBrains Mono fonts.
// A new
// license is a policy change, made here with a reason
// (docs/development/typescript.md#dependencies).
import { execFileSync } from "node:child_process";

const ALLOWED = new Set([
  "0BSD",
  "Apache-2.0",
  "BlueOak-1.0.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "CC-BY-4.0",
  "CC0-1.0",
  "ISC",
  "MIT",
  "MIT-0",
  "MPL-2.0",
  "OFL-1.1",
  "Unlicense",
  "Zlib",
]);

// Exact versions whose package.json declares no license, so pnpm reports
// "Unknown", each with the license its own license file grants. A new
// version is unlabeled again until checked.
const UNLABELED = new Map([
  // Mermaid's color library; its `license` file is MIT.
  ["khroma@2.1.0", "MIT"],
]);

interface Entry {
  name: string;
  versions: string[];
}

function isEntries(value: unknown): value is Record<string, Entry[]> {
  return value !== null && typeof value === "object" && Object.values(value).every(Array.isArray);
}

// `pnpm licenses list` reads what is installed, so this runs after an install;
// `-r` covers every workspace package, not only the root's tools.
const output = execFileSync("pnpm", ["licenses", "list", "--json", "-r"], { encoding: "utf8" });
const listing: unknown = JSON.parse(output);
if (!isEntries(listing)) throw new Error("pnpm licenses list printed an unexpected shape");

// An SPDX expression such as "(MIT OR Apache-2.0)" passes when one of its
// alternatives is allowed; "AND" needs every part.
function allowed(license: string): boolean {
  const bare = license.replace(/^\((.*)\)$/, "$1");
  if (bare.includes(" AND ")) return bare.split(" AND ").every(allowed);
  return bare.split(" OR ").some((part) => ALLOWED.has(part.trim()));
}

const refused = Object.entries(listing).flatMap(([license, entries]) =>
  entries
    .flatMap((entry) => entry.versions.map((version) => `${entry.name}@${version}`))
    .filter((id) => !allowed(license === "Unknown" ? (UNLABELED.get(id) ?? license) : license))
    .map((id) => `${id}: ${license}`),
);
for (const line of refused) console.error(line);

const count = Object.values(listing).reduce((sum, entries) => sum + entries.length, 0);
if (refused.length > 0) {
  console.error("These licenses are not on the allowlist in scripts/licenses.ts.");
  process.exit(1);
}
console.log(`licenses ok: ${count} packages`);
