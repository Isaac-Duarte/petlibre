// Serves build/ as the Cloudflare Worker does (wrangler.jsonc's
// "auto-trailing-slash"): /page answers with page.html, a folder with its
// index.html. `vite preview` can't stand in for it, since it serves
// SvelteKit's own output and not the Pagefind index written into build/.
import { createReadStream, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";

const root = join(import.meta.dirname, "..", "build");
const port = Number(process.env.PORT ?? 4173);
const host = process.env.HOST ?? "127.0.0.1";

const TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".wasm": "application/wasm",
  ".txt": "text/plain; charset=utf-8",
};

function isFile(path: string): boolean {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

function resolve(pathname: string): string | undefined {
  const path = join(root, normalize(decodeURIComponent(pathname)));
  if (!path.startsWith(root)) return undefined;
  return [path, `${path}.html`, join(path, "index.html")].find(isFile);
}

createServer((request, response) => {
  const file = resolve(new URL(request.url ?? "/", "http://localhost").pathname);
  if (file === undefined) {
    response.writeHead(404).end("Not found");
    return;
  }
  response.writeHead(200, { "content-type": TYPES[extname(file)] ?? "application/octet-stream" });
  createReadStream(file).pipe(response);
}).listen(port, host, () => {
  console.log(`serving build/ on http://${host}:${port}/`);
});
