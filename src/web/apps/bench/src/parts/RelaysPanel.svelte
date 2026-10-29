<script lang="ts">
  // The relay board as the server last read or set it, and who holds the
  // bench: the server holds the lease and the board for its whole life.
  import { Empty, LiveDot, Panel, ago, clockTime, fullTime } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";

  const ROLES = [
    ["1", "Feeder power"],
    ["2", "Loader strap"],
    ["3", "UART into the feeder"],
    ["4", "UART out of the feeder"],
  ] as const;

  const state = $derived(app.state);
  const read = (at: number) => {
    const since = ago(at, app.now);
    return since === "now" ? "read just now" : `read ${since} ago`;
  };
  const holder = $derived(
    state
      ? `Server pid ${String(state.server.pid)}, holding the bench since ${clockTime(state.server.started, false)}${
          state.server.phase === "stopping"
            ? "; stopping"
            : state.server.stopping
              ? `; stopping once ${state.server.stopping.job === null ? "the write" : `job ${String(state.server.stopping.job)}`} ends, taking no new jobs`
              : ""
        }`
      : "",
  );
</script>

<Panel title="Relays">
  {#snippet meta()}
    {#if state}lease held by <code>wellfed-bench serve</code>{/if}
  {/snippet}
  {#if !state}
    <Empty title="Waiting for the bench server" />
  {:else}
    <ul class="relays">
      {#each ROLES as [channel, role] (channel)}
        {@const relay = state.relays[channel]}
        <li>
          <span class="n">{channel}</span>
          <span class="role">{role}</span>
          <span class="value">
            <LiveDot tone={relay === undefined ? "muted" : relay.on ? "warn" : "muted"} />
            {relay === undefined ? "not read" : relay.on ? "on" : "off"}
          </span>
          <span class="at" title={relay ? `Read ${fullTime(relay.at)}` : undefined}>
            {relay ? read(relay.at) : ""}
          </span>
        </li>
      {/each}
    </ul>
    <p class="holder">{holder}</p>
  {/if}
</Panel>

<style>
  .relays {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto auto;
    gap: 6px var(--space-3);
    align-items: baseline;
    font-size: var(--text-sm);
  }
  li {
    display: contents;
  }
  .n {
    color: var(--color-text-muted);
    font-size: var(--text-xs);
  }
  .role {
    overflow-wrap: anywhere;
  }
  .value {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
  }
  .at {
    color: var(--color-text-muted);
    font-size: var(--text-xs);
    text-align: right;
    white-space: nowrap;
  }
  .holder {
    margin: var(--space-3) 0 0;
    color: var(--color-text-muted);
    font-size: var(--text-xs);
  }
</style>
