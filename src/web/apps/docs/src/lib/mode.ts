// The Overview | Details switch. A page may end its overview with
// `<!-- details -->`; the rest is wrapped in `.details`, which docs.css hides
// while the root's data-mode is "overview". HEAD_SCRIPT sets the attribute
// before first paint, so a reader who chose Details never sees a page
// collapse. Without JavaScript it is never set, and every page shows in full.
// vite.config.ts hashes HEAD_SCRIPT into the page's CSP.
export type Mode = "overview" | "details";

const KEY = "wellfed-docs-mode";

export const HEAD_SCRIPT = `document.documentElement.dataset.mode="overview";try{if(localStorage.getItem("${KEY}")==="details")document.documentElement.dataset.mode="details"}catch{}`;

/** HEAD_SCRIPT as the element the layout writes into <head>. */
export const HEAD_HTML = `<script>${HEAD_SCRIPT}</script>`;

export function currentMode(): Mode {
  return document.documentElement.dataset.mode === "details" ? "details" : "overview";
}

/** Show `mode` on every page; `remember` keeps it for later visits too. */
export function setMode(mode: Mode, remember: "remember" | "this-visit"): void {
  document.documentElement.dataset.mode = mode;
  if (remember === "this-visit") return;
  try {
    localStorage.setItem(KEY, mode);
  } catch {
    // Storage blocked (a private window, cleared site data): the choice lasts this visit.
  }
}
