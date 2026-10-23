<script lang="ts">
  // The bench's own state in one quiet line: is the hardware there, what
  // was last switched, which flash client and image are in play.
  import { LiveDot, shortHash, type Tone } from "@petlibre/ui";
  import { app } from "../lib/bench.svelte";

  interface Item {
    label: string;
    value: string;
    tone: Tone;
    title?: string;
    mono?: boolean;
  }

  const items = $derived.by((): Item[] => {
    const health = app.health;
    if (!health) return [];
    const out: Item[] = [];

    if ("error" in health.ports) {
      out.push({ label: "Config", value: "unreadable", tone: "fail", title: `${health.config}: ${health.ports.error}` });
    } else {
      const found = (yes: boolean) => (yes ? "found" : "missing");
      out.push({ label: "Relay board", value: found(health.ports.relay_port), tone: health.ports.relay_port ? "pass" : "fail" });
      out.push({ label: "UART", value: found(health.ports.uart_port), tone: health.ports.uart_port ? "pass" : "fail" });
    }

    // The live command's relay states are newer than the last recorded ones.
    const relay = (channel: string) => app.snapshot?.current?.relay[channel] ?? health.relays?.[channel]?.on;
    for (const [channel, label] of [
      ["1", "Power"],
      ["2", "Loader strap"],
    ] as const) {
      const on = relay(channel);
      out.push({ label, value: on === undefined ? "unknown" : on ? "on" : "off", tone: on ? "warn" : "muted" });
    }

    out.push({
      label: "petlibre-flash",
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
    if (health.busy) out.push({ label: "Running", value: health.busy, tone: "live" });
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
    gap: 6px var(--s-5);
    margin: 0;
    padding: var(--s-3) 0;
    list-style: none;
    border-top: 1px solid var(--line);
    border-bottom: 1px solid var(--line);
    font-size: var(--t-sm);
  }
  li {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    white-space: nowrap;
  }
  .label {
    color: var(--muted);
  }
  .mono {
    font-family: var(--mono);
    font-size: var(--t-xs);
  }
</style>
