<script lang="ts">
  import { GitHubMark as GitHub } from "@wellfed/ui";
  import { drawing } from "#lib/drawing.ts";
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

  // The feeder traced from its 3D model, with what Wellfed is pinned beside
  // it. Each claim is one the lede and the MIT license already make; each
  // anchor is a hotspot the tracer projected, so a leader starts on its part.
  const claims = [
    { anchor: "window", label: "Open source" },
    { anchor: "led_fault", label: "No account" },
    { anchor: "outlet", label: "No vendor cloud" },
    { anchor: "bowl", label: "Free (MIT)" },
  ];
  // Room right of the drawing for the labels, in the drawing's units.
  const LABELS = 250;
  const [x0 = 0, y0 = 0, width = 0, height = 0] = drawing.viewBox;
  const right = x0 + width + LABELS;
  const callouts = claims.flatMap(({ anchor, label }) => {
    const [x, y] = drawing.anchors[anchor] ?? [];
    return x === undefined || y === undefined ? [] : [{ label, x, y }];
  });
</script>

<section class="hero" aria-labelledby="hero-title">
  <div class="wrap">
    <div class="top">
      <div>
        <h1 id="hero-title">Your pet's dinner shouldn't need the internet.</h1>
        <p class="lede">
          Wellfed is open firmware for the PETLIBRO Granary PLAF103. Your feeder runs on your own Wi-Fi, with no vendor
          cloud and no account. It's early days: Wellfed feeds on our test bench, but it isn't ready to look after a
          real pet yet.
        </p>
        <div class="actions">
          <a class="button" href={REPO} rel="external"><GitHub /> View on GitHub</a>
          <a class="quiet-link" href="#compare">Compare with stock</a>
        </div>
      </div>

      <figure class="drawing">
        <svg viewBox="{x0} {y0} {width + LABELS} {height}" role="img" aria-labelledby="drawing-title">
          <title id="drawing-title">
            Line drawing of the PLAF103 feeder: open source, free under the MIT license, no account and no vendor cloud
          </title>
          {#each drawing.groups as group (`${group.part}-${group.line}`)}
            <g class="lines {group.part} {group.line}">
              {#each group.paths as d, i (i)}
                <path pathLength="1" {d} />
              {/each}
            </g>
          {/each}
          {#each callouts as callout, n (callout.label)}
            <g class="callout at-{n}">
              <path class="leader" pathLength="1" d="M{callout.x} {callout.y} H{right}" />
              <circle cx={callout.x} cy={callout.y} r="4" />
              <text x={right} y={callout.y - 9}>{callout.label}</text>
            </g>
          {/each}
        </svg>
        <figcaption>Traced from the PLAF103's 3D model.</figcaption>
      </figure>
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
  .top {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 460px);
    gap: var(--space-6);
    align-items: center;
  }
  @media (max-width: 860px) {
    .top {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  h1 {
    max-width: 16ch;
    font-size: clamp(42px, 6vw, 76px);
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

  /* A technical line drawing: lines carry the hierarchy, in the theme's own
     roles. Silhouettes in the text color, creases and the window in the strong
     line, the ring's print muted, and the food in its color. No fills. */
  .drawing {
    margin: 0;
  }
  @media (max-width: 860px) {
    .drawing {
      justify-self: center;
      width: 100%;
      max-width: 440px;
    }
  }
  .drawing svg {
    display: block;
    width: 100%;
    height: auto;
    --stroke: 1.3;
  }
  .drawing figcaption {
    margin-top: var(--space-3);
    font-size: var(--text-sm);
  }
  .lines path,
  .leader {
    fill: none;
    stroke-linecap: round;
    stroke-linejoin: round;
    stroke-dasharray: 1;
    stroke-dashoffset: 0;
  }
  .outline path,
  .edge path {
    stroke: var(--color-text);
    stroke-width: calc(var(--stroke) * 1.15);
  }
  .crease path,
  .window path {
    stroke: var(--color-line-strong);
    stroke-width: var(--stroke);
  }
  .icon path {
    stroke: var(--color-text-muted);
    stroke-width: calc(var(--stroke) * 0.75);
  }
  .food path {
    stroke: var(--color-food);
    stroke-width: calc(var(--stroke) * 0.8);
  }
  .leader {
    stroke: var(--color-line-strong);
    stroke-width: var(--stroke);
  }
  .callout circle {
    fill: var(--color-background);
    stroke: var(--color-text);
    stroke-width: var(--stroke);
  }
  .callout text {
    fill: var(--color-text-muted);
    font-size: 24px;
    font-weight: var(--weight-medium);
    text-anchor: end;
  }
  /* The drawing is scaled down on a phone, so its labels grow to stay legible. */
  @media (max-width: 480px) {
    .callout text {
      font-size: 30px;
    }
  }

  /* The draw-in: the outline, the bowl, the creases, the window and the
     ring's print, then the food appears and the leaders run out to their
     claims. Only CSS: the page ships no script. */
  @media (prefers-reduced-motion: no-preference) {
    .lines path,
    .leader {
      stroke-dashoffset: 1;
      animation: draw var(--duration, 1.6s) cubic-bezier(0.55, 0.1, 0.25, 1) var(--delay, 0s) forwards;
    }
    .body {
      --delay: 0.1s;
      --duration: 1.7s;
    }
    .bowl {
      --delay: 0.5s;
      --duration: 1.4s;
    }
    .crease {
      --delay: 0.9s;
      --duration: 1.3s;
    }
    .window {
      --delay: 1.2s;
      --duration: 0.9s;
    }
    .icon {
      --delay: 1.5s;
      --duration: 0.8s;
    }
    .food path {
      stroke-dashoffset: 0;
      opacity: 0;
      animation: appear 0.6s ease 1.9s forwards;
    }
    .callout {
      --duration: 0.5s;
    }
    .callout circle,
    .callout text {
      opacity: 0;
      animation: appear 0.4s ease calc(var(--delay) + 0.2s) forwards;
    }
    .at-0 {
      --delay: 2.2s;
    }
    .at-1 {
      --delay: 2.32s;
    }
    .at-2 {
      --delay: 2.44s;
    }
    .at-3 {
      --delay: 2.56s;
    }
  }
  @keyframes draw {
    to {
      stroke-dashoffset: 0;
    }
  }
  @keyframes appear {
    to {
      opacity: 1;
    }
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
