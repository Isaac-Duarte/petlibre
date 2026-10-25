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
    if (typeof value === "string") return value;
    if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") return value.toString();
    return JSON.stringify(value);
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
    grid-template-columns: fit-content(50%) minmax(0, 1fr);
    gap: 4px var(--space-4);
    margin: 0;
    font-size: var(--text-sm);
  }
  dt {
    color: var(--color-text-muted);
    overflow-wrap: anywhere;
  }
  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .mono dd {
    font-family: var(--mono);
    font-size: var(--text-xs);
    line-height: 1.7;
  }
</style>
