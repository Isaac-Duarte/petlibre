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
  {#if Boolean(title) || actions}
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
    background: var(--color-surface);
    border: 1px solid var(--color-line);
    border-radius: var(--radius-card);
    min-width: 0;
  }
  header {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px var(--space-3);
    padding: var(--space-4) var(--space-5) 0;
  }
  .meta {
    color: var(--color-text-muted);
    font-size: var(--text-sm);
  }
  .actions {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .body {
    padding: var(--space-4) var(--space-5) var(--space-5);
  }
  /* The bottom inset keeps a last row's background inside the rounded corner. */
  .flush .body {
    padding: var(--space-3) 0 var(--space-3);
  }
  @media (max-width: 600px) {
    header {
      padding: var(--space-3) var(--space-4) 0;
    }
    /* Controls take the rest of the title's line, or a line of their own,
       so a search field can grow instead of pushing the page wider. */
    .actions {
      flex: 1 1 auto;
      flex-wrap: wrap;
      justify-content: flex-end;
      min-width: 0;
    }
    .body {
      padding: var(--space-3) var(--space-4) var(--space-4);
    }
  }
</style>
