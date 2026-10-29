<script lang="ts">
  // Overview | Details, on a page whose overview ends with `<!-- details -->`.
  // The choice holds on every page and is remembered. A link into the details
  // (an outline entry, a search result, a shared URL) opens them for this
  // visit without changing what is remembered.
  import { afterNavigate } from "$app/navigation";
  import { onMount } from "svelte";
  import { currentMode, setMode, type Mode } from "#lib/mode.ts";

  let mode = $state<Mode>("overview");

  function choose(next: Mode) {
    setMode(next, "remember");
    mode = next;
  }

  function reveal() {
    const id = decodeURIComponent(location.hash.slice(1));
    const target = id === "" ? null : document.getElementById(id);
    if (target?.closest(".details") && currentMode() === "overview") {
      setMode("details", "this-visit");
      target.scrollIntoView();
    }
    mode = currentMode();
  }

  // The button that ends an overview is part of the rendered Markdown, so it
  // is reached by delegation rather than bound here.
  function showDetails(event: MouseEvent) {
    if (event.target instanceof Element && event.target.closest("[data-show-details]")) choose("details");
  }

  onMount(() => {
    reveal();
    window.addEventListener("hashchange", reveal);
    document.addEventListener("click", showDetails);
    return () => {
      window.removeEventListener("hashchange", reveal);
      document.removeEventListener("click", showDetails);
    };
  });
  afterNavigate(reveal);
</script>

<div class="mode-switch" role="group" aria-label="How much of the page to show" data-pagefind-ignore>
  <button
    type="button"
    aria-pressed={mode === "overview"}
    onclick={() => {
      choose("overview");
    }}>Overview</button
  >
  <button
    type="button"
    aria-pressed={mode === "details"}
    onclick={() => {
      choose("details");
    }}>Details</button
  >
</div>

<style>
  .mode-switch {
    display: inline-flex;
    padding: 2px;
    border: 1px solid var(--color-line);
    border-radius: var(--radius-pill);
  }
  /* Without JavaScript every page shows in full, so there is nothing to switch. */
  :global(:root:not([data-mode])) .mode-switch {
    display: none;
  }
  button {
    padding: 3px var(--space-3);
    border: 0;
    border-radius: var(--radius-pill);
    background: none;
    color: var(--color-text-muted);
    font: inherit;
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    cursor: pointer;
  }
  button:hover {
    color: var(--color-text);
  }
  button[aria-pressed="true"] {
    background: var(--color-accent-soft);
    color: var(--color-accent);
  }
  button:focus-visible {
    outline: none;
    box-shadow: var(--focus);
  }
</style>
