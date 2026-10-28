<script lang="ts">
  // "On this page": the h2 and h3 outline, with the section being read marked
  // once the page hydrates. The section is the last heading scrolled past the
  // top quarter of the viewport.
  import type { Heading } from "#lib/doc.ts";

  let { headings }: { headings: Heading[] } = $props();
  let current = $state("");

  $effect(() => {
    const targets = headings.map((h) => document.getElementById(h.id)).filter((el) => el !== null);
    const update = () => {
      const line = window.innerHeight / 4;
      const passed = targets.filter((el) => el.getBoundingClientRect().top < line);
      current = passed.at(-1)?.id ?? targets[0]?.id ?? "";
    };

    update();
    window.addEventListener("scroll", update, { passive: true });
    return () => {
      window.removeEventListener("scroll", update);
    };
  });
</script>

{#if headings.length > 1}
  <nav aria-labelledby="toc-title">
    <p id="toc-title">On this page</p>
    <ul>
      {#each headings as heading (heading.id)}
        <li class:sub={heading.depth === 3}>
          <a href="#{heading.id}" aria-current={heading.id === current ? "location" : undefined}>{heading.text}</a>
        </li>
      {/each}
    </ul>
  </nav>
{/if}

<style>
  p {
    margin: 0 0 var(--space-3);
    color: var(--color-text);
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
    border-left: 1px solid var(--color-line);
  }
  li.sub a {
    padding-left: calc(var(--space-3) + var(--space-3));
  }
  a {
    display: block;
    margin-left: -1px;
    padding: 5px var(--space-3);
    border-left: 1px solid transparent;
    color: var(--color-text-muted);
    font-size: var(--text-sm);
    line-height: 1.4;
    text-decoration: none;
  }
  a:hover {
    color: var(--color-text);
  }
  a[aria-current] {
    border-left-color: var(--color-accent);
    color: var(--color-accent);
    font-weight: var(--weight-medium);
  }
</style>
