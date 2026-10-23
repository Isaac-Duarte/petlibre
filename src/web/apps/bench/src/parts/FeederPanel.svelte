<script lang="ts">
  // The feeder as the local server last heard it. The page never asks the
  // feeder itself; this is the server's health record and recent replies.
  import { Badge, Command, Empty, Kv, LiveDot, Panel, clockTime, shortHash, CopyButton } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";

  const feeder = $derived(app.feeder);

  function ageText(ms: number | null): string {
    if (ms === null) return "no health yet";
    const s = Math.round(ms / 1000);
    return s < 90 ? `${s} s ago` : `${Math.round(s / 60)} min ago`;
  }
</script>

<Panel title="Feeder">
  {#snippet meta()}
    {#if feeder?.server === "up"}from wellfed-local on <code>{feeder.address}</code>{/if}
  {/snippet}

  {#if !feeder}
    <Empty title="Asking the local server" />
  {:else if feeder.server === "unconfigured"}
    <Empty title="No local server configured">
      The page reads the feeder through <code>wellfed-local serve</code>, configured in
      <code>{feeder.config}</code>.
    </Empty>
  {:else if feeder.server === "down"}
    <div class="state">
      <LiveDot tone="muted" />
      <span class="headline">Server not running</span>
    </div>
    <p class="hint">Nothing answers on <code>{feeder.address}</code>. An integration run starts its own server; to watch the feeder outside one:</p>
    <Command command="wellfed-local serve --config private/server.json" />
  {:else if !feeder.replacement}
    <Empty title="Server up, no replacement-firmware role">This server does not track the replacement firmware's health.</Empty>
  {:else}
    {@const r = feeder.replacement}
    {@const h = r.health}
    {@const tone = r.online && !r.stale ? "pass" : r.online ? "warn" : "fail"}
    <div class="state">
      <LiveDot {tone} pulse={tone === "pass"} />
      <span class="headline">{r.online ? (r.stale ? "Online, health stale" : "Online") : "Offline"}</span>
      <span class="age">{ageText(r.age_ms)}</span>
    </div>

    {#if h}
      <div class="build">
        <span class="label">Build</span>
        <code class="hash" title={h.build}>{shortHash(h.build, 16)}</code>
        <CopyButton text={h.build} label="Copy ID" />
      </div>

      {#if h.faults.length}
        <ul class="faults" aria-label="Faults">
          {#each h.faults as fault (fault)}<li><Badge tone="fail" label={fault} /></li>{/each}
        </ul>
      {/if}

      <Kv
        entries={[
          ["Bank", h.bank],
          ["Health sequence", h.seq],
          ["Faults", h.faults.length ? h.faults.length : "none"],
          ["Wi-Fi credentials", h.wifi.source],
          ["Transport connects", h.transport.connects],
          ["Clock", h.clock ? (h.clock.unix_ms ? `${clockTime(h.clock.unix_ms / 1000)}, ${h.clock.syncs} syncs` : "not set") : "not reported"],
          ["Broker", feeder.broker_connected ? "connected" : "not connected"],
        ]}
      />
    {/if}

    {#if r.replies.length}
      <h3 class="replies-title">Recent replies</h3>
      <ul class="replies">
        {#each r.replies as reply, i (reply.id ?? i)}
          <li>
            <LiveDot tone={reply.ok ? "pass" : "fail"} />
            <span class="op">{reply.op ?? "reply"}</span>
            {#if reply.error}<span class="err">{reply.error}</span>{/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</Panel>

<style>
  .state {
    display: flex;
    align-items: center;
    gap: var(--s-2);
  }
  .headline {
    font-size: var(--t-md);
    font-weight: 600;
  }
  .age {
    margin-left: auto;
    color: var(--muted);
    font-size: var(--t-sm);
  }
  .hint {
    color: var(--muted);
    font-size: var(--t-sm);
    margin: var(--s-2) 0 var(--s-3);
  }
  .build {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    margin: var(--s-4) 0;
    padding: var(--s-3) var(--s-3) var(--s-3) var(--s-4);
    background: var(--sunken);
    border-radius: var(--radius);
  }
  .build .label {
    color: var(--muted);
    font-size: var(--t-sm);
  }
  .hash {
    font-size: var(--t-md);
    letter-spacing: 0.02em;
    margin-right: auto;
  }
  .faults {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0 0 var(--s-3);
    padding: 0;
    list-style: none;
  }
  .replies-title {
    margin-top: var(--s-4);
    font-size: var(--t-sm);
    color: var(--muted);
    font-weight: 500;
  }
  .replies {
    margin: var(--s-2) 0 0;
    padding: 0;
    list-style: none;
    font-size: var(--t-sm);
  }
  .replies li {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    padding: 3px 0;
  }
  .op {
    font-family: var(--mono);
    font-size: var(--t-xs);
  }
  .err {
    color: var(--fail);
  }
</style>
