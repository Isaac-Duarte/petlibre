<script lang="ts">
  // Full-text search over the built site. Pagefind indexes build/ after the
  // prerender and serves its index as static files from /pagefind/, so a query
  // never leaves the site's own origin. A native modal <dialog> gives the focus
  // trap and Escape for free, with no inline styles for the CSP to refuse.
  import { afterNavigate } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { onMount } from "svelte";

  interface Hit {
    /** The page's route without its leading slash, and the matching section's #anchor. */
    slug: string;
    hash: string;
    title: string;
    excerpt: string;
  }

  interface Pagefind {
    debouncedSearch: (query: string) => Promise<{ results: { data: () => Promise<unknown> }[] } | null>;
  }

  let dialog = $state<HTMLDialogElement>();
  let query = $state("");
  let hits = $state<Hit[]>([]);
  let active = $state(0);
  let status = $state<"idle" | "searching" | "unavailable">("idle");
  let pagefind: Pagefind | undefined;

  function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null;
  }

  function isPagefind(value: unknown): value is Pagefind {
    return isRecord(value) && typeof value.debouncedSearch === "function";
  }

  // Pagefind's result shape, checked rather than assumed: the deepest matching
  // heading when there is one, so a hit opens at its section.
  function toHit(data: unknown): Hit | undefined {
    if (!isRecord(data) || typeof data.url !== "string" || typeof data.excerpt !== "string") return undefined;
    const meta = data.meta;
    const title = isRecord(meta) && typeof meta.title === "string" ? meta.title : data.url;
    const sections: unknown[] = Array.isArray(data.sub_results) ? data.sub_results : [];
    const [section] = sections;
    const url = isRecord(section) && typeof section.url === "string" ? section.url : data.url;

    // Pagefind names the built files: /firmware/pairing.html#keys, /index.html.
    const [path = "", ...anchor] = url.split("#");
    const slug = path
      .replace(/^\//, "")
      .replace(/(^|\/)index\.html$/, "")
      .replace(/\.html$/, "")
      .replace(/\/$/, "");
    return { slug, hash: anchor.length > 0 ? `#${anchor.join("#")}` : "", title, excerpt: data.excerpt };
  }

  async function load(): Promise<Pagefind | undefined> {
    if (pagefind !== undefined) return pagefind;
    try {
      // The index exists only in a production build (`pagefind --site build`).
      const module: unknown = await import(/* @vite-ignore */ `${location.origin}/pagefind/pagefind.js`);
      if (isPagefind(module)) pagefind = module;
    } catch {
      status = "unavailable";
    }
    return pagefind;
  }

  async function search(text: string) {
    const engine = await load();
    if (engine === undefined) return;
    if (text.trim() === "") {
      hits = [];
      status = "idle";
      return;
    }

    status = "searching";
    const response = await engine.debouncedSearch(text);
    if (response === null) return;
    const data = await Promise.all(response.results.slice(0, 8).map((result) => result.data()));
    hits = data.map(toHit).filter((hit) => hit !== undefined);
    active = 0;
    status = "idle";
  }

  export function open() {
    if (dialog === undefined || dialog.open) return;
    dialog.showModal();
    void load();
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      active = (active + step + hits.length) % Math.max(hits.length, 1);
    } else if (event.key === "Enter") {
      // Follow the highlighted result's own link, so the router handles it.
      if (hits[active] !== undefined) document.querySelector<HTMLAnchorElement>(`#search-hit-${active} a`)?.click();
    }
  }

  // "/" and Ctrl or ⌘ K open search from anywhere but a text field.
  function onwindowkeydown(event: KeyboardEvent) {
    const typing = event.target instanceof HTMLElement && event.target.closest("input, textarea, [contenteditable]");
    if ((event.key === "k" && (event.metaKey || event.ctrlKey)) || (event.key === "/" && !typing)) {
      event.preventDefault();
      open();
    }
  }

  // The dialog opens only once the page has hydrated; until then the button
  // is disabled rather than silently doing nothing.
  let ready = $state(false);
  onMount(() => (ready = true));

  afterNavigate(() => dialog?.close());

  // Clicking the backdrop (the dialog element itself, outside its panel) closes it.
  function onclick(event: MouseEvent) {
    if (event.target === dialog) dialog.close();
  }
</script>

<svelte:window onkeydown={onwindowkeydown} />

<button
  type="button"
  class="trigger"
  onclick={open}
  disabled={!ready}
  aria-label="Search docs"
  aria-keyshortcuts="Control+K Meta+K /"
>
  <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5 14 14" /></svg>
  <span class="label">Search docs</span>
  <kbd>/</kbd>
