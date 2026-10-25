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
    catalog
      .filter((entry) => includes(entry, full))
      .flatMap((entry) => {
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
    options.only = options.only.includes(scenario)
      ? options.only.filter((s) => s !== scenario)
      : [...options.only, scenario];
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
      <label
        ><input type="checkbox" bind:checked={options.build} /> <code>--build</code> variants from this checkout</label
      >
      <label><input type="checkbox" bind:checked={options.power} /> <code>--power</code> cold boots</label>
      <label
        ><input type="checkbox" bind:checked={options.destructive} /> <code>--destructive</code> power cuts mid-update</label
      >
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
          <td class="pick">
            <label class="hit">
              <input
                type="checkbox"
                checked={entry.baseline || options.only.includes(entry.name)}
                disabled={entry.baseline}
                onchange={() => {
                  toggle(entry.name);
                }}
                aria-label="Only {entry.name}"
                title={entry.baseline
                  ? "Installs the build under test; every run starts with it"
                  : `Run only the checked scenarios`}
              />
            </label>
          </td>
          <td class="scenario">
            <span class="name">{entry.name}</span>
            <span class="about">{entry.about}</span>
          </td>
          <td class="tier"><Badge tone="muted" label={entry.tier} /></td>
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

  <Command {command} />
</Panel>

<style>
  .controls {
    display: grid;
    grid-template-columns: minmax(180px, 260px) auto minmax(0, 1fr);
    gap: var(--space-4) var(--space-5);
    align-items: start;
    margin-bottom: var(--space-4);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: var(--text-sm);
    color: var(--color-text-muted);
  }
  input[type="text"] {
    height: 34px;
    padding: 0 var(--space-3);
    border: 1px solid var(--color-line-strong);
    border-radius: var(--radius-pill);
    background: var(--color-surface);
    color: var(--color-text);
    font: var(--text-sm) var(--mono);
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
    background: var(--color-sunken);
    border-radius: var(--radius-pill);
    width: max-content;
  }
  .segmented button {
    height: 28px;
    padding: 0 14px;
    border: 0;
    border-radius: var(--radius-pill);
    background: none;
    color: var(--color-text-muted);
    cursor: pointer;
    font-size: var(--text-sm);
    font-weight: 500;
  }
  .segmented button.on {
    background: var(--color-surface);
    color: var(--color-text);
    box-shadow: 0 0 0 1px var(--color-line);
  }
  .only {
    padding: 0 10px;
    color: var(--color-accent);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .flags {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(270px, 1fr));
    gap: 6px var(--space-4);
    margin: 0;
    padding: 0;
    border: 0;
    font-size: var(--text-sm);
  }
  .flags legend {
    padding: 0;
    margin-bottom: 6px;
    color: var(--color-text-muted);
  }
  .flags label {
    display: flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }
  input[type="checkbox"] {
    accent-color: var(--color-text);
  }
  .catalog {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-sm);
    margin-bottom: var(--space-4);
  }
  .catalog th {
    text-align: left;
    font-weight: 500;
    font-size: var(--text-xs);
    color: var(--color-text-muted);
    padding: 0 var(--space-2) 6px;
  }
  .catalog td {
    padding: 8px var(--space-2);
    border-top: 1px solid var(--color-line);
    vertical-align: top;
  }
  .catalog tr:not(.included) td:not(:first-child) {
    opacity: 0.45;
  }
  .name {
    display: block;
    font-family: var(--mono);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .about {
    color: var(--color-text-muted);
  }
  .needs {
    white-space: nowrap;
  }
  .needs code {
    margin-right: 6px;
    font-size: var(--text-xs);
  }
  .needs code.missing {
    color: var(--color-warning);
    font-weight: 600;
  }
  .warning {
    margin: 0 0 var(--space-3);
    padding: var(--space-2) var(--space-3);
    background: var(--color-warning-soft);
    color: var(--color-warning);
    border-radius: var(--radius-control);
    font-size: var(--text-sm);
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
  .hit {
    display: inline-flex;
  }
  @media (max-width: 860px) {
    .controls {
      grid-template-columns: minmax(0, 1fr);
    }
    .flags {
      grid-template-columns: minmax(0, 1fr);
    }
    .flags label {
      white-space: normal;
    }
  }
  /* Each scenario becomes a card: the checkbox beside its name and what it
     checks, then its tier and flags on one line below, so the description
     gets the width instead of being squeezed by the flags column. */
  @media (max-width: 600px) {
    .catalog thead {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
    }
    .catalog tr {
      display: grid;
      grid-template-columns: 44px auto minmax(0, 1fr);
      grid-template-areas: "pick scenario scenario" "pick tier needs";
      gap: var(--space-1) var(--space-2);
      padding: var(--space-2) 0;
      border-top: 1px solid var(--color-line);
    }
    .catalog td {
      padding: 0;
      border: 0;
    }
    .catalog .pick {
      grid-area: pick;
    }
    .catalog .scenario {
      grid-area: scenario;
    }
    .catalog .tier {
      grid-area: tier;
    }
    .catalog .needs {
      grid-area: needs;
      align-self: center;
      white-space: normal;
    }
    .catalog .needs:empty {
      display: none;
    }
  }
  @media (max-width: 760px), (pointer: coarse) {
    .hit {
      align-items: center;
      justify-content: center;
      width: 44px;
      height: 44px;
    }
    .flags label {
      min-height: 44px;
    }
    input[type="checkbox"] {
      width: 20px;
      height: 20px;
      flex: none;
    }
    input[type="text"] {
      height: 44px;
      font-size: var(--text-md);
    }
    .segmented button {
      height: 44px;
    }
  }
</style>
