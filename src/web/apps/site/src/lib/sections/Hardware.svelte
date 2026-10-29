<script lang="ts">
  import { DOCS } from "#lib/links.ts";

  const facts: [string, string][] = [
    ["Model", "PETLIBRO Granary PLAF103"],
    ["Capacity", "5 L"],
    ["FCC ID", "2A3DEPLAF103S"],
  ];
</script>

<section id="hardware" class="section" aria-labelledby="hardware-title">
  <div class="wrap grid">
    <div>
      <h2 id="hardware-title">Built for one feeder, and tested on it.</h2>
      <p class="lede">
        Wellfed is built and tested on the Granary PLAF103. Not sure which one you have? Check the FCC ID on its label.
      </p>
    </div>

    <div class="spec">
      <dl>
        {#each facts as [term, value] (term)}
          <div>
            <dt>{term}</dt>
            <dd>{value}</dd>
          </div>
        {/each}
      </dl>
      <p class="not">
        Other models aren't supported, even ones that look alike. They're built differently inside, and the wrong
        firmware could leave a feeder unable to start. That includes the PLAF108, PLAF203 and AF301.
      </p>
      <a class="quiet-link" href={DOCS.plaf103} rel="external">Read the PLAF103 notes</a>
    </div>
  </div>
</section>

<style>
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 64px;
  }
  @media (max-width: 860px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
      gap: var(--space-6);
    }
  }
  dl {
    margin: 0;
    border-top: 2px solid var(--color-text);
  }
  dl div {
    display: grid;
    grid-template-columns: 9rem minmax(0, 1fr);
    gap: var(--space-4);
    padding-block: var(--space-4);
    border-bottom: 1px solid var(--color-line);
  }
  dt {
    color: var(--color-text-muted);
  }
  dd {
    margin: 0;
    font-weight: var(--weight-medium);
  }
  .not {
    margin: var(--space-5) 0 var(--space-4);
    color: var(--color-text-muted);
  }
</style>
