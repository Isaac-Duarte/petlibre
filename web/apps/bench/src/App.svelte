<script lang="ts">
  import { LiveDot, clockTime } from "@wellfed/ui";
  import { onMount } from "svelte";
  import { app } from "./lib/bench.svelte";
  import { route, TABS, type Tab } from "./lib/route.svelte";
  import { theme } from "./lib/theme.svelte";
  import BenchStrip from "./parts/BenchStrip.svelte";
  import Overview from "./views/Overview.svelte";
  import Tests from "./views/Tests.svelte";
  import Sessions from "./views/Sessions.svelte";
  import Log from "./views/Log.svelte";

  const NAMES: Record<Tab, string> = { overview: "Overview", tests: "Tests", sessions: "Sessions", log: "Log" };
  const KEYS: Record<string, Tab> = { o: "overview", t: "tests", s: "sessions", l: "log" };

  onMount(() => app.start());

  const current = $derived(app.snapshot?.current ?? null);
  const live = $derived(current?.outcome === "running");

  $effect(() => {
    document.title = current ? `${current.outcome} ${current.command} – wellfed bench` : "wellfed bench";
  });

  // "g" then a tab's letter, as in most developer tools; "/" filters the log.
  let pending = false;
  function onkeydown(event: KeyboardEvent) {
    const target = event.target as HTMLElement | null;
    if (target?.closest("input, textarea, select, [contenteditable]") || event.metaKey || event.ctrlKey || event.altKey) {
      return;
    }
    if (pending) {
      pending = false;
      const tab = KEYS[event.key];
      if (tab) route.go(tab);
      return;
    }
    if (event.key === "g") pending = true;
    if (event.key === "/") {
      event.preventDefault();
      route.go("log");
      queueMicrotask(() => document.getElementById("log-filter")?.focus());
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="shell">
  <header class="top">
    <a class="mark" href={route.href("overview")}>
      <span class="word">wellfed</span>
      <span class="tool">bench</span>
    </a>

    <nav aria-label="Views">
      {#each TABS as tab (tab)}
        <a href={route.href(tab)} class:on={route.tab === tab} aria-current={route.tab === tab ? "page" : undefined}>
          {NAMES[tab]}
          {#if tab === "tests" && current?.command === "itest" && live}
            <LiveDot pulse label="Integration run in progress" />
          {/if}
        </a>
      {/each}
    </nav>

    <div class="status">
      <span class="link" title={app.connected ? "Receiving updates from the bench" : "Reconnecting to the bench"}>
        <LiveDot tone={app.connected ? (live ? "live" : "pass") : "fail"} pulse={app.connected && live} />
        {app.connected ? (live ? "Live" : "Connected") : "Reconnecting"}
      </span>
      <time class="clock" title="Bench host time">{clockTime(app.now)}</time>
      <button type="button" class="theme" onclick={() => theme.cycle()} title="Theme: {theme.value}" aria-label="Theme: {theme.value}">
        {#if theme.value === "dark"}
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M13 9.5A5.5 5.5 0 0 1 6.5 3a5.5 5.5 0 1 0 6.5 6.5Z" /></svg>
        {:else if theme.value === "light"}
          <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="3" /><path d="M8 1v2M8 13v2M1 8h2M13 8h2M3 3l1.4 1.4M11.6 11.6 13 13M3 13l1.4-1.4M11.6 4.4 13 3" /></svg>
        {:else}
          <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="5.5" /><path d="M8 2.5v11a5.5 5.5 0 0 0 0-11Z" class="fill" /></svg>
        {/if}
      </button>
    </div>
  </header>

  <BenchStrip />

  <main>
    {#if route.tab === "overview"}
      <Overview />
    {:else if route.tab === "tests"}
      <Tests />
    {:else if route.tab === "sessions"}
      <Sessions />
    {:else}
      <Log />
    {/if}
  </main>
</div>

<style>
  .shell {
    max-width: 1480px;
    margin: 0 auto;
    padding: 0 var(--s-5) var(--s-7);
  }
  .top {
    display: flex;
    align-items: center;
    gap: var(--s-5);
    min-height: 64px;
    flex-wrap: wrap;
  }
  .mark {
    display: flex;
    align-items: baseline;
    gap: 6px;
    color: var(--ink);
    text-decoration: none;
  }
  .word {
    font-size: var(--t-lg);
    font-weight: 700;
    letter-spacing: -0.03em;
  }
  .tool {
    font-size: var(--t-lg);
    font-weight: 300;
    color: var(--muted);
    letter-spacing: -0.02em;
  }
  nav {
    display: flex;
    gap: 2px;
    padding: 3px;
    background: var(--sunken);
    border-radius: var(--pill);
  }
  nav a {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 16px;
    border-radius: var(--pill);
    color: var(--muted);
    text-decoration: none;
    font-weight: 500;
  }
  nav a:hover {
    color: var(--ink);
  }
  nav a.on {
    background: var(--ink);
    color: var(--paper);
  }
  .status {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: var(--s-4);
    font-size: var(--t-sm);
    color: var(--muted);
  }
  .link {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .clock {
    color: var(--ink-2);
    font-variant-numeric: tabular-nums;
  }
  .theme {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    border: 1px solid var(--line);
    background: var(--surface);
    cursor: pointer;
  }
  .theme:hover {
    border-color: var(--ink);
  }
  .theme svg {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.4;
    stroke-linecap: round;
  }
  .theme .fill {
    fill: currentColor;
    stroke: none;
  }
  main {
    margin-top: var(--s-5);
  }
  @media (max-width: 760px) {
    .shell {
      padding: 0 var(--s-4) var(--s-6);
    }
    .top {
      gap: var(--s-3);
      padding-top: var(--s-3);
    }
    nav {
      order: 3;
      width: 100%;
      overflow-x: auto;
    }
    nav a {
      flex: 1;
      justify-content: center;
      padding: 0 10px;
    }
    .clock {
      display: none;
    }
  }
</style>
