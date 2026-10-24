<script lang="ts">
  import Stage from "#lib/Stage.svelte";
  import { STEPS } from "#lib/status.ts";
</script>

<section id="how" class="section" aria-labelledby="how-title">
  <div class="wrap">
    <h2 id="how-title">From stock to yours in three steps.</h2>
    <p class="lede">This is where the project is heading. Each step says how far along it is today.</p>

    <!-- Numbered because the steps happen in this order. -->
    <ol class="steps">
      {#each STEPS as step, i (step.title)}
        <li>
          <span class="number" aria-hidden="true">{i + 1}</span>
          <h3>{step.title}</h3>
          <p>{step.body}</p>
          <Stage stage={step.stage} />
        </li>
      {/each}
    </ol>
  </div>
</section>

<style>
  .steps {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: var(--space-6);
    margin: 64px 0 0;
    padding: 0;
    list-style: none;
  }
  @media (max-width: 860px) {
    .steps {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  li {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-3);
    padding-top: var(--space-5);
    border-top: 2px solid var(--color-text);
  }
  .number {
    color: var(--color-text-muted);
    font-size: var(--text-base);
    font-weight: var(--weight-semibold);
    font-variant-numeric: tabular-nums;
  }
  h3 {
    font-size: var(--text-lg);
    font-weight: var(--weight-semibold);
    letter-spacing: -0.01em;
  }
  p {
    flex: 1;
    margin: 0;
    max-width: 34ch;
    color: var(--color-text-muted);
  }
</style>
