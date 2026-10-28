<script lang="ts">
  import { GitHubMark as GitHub } from "@wellfed/ui";
  import { REPO } from "#lib/links.ts";

  // The page's one picture: where a single feed command travels. Stock sends
  // it through the vendor's cloud and back; Wellfed keeps it on your Wi-Fi.
  // Drawn in HTML rather than SVG so it reflows to a phone without its labels
  // shrinking. Order matters: nodes and hops alternate.
  type Hop = { kind: "node"; label: string; away?: boolean } | { kind: "hop"; label: string };

  const routes: { name: string; mine: boolean; hops: Hop[] }[] = [
    {
      name: "Stock firmware",
      mine: false,
      hops: [
        { kind: "node", label: "Phone" },
        { kind: "hop", label: "internet" },
        { kind: "node", label: "Vendor cloud", away: true },
        { kind: "hop", label: "internet" },
        { kind: "node", label: "Feeder" },
      ],
    },
    {
      name: "Wellfed",
      mine: true,
      hops: [
        { kind: "node", label: "Phone" },
        { kind: "hop", label: "your Wi-Fi" },
        { kind: "node", label: "Feeder" },
      ],
    },
  ];
</script>

<section class="hero" aria-labelledby="hero-title">
  <div class="wrap">
    <h1 id="hero-title">Your pet's dinner shouldn't need the internet.</h1>
    <p class="lede">
      Wellfed is open firmware for the PETLIBRO Granary PLAF103. Your feeder runs on your own Wi-Fi, with no vendor
      cloud and no account. It's early days: Wellfed feeds on our test bench, but it isn't ready to look after a real
      pet yet.
    </p>
    <div class="actions">
      <a class="button" href={REPO} rel="external"><GitHub /> View on GitHub</a>
      <a class="quiet-link" href="#compare">Compare with stock</a>
    </div>

    <figure class="routes">
      <figcaption>Where a feed command goes when you tap Feed</figcaption>
      {#each routes as route (route.name)}
        <div class="route" class:mine={route.mine}>
          <span class="name">{route.name}</span>
          <ol class="path">
            {#each route.hops as hop, i (i)}
              {#if hop.kind === "node"}
                <li class="node" class:away={hop.away}>{hop.label}</li>
              {:else}
                <li class="hop"><span>{hop.label}</span></li>
              {/if}
            {/each}
          </ol>
        </div>
      {/each}
    </figure>
  </div>
</section>

<style>
  .hero {
    padding-block: 96px 112px;
  }
  @media (max-width: 760px) {
    .hero {
      padding-block: 56px 72px;
    }
  }
  h1 {
    max-width: 16ch;
    font-size: clamp(44px, 7.6vw, 88px);
    font-weight: var(--weight-display);
    letter-spacing: -0.045em;
    line-height: 0.98;
    text-wrap: balance;
  }
  .lede {
    margin-top: var(--space-5);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-4) var(--space-6);
    margin-top: var(--space-6);
  }

  .routes {
    display: grid;
    gap: var(--space-5);
    margin: 88px 0 0;
  }
  figcaption {
    color: var(--color-text-muted);
    font-size: var(--text-base);
  }
  .route {
    display: grid;
    grid-template-columns: 9rem minmax(0, 1fr);
    align-items: center;
    gap: var(--space-4);
  }
  @media (max-width: 640px) {
    .route {
      grid-template-columns: minmax(0, 1fr);
      gap: var(--space-2);
    }
  }
  .name {
    color: var(--color-text-muted);
    font-size: var(--text-base);
    font-weight: var(--weight-semibold);
  }
  .mine .name {
    color: var(--color-text);
  }
  .path {
    display: flex;
    align-items: center;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .node {
    flex: none;
    padding: 10px 16px;
    border: 1px solid var(--color-line-strong);
    border-radius: var(--radius-control);
    background: var(--color-surface);
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    white-space: nowrap;
  }
  .node.away {
    border-style: dashed;
    color: var(--color-text-muted);
  }
  .mine .node {
    border-color: var(--color-accent);
  }
  /* A hop is the line between two nodes, labelled with what it crosses. */
  .hop {
    position: relative;
    flex: 1;
    min-width: 36px;
    height: 0;
    border-top: 2px dashed var(--color-line-strong);
  }
  .mine .hop {
    border-top: 2px solid var(--color-accent);
  }
  .hop span {
    position: absolute;
    bottom: 6px;
    left: 50%;
    translate: -50% 0;
    color: var(--color-text-muted);
    font-size: var(--text-sm);
    white-space: nowrap;
  }
  .mine .hop span {
    color: var(--color-accent);
    font-weight: var(--weight-semibold);
  }
  @media (max-width: 420px) {
    .node {
      padding: 8px 10px;
      font-size: var(--text-sm);
    }
    .hop span {
      font-size: var(--text-xs);
    }
  }
</style>