</button>

<dialog bind:this={dialog} {onclick} aria-label="Search the docs">
  <div class="panel">
    <label class="field">
      <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5 14 14" /></svg>
      <span class="visually-hidden">Search the docs</span>
      <input
        type="search"
        placeholder="Search the docs"
        autocomplete="off"
        spellcheck="false"
        bind:value={query}
        oninput={() => void search(query)}
        {onkeydown}
        role="combobox"
        aria-expanded={hits.length > 0}
        aria-controls="search-results"
        aria-activedescendant={hits.length > 0 ? `search-hit-${active}` : undefined}
      />
      <kbd>Esc</kbd>
    </label>

    {#if status === "unavailable"}
      <p class="note">Search is built with the production site. Run <code>pnpm build</code> and preview it.</p>
    {:else if query.trim() !== "" && hits.length === 0 && status === "idle"}
      <p class="note">Nothing matches “{query}”.</p>
    {/if}

    {#if hits.length > 0}
      <ul id="search-results" role="listbox" aria-label="Results">
        {#each hits as hit, i (`${hit.slug}${hit.hash}`)}
          <li id="search-hit-{i}" role="option" aria-selected={i === active}>
            <a
              href="{resolve('/[...slug]', { slug: hit.slug })}{hit.hash}"
              class:active={i === active}
              onmouseenter={() => (active = i)}
            >
              <span class="title">{hit.title}</span>
              <!-- eslint-disable-next-line svelte/no-at-html-tags -- Pagefind's excerpt of our own built pages, with <mark> around the match -->
              <span class="excerpt">{@html hit.excerpt}</span>
            </a>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</dialog>

<style>
  .trigger {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 220px;
    height: 36px;
    padding: 0 var(--space-2) 0 var(--space-3);
    border: 1px solid var(--color-line);
    border-radius: var(--radius-pill);
    background: var(--color-surface);
    color: var(--color-text-muted);
    font-size: var(--text-base);
    cursor: pointer;
  }
  .trigger:hover {
    border-color: var(--color-line-strong);
    color: var(--color-text);
  }
  .trigger .label {
    flex: 1;
    text-align: left;
  }
  svg {
    width: 16px;
    height: 16px;
    flex: none;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
  }
  kbd {
    padding: 1px 6px;
    border: 1px solid var(--color-line);
    border-radius: 6px;
    background: var(--color-sunken);
    color: var(--color-text-muted);
    font-size: var(--text-xs);
  }
  @media (max-width: 760px) {
    .trigger {
      min-width: 0;
      width: 40px;
      height: 40px;
      justify-content: center;
      padding: 0;
    }
    .trigger .label,
    .trigger kbd {
      display: none;
    }
  }

  dialog {
    width: min(640px, calc(100vw - 2 * var(--space-4)));
    max-height: min(560px, calc(100dvh - 2 * var(--space-4)));
    margin: 12vh auto auto;
    padding: 0;
    border: 1px solid var(--color-line);
    border-radius: var(--radius-card);
    background: var(--color-surface);
    color: var(--color-text);
    box-shadow: var(--shadow-overlay);
  }
  dialog::backdrop {
    background: color-mix(in srgb, var(--color-scrim) 45%, transparent);
  }
  @media (max-width: 760px) {
    dialog {
      margin-top: var(--space-4);
    }
  }
  .panel {
    display: flex;
    flex-direction: column;
    max-height: inherit;
  }

  .field {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border-bottom: 1px solid var(--color-line);
    color: var(--color-text-muted);
  }
  input {
    flex: 1;
    min-width: 0;
    height: 40px;
    border: 0;
    background: none;
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-md);
  }
  input:focus-visible {
    box-shadow: none;
  }
  input::-webkit-search-cancel-button {
    display: none;
  }

  .note {
    margin: 0;
    padding: var(--space-5) var(--space-4);
    color: var(--color-text-muted);
    font-size: var(--text-base);
  }

  ul {
    margin: 0;
    padding: var(--space-2);
    overflow-y: auto;
    list-style: none;
  }
  ul a {
    display: block;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-control);
    color: var(--color-text);
    text-decoration: none;
  }
  ul a.active {
    background: var(--color-accent-soft);
  }
  .title {
    display: block;
    font-weight: var(--weight-semibold);
  }
  ul a.active .title {
    color: var(--color-accent);
  }
  .excerpt {
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    color: var(--color-text-muted);
    font-size: var(--text-sm);
  }
  .excerpt :global(mark) {
    background: none;
    color: var(--color-text);
    font-weight: var(--weight-semibold);
  }
</style>
