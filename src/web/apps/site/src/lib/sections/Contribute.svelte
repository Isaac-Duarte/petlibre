<script lang="ts">
  import { DOCS, REPO } from "#lib/links.ts";
</script>

<section id="contribute" class="section" aria-labelledby="contribute-title">
  <div class="wrap grid">
    <div>
      <h2 id="contribute-title">Help build it.</h2>
      <p class="lede">
        Everything builds and tests on a laptop, against a simulated feeder. No hardware needed to start.
      </p>
    </div>

    <div class="body">
      <pre><code
          >git clone {REPO}
cd petlibre
cargo build --locked
make check</code
        ></pre>
      <p>
        With a PLAF103 and a USB serial adapter you can go further: full flash backups, recovery to stock, and the
        diagnostic firmware all work on real hardware today.
      </p>
      <ul>
        <li><a class="quiet-link" href={DOCS.development} rel="external">Development guide</a></li>
        <li><a class="quiet-link" href={DOCS.uart} rel="external">UART wiring and recovery</a></li>
        <li><a class="quiet-link" href={DOCS.contributing} rel="external">Contributing</a></li>
      </ul>
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
  .body {
    display: grid;
    gap: var(--space-5);
    min-width: 0;
  }
  pre {
    margin: 0;
    padding: var(--space-5);
    overflow-x: auto;
    border-radius: var(--radius-control);
    background: var(--color-sunken);
    font-size: var(--text-base);
    line-height: 1.7;
  }
  p {
    margin: 0;
    color: var(--color-text-muted);
  }
  ul {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-3) var(--space-5);
    margin: 0;
    padding: 0;
    list-style: none;
  }
</style>
