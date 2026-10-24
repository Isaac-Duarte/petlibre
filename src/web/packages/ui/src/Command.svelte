<script lang="ts">
  import CopyButton from "./CopyButton.svelte";

  let { command, caption }: { command: string; caption?: string } = $props();
</script>

<figure class="command">
  {#if caption}<figcaption>{caption}</figcaption>{/if}
  <div class="body">
    <pre>{command}</pre>
    <CopyButton text={command} />
  </div>
</figure>

<style>
  .command {
    margin: 0;
  }
  figcaption {
    font-size: var(--text-sm);
    color: var(--color-text-muted);
    margin-bottom: 6px;
  }
  .body {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-2) var(--space-2) var(--space-3);
    background: var(--color-sunken);
    border-radius: var(--radius-control);
  }
  pre {
    flex: 1;
    min-width: 0;
    margin: 4px 0 0;
    font-size: var(--text-xs);
    line-height: 1.6;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
