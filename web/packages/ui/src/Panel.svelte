<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    title,
    meta,
    actions,
    children,
    flush = false,
    id,
  }: {
    title?: string;
    /** Quiet text beside the title: a count, a path, an age. */
    meta?: Snippet;
    actions?: Snippet;
    children: Snippet;
    /** No inner padding, for tables and logs that run edge to edge. */
    flush?: boolean;
    id?: string;
  } = $props();
</script>

<section class="panel" class:flush {id} aria-label={title}>
  {#if title || actions}
    <header>
      <h2>{title}</h2>
      {#if meta}<span class="meta">{@render meta()}</span>{/if}
      {#if actions}<div class="actions">{@render actions()}</div>{/if}
    </header>
  {/if}
  <div class="body">{@render children()}</div>
</section>

<style>
  .panel {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    min-width: 0;
  }
  header {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px var(--s-3);
    padding: var(--s-4) var(--s-5) 0;
  }
  .meta {
    color: var(--muted);
    font-size: var(--t-sm);
  }
  .actions {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: var(--s-2);
  }
  .body {
    padding: var(--s-4) var(--s-5) var(--s-5);
  }
  .flush .body {
    padding: var(--s-3) 0 0;
  }
  @media (max-width: 600px) {
    header {
      padding: var(--s-3) var(--s-4) 0;
    }
    .body {
      padding: var(--s-3) var(--s-4) var(--s-4);
    }
  }
</style>
