<script lang="ts">
  let {
    entries,
    mono = true,
  }: {
    entries: [string, unknown][];
    /** Values in the monospace face: hashes, paths and counts. */
    mono?: boolean;
  } = $props();

  function show(value: unknown): string {
    if (value === null || value === undefined) return "–";
    return typeof value === "object" ? JSON.stringify(value) : String(value);
  }
</script>

<dl class:mono>
  {#each entries as [key, value] (key)}
    <dt>{key}</dt>
    <dd>{show(value)}</dd>
  {/each}
</dl>

<style>
  dl {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 4px var(--s-4);
    margin: 0;
    font-size: var(--t-sm);
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .mono dd {
    font-family: var(--mono);
    font-size: var(--t-xs);
    line-height: 1.7;
  }
</style>
