<script lang="ts">
  // The bench's own state in one quiet line: is the hardware there, what
  // was last switched, which flash client and image are in play.
  import { LiveDot, shortHash, type Tone } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";

  interface Item {
    label: string;
    value: string;
    tone: Tone;
    title?: string;
    mono?: boolean;
  }

  const items = $derived.by((): Item[] => {
    const state = app.state;
    const health = app.health;
    if (!state) return [];
    const out: Item[] = [];

    // The server holds the relay board for its life and looks for the UART
    // adapter itself, so both come from its state, not from the page
    // looking at paths.
    const read = Object.keys(state.relays).length;
    out.push({
      label: "Relay board",
      value: read ? "held by the server" : "not read",
      tone: read ? "pass" : "fail",
    });
    out.push({
      label: "UART",
      value: state.server.uart_port ? "found" : "missing",
      tone: state.server.uart_port ? "pass" : "fail",
    });

    // The server holds the board, so its last answers are the relays.
    for (const [channel, label] of [
      ["1", "Power"],
      ["2", "Loader strap"],
    ] as const) {
      const on = state.relays[channel]?.on;
      out.push({ label, value: on === undefined ? "unknown" : on ? "on" : "off", tone: on ? "warn" : "muted" });
    }

    // Read from files, so absent while the flash client holds the UART on
    // a page opened then.
    if (health) {
      out.push({
        label: "wellfed-flash",
        value: health.flash_tool.present ? shortHash(health.flash_tool.sha256) : "not built",
        tone: health.flash_tool.present ? "muted" : "fail",
        title: health.flash_tool.path,
        mono: health.flash_tool.present,
      });
      if (health.installed) {
        const role = health.installed.role === "recovery" ? ", recovery role" : "";
        out.push({
          label: `Last boot, bank ${health.installed.bank ?? "?"}${role}`,
          value: shortHash(health.installed.build),
          tone: "muted",
          title: `From ${health.installed.capture}`,
          mono: true,
        });
      }
    }

    const job = state.job;
    if (job && !job.end) out.push({ label: `Job ${String(job.id)}`, value: job.label, tone: "live" });
    return out;
  });
</script>

{#if items.length}
  <ul class="strip" aria-label="Bench hardware and tools">
    {#each items as item (item.label)}
      <li title={item.title}>
        <LiveDot tone={item.tone} />
        <span class="label">{item.label}</span>
        <span class="value" class:mono={item.mono}>{item.value}</span>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .strip {
    display: flex;
    flex-wrap: wrap;
    gap: 6px var(--space-5);
    margin: 0;
    padding: var(--space-3) 0;
    list-style: none;
    border-top: 1px solid var(--color-line);
    border-bottom: 1px solid var(--color-line);
    font-size: var(--text-sm);
  }
  li {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    white-space: nowrap;
  }
  .label {
    color: var(--color-text-muted);
  }
  .mono {
    font-family: var(--mono);
    font-size: var(--text-xs);
  }
</style>
