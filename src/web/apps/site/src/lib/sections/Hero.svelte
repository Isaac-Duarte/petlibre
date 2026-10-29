<script lang="ts">
  import { GitHubMark as GitHub } from "@wellfed/ui";
  import { drawing } from "#lib/drawing.ts";
  import { DOCS, REPO } from "#lib/links.ts";

  // The whole page: a headline over the feeder, traced from its 3D model,
  // with what Wellfed is pinned to either side of it. Each claim is one the
  // README and the MIT license already make; each anchor is a hotspot the
  // tracer projected, so a leader starts on a real part of the feeder.
  const claims = [
    { anchor: "window", label: "Open source", side: "right" },
    { anchor: "button_lock", label: "No account", side: "left" },
    { anchor: "outlet", label: "No vendor cloud", side: "left" },
    { anchor: "bowl", label: "Free (MIT)", side: "right" },
  ] as const;

  // Room on each side of the drawing for the claims, in the drawing's units.
  const MARGIN = 280;
  const [x0 = 0, y0 = 0, width = 0, height = 0] = drawing.viewBox;
  const left = x0 - MARGIN;
  const right = x0 + width + MARGIN;
  const callouts = claims.flatMap(({ anchor, label, side }) => {
    const [x, y] = drawing.anchors[anchor] ?? [];
    return x === undefined || y === undefined ? [] : [{ label, side, x, y, end: side === "left" ? left : right }];
  });
</script>

