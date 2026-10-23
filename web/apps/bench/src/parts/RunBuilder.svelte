<script lang="ts">
  // Compose the next run's command from the catalog. The page writes the
  // command out and nothing more: a run powers, flashes over the air and
  // brings up the hotspot, so the operator starts it in a terminal.
  import { Badge, Command, Panel } from "@wellfed/ui";
  import type { CatalogEntry, Tier } from "@wellfed/api";
  import { includes, itestCommand, missingFlags, nextRunName, type RunOptions } from "../lib/tests";

  let { catalog, runNames }: { catalog: CatalogEntry[]; runNames: string[] } = $props();

  const suggested = $derived(nextRunName(runNames, new Date()));
  let name = $state("");
  let options = $state<Omit<RunOptions, "name">>({
    tier: "standard",
    only: [],
    power: false,
    build: false,
    destructive: false,
    keepGoing: false,
  });

  const full = $derived({ ...options, name: name.trim() || suggested });
  const command = $derived(itestCommand(full));
  const skipped = $derived(
    catalog.filter((entry) => includes(entry, full)).flatMap((entry) => {
      const missing = missingFlags(entry, full);
      return missing.length ? [{ name: entry.name, missing }] : [];
    }),
  );

  const TIERS: { tier: Tier; label: string }[] = [
    { tier: "smoke", label: "Smoke" },
    { tier: "standard", label: "Standard" },
    { tier: "soak", label: "Soak" },
  ];

  function toggle(scenario: string) {
    options.only = options.only.includes(scenario) ? options.only.filter((s) => s !== scenario) : [...options.only, scenario];
  }

  // --destructive requires --power, as the CLI does.
  $effect(() => {
    if (options.destructive && !options.power) options.power = true;
  });
</script>

<Panel title="Start a run">
  {#snippet meta()}compose the command; run it in a terminal on the bench host{/snippet}

  <div class="controls">
    <label class="field">
      <span>Name</span>
      <input type="text" bind:value={name} placeholder={suggested} spellcheck="false" autocomplete="off" />
    </label>

    <div class="field">
      <span id="tier-label">Tier</span>
      <div class="segmented" role="radiogroup" aria-labelledby="tier-label">
        {#each TIERS as t (t.tier)}
          <button
            type="button"
            role="radio"
            aria-checked={!options.only.length && options.tier === t.tier}
            class:on={!options.only.length && options.tier === t.tier}
            onclick={() => {
              options.tier = t.tier;
              options.only = [];
            }}>{t.label}</button
          >
        {/each}
        {#if options.only.length}<span class="only">{options.only.length} selected</span>{/if}
      </div>
    </div>

    <fieldset class="flags">
      <legend>Allow</legend>
      <label><input type="checkbox" bind:checked={options.build} /> <code>--build</code> variants from this checkout</label>
      <label><input type="checkbox" bind:checked={options.power} /> <code>--power</code> cold boots</label>
      <label><input type="checkbox" bind:checked={options.destructive} /> <code>--destructive</code> power cuts mid-update</label>
      <label><input type="checkbox" bind:checked={options.keepGoing} /> <code>--keep-going</code> past a failure</label>
    </fieldset>
  </div>

  <table class="catalog">
    <thead><tr><th><span class="sr">Include</span></th><th>Scenario</th><th>Tier</th><th>Needs</th></tr></thead>
    <tbody>
      {#each catalog as entry (entry.name)}
        {@const included = includes(entry, full)}
        {@const missing = included ? missingFlags(entry, full) : []}
        <tr class:included class:short={missing.length > 0}>
          <td>
            <input
              type="checkbox"
              checked={entry.baseline || options.only.includes(entry.name)}
              disabled={entry.baseline}
              onchange={() => toggle(entry.name)}
              aria-label="Only {entry.name}"
              title={entry.baseline ? "Installs the build under test; every run starts with it" : `Run only the checked scenarios`}
            />
          </td>
          <td>
            <span class="name">{entry.name}</span>
            <span class="about">{entry.about}</span>
          </td>
          <td><Badge tone="muted" label={entry.tier} /></td>
          <td class="needs">
            {#each entry.needs as need (need.flag)}
              <code class:missing={missing.includes(need.flag)} title={need.why}>{need.flag}</code>
            {/each}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>

  {#if skipped.length}
    <p class="warning">
      Without their flags, these will be skipped, never passed:
      {#each skipped as s, i (s.name)}{i ? ", " : ""}<code>{s.name}</code> ({s.missing.join(" ")}){/each}.
    </p>
  {/if}

  <Command command={command} />
</Panel>

<style>
  .controls {
    display: grid;
    grid-template-columns: minmax(180px, 260px) auto minmax(0, 1fr);
    gap: var(--s-4) var(--s-5);
    align-items: start;
    margin-bottom: var(--s-4);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: var(--t-sm);
    color: var(--muted);
  }
  input[type="text"] {
    height: 34px;
    padding: 0 var(--s-3);
    border: 1px solid var(--line-strong);
    border-radius: var(--pill);
    background: var(--surface);
    color: var(--ink);
    font: var(--t-sm) var(--mono);
  }
  input[type="text"]:focus {
    outline: none;
    box-shadow: var(--focus);
  }
  .segmented {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 3px;
    background: var(--sunken);
    border-radius: var(--pill);
    width: max-content;
  }
  .segmented button {
    height: 28px;
    padding: 0 14px;
    border: 0;
    border-radius: var(--pill);
    background: none;
    color: var(--muted);
    cursor: pointer;
    font-size: var(--t-sm);
    font-weight: 500;
  }
  .segmented button.on {
    background: var(--surface);
    color: var(--ink);
    box-shadow: 0 0 0 1px var(--line);
  }
  .only {
    padding: 0 10px;
    color: var(--accent);
    font-size: var(--t-xs);
    font-weight: 600;
  }
  .flags {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(270px, 1fr));
    gap: 6px var(--s-4);
    margin: 0;
    padding: 0;
    border: 0;
    font-size: var(--t-sm);
  }
  .flags legend {
    padding: 0;
    margin-bottom: 6px;
    color: var(--muted);
  }
  .flags label {
    display: flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }
  input[type="checkbox"] {
    accent-color: var(--ink);
  }
  .catalog {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--t-sm);
    margin-bottom: var(--s-4);
  }
  .catalog th {
    text-align: left;
    font-weight: 500;
    font-size: var(--t-xs);
    color: var(--muted);
    padding: 0 var(--s-2) 6px;
  }
  .catalog td {
    padding: 8px var(--s-2);
    border-top: 1px solid var(--line);
    vertical-align: top;
  }
  .catalog tr:not(.included) td:not(:first-child) {
    opacity: 0.45;
  }
  .name {
    display: block;
    font-family: var(--mono);
    font-size: var(--t-xs);
    font-weight: 600;
  }
  .about {
    color: var(--muted);
  }
  .needs {
    white-space: nowrap;
  }
  .needs code {
    margin-right: 6px;
    font-size: var(--t-xs);
  }
  .needs code.missing {
    color: var(--warn);
    font-weight: 600;
  }
  .warning {
    margin: 0 0 var(--s-3);
    padding: var(--s-2) var(--s-3);
    background: var(--warn-soft);
    color: var(--warn);
    border-radius: var(--radius);
    font-size: var(--t-sm);
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
  @media (max-width: 860px) {
    .controls {
      grid-template-columns: minmax(0, 1fr);
    }
    .catalog th:nth-child(3),
    .catalog td:nth-child(3) {
      display: none;
    }
  }
</style>