<section class="hero" aria-labelledby="hero-title">
  <div class="wrap">
    <p class="eyebrow">Open firmware for the PETLIBRO Granary PLAF103</p>
    <h1 id="hero-title">Your pet's dinner shouldn't need the internet.</h1>
    <p class="lede">Wellfed runs your feeder on your own Wi-Fi, with no cloud and no account.</p>
    <p class="status">
      <span><strong>In heavy development.</strong></span>
    </p>

    <figure class="drawing">
      <svg viewBox="{left} {y0} {width + 2 * MARGIN} {height}" role="img" aria-labelledby="drawing-title">
        <title id="drawing-title">
          Line drawing of the PLAF103 feeder: open source, no account, no vendor cloud, and free under the MIT license
        </title>
        <defs>
          <!-- A drafting table's dot grid, fading out from behind the feeder. -->
          <pattern id="drawing-dots" width="24" height="24" patternUnits="userSpaceOnUse">
            <circle cx="12" cy="12" r="1.4" />
          </pattern>
          <radialGradient id="drawing-fade">
            <stop offset="0.35" stop-color="white" />
            <stop offset="1" stop-color="black" />
          </radialGradient>
          <mask id="drawing-mask">
            <rect x={left} y={y0} width={width + 2 * MARGIN} {height} fill="url(#drawing-fade)" />
          </mask>
        </defs>
        <rect class="dots" x={left} y={y0} width={width + 2 * MARGIN} {height} mask="url(#drawing-mask)" />

        {#each drawing.groups as group (`${group.part}-${group.line}`)}
          <g class="lines {group.part} {group.line}">
            {#each group.paths as d, i (i)}
              <path pathLength="1" {d} />
            {/each}
          </g>
        {/each}

        {#each callouts as callout, n (callout.label)}
          <g class="callout {callout.side} at-{n}">
            <path class="leader" pathLength="1" d="M{callout.x} {callout.y} H{callout.end}" />
            <circle cx={callout.x} cy={callout.y} r="4.5" />
            <text x={callout.end} y={callout.y - 10}>{callout.label}</text>
          </g>
        {/each}
      </svg>
      <!-- On a phone the drawing is too small for its labels, so they move here. -->
      <ul class="claims">
        {#each callouts as callout (callout.label)}
          <li>{callout.label}</li>
        {/each}
      </ul>
    </figure>

    <div class="actions">
      <a class="button" href={REPO} rel="external"><GitHub /> View on GitHub</a>
      <a class="quiet-link" href={DOCS.roadmap} rel="external">See what works today</a>
    </div>
  </div>
</section>

<style>
  .hero {
    padding-block: 40px 72px;
    text-align: center;
  }
  @media (max-width: 760px) {
    .hero {
      padding-block: 40px 64px;
    }
  }
  .eyebrow {
    margin: 0 0 var(--space-4);
    color: var(--color-text-muted);
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }
  h1 {
    max-width: 18ch;
    margin-inline: auto;
    font-size: clamp(38px, 5.4vw, 64px);
    font-weight: var(--weight-display);
    letter-spacing: -0.045em;
    line-height: 0.98;
    text-wrap: balance;
  }
  .lede {
    margin-inline: auto;
    margin-top: var(--space-5);
    max-width: 52ch;
    text-wrap: balance;
  }
  /* Where Wellfed stands, led by a food-colored dot that breathes like a
     device that's on but busy. The words carry the meaning. */
  .status {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    margin: var(--space-4) 0 0;
    color: var(--color-text-muted);
    font-size: var(--text-base);
  }
  .status::before {
    content: "";
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--color-food);
  }
  .status strong {
    color: var(--color-text);
    font-weight: var(--weight-semibold);
  }
  @media (max-width: 640px) {
    .status {
      display: block;
      max-width: 32ch;
      margin-inline: auto;
    }
    .status::before {
      display: inline-block;
      margin-right: var(--space-2);
      vertical-align: 1px;
    }
  }

  /* A technical line drawing: lines carry the hierarchy, in the theme's own
     roles. Silhouettes in the text color, creases and the window in the strong
     line, the ring's print muted, and the food in its color. No fills. */
  .drawing {
    margin: var(--space-5) 0 0;
  }
  /* Sized by height, so the headline, the feeder and the buttons fit one screen. */
  .drawing svg {
    display: block;
    width: 100%;
    height: clamp(300px, 46svh, 520px);
    --stroke: 1.3;
  }
  pattern circle {
    fill: var(--color-line-strong);
  }
  .dots {
    fill: url(#drawing-dots);
    opacity: 0.45;
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
    font-size: 25px;
    font-weight: var(--weight-medium);
  }
  .left text {
    text-anchor: start;
  }
  .right text {
    text-anchor: end;
  }
  .claims {
    display: none;
  }
  /* Narrow screens: the labels would shrink past reading, so the drawing
     keeps only its lines and the claims sit under it as one line of text. */
  @media (max-width: 640px) {
    .drawing svg {
      height: auto;
    }
    .callout {
      display: none;
    }
    .claims {
      display: flex;
      flex-wrap: wrap;
      justify-content: center;
      gap: var(--space-2) var(--space-4);
      margin: var(--space-4) 0 0;
      padding: 0;
      list-style: none;
      color: var(--color-text-muted);
      font-size: var(--text-sm);
      font-weight: var(--weight-medium);
    }
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    align-items: center;
    gap: var(--space-4) var(--space-6);
    margin-top: var(--space-6);
  }

  /* The draw-in: the dot grid settles, the outline draws, then the bowl, the
     creases, the window and the ring's print; the food appears and the leaders
     run out to their claims. Only CSS: the page ships no script. */
  @media (prefers-reduced-motion: no-preference) {
    .status::before {
      animation: breathe 2.4s ease-in-out infinite;
    }
    .dots {
      opacity: 0;
      animation: settle 1.2s ease forwards;
    }
    .lines path,
    .leader {
      stroke-dashoffset: 1;
      animation: draw var(--duration, 1.6s) cubic-bezier(0.55, 0.1, 0.25, 1) var(--delay, 0s) forwards;
    }
    .body {
      --delay: 0.2s;
      --duration: 1.7s;
    }
    .bowl {
      --delay: 0.6s;
      --duration: 1.4s;
    }
    .crease {
      --delay: 1s;
      --duration: 1.3s;
    }
    .window {
      --delay: 1.3s;
      --duration: 0.9s;
    }
    .icon {
      --delay: 1.6s;
      --duration: 0.8s;
    }
    .food path {
      stroke-dashoffset: 0;
      opacity: 0;
      animation: appear 0.6s ease 2s forwards;
    }
    .callout {
      --duration: 0.6s;
    }
    .callout circle,
    .callout text,
    .claims li {
      opacity: 0;
      animation: appear 0.4s ease calc(var(--delay, 2.3s) + 0.25s) forwards;
    }
    .at-0 {
      --delay: 2.3s;
    }
    .at-1 {
      --delay: 2.42s;
    }
    .at-2 {
      --delay: 2.54s;
    }
    .at-3 {
      --delay: 2.66s;
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
  @keyframes breathe {
    50% {
      opacity: 0.35;
    }
  }
  @keyframes settle {
    to {
      opacity: 0.45;
    }
  }
</style>
