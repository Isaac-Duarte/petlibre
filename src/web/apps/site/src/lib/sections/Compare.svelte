<script lang="ts">
  import Stage from "#lib/Stage.svelte";
  import { COMPARISON, UPDATED } from "#lib/status.ts";

  const updated = new Intl.DateTimeFormat("en", { dateStyle: "long", timeZone: "UTC" }).format(new Date(UPDATED));
</script>

<section id="compare" class="section" aria-labelledby="compare-title">
  <div class="wrap">
    <h2 id="compare-title">What changes when you switch.</h2>
    <p class="lede">
      We learned how stock works by studying firmware 1.1.15. The Wellfed column is where we're going, with how far
      along each part is as of <time datetime={UPDATED}>{updated}</time>.
    </p>

    <!-- A real table: on phones each row restacks, and the column names come back as labels. -->
    <table>
      <thead>
        <tr>
          <th scope="col"><span class="visually-hidden">Topic</span></th>
          <th scope="col">Stock 1.1.15</th>
          <th scope="col">Wellfed</th>
        </tr>
      </thead>
      <tbody>
        {#each COMPARISON as row (row.topic)}
          <tr>
            <th scope="row">{row.topic}</th>
            <td data-label="Stock 1.1.15">{row.stock}</td>
            <td data-label="Wellfed" class="ours">
              <span>{row.wellfed}</span>
              <Stage stage={row.stage} />
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>

<style>
  table {
    width: 100%;
    margin-top: 64px;
    border-collapse: collapse;
    table-layout: fixed;
  }
  thead th {
    padding: 0 var(--space-5) var(--space-3) 0;
    border-bottom: 2px solid var(--color-text);
    font-size: var(--text-base);
    font-weight: var(--weight-semibold);
    text-align: left;
  }
  thead th:first-child {
    width: 24%;
  }
  tbody th,
  td {
    padding: var(--space-5) var(--space-5) var(--space-5) 0;
    border-bottom: 1px solid var(--color-line);
    text-align: left;
    vertical-align: top;
  }
  tbody th {
    font-weight: var(--weight-semibold);
  }
  td {
    color: var(--color-text-muted);
  }
  .ours {
    color: var(--color-text);
  }
  .ours span {
    display: block;
    margin-bottom: var(--space-2);
  }

  @media (max-width: 760px) {
    table,
    tbody,
    tr,
    th,
    td {
      display: block;
    }
    thead {
      display: none;
    }
    tr {
      padding-block: var(--space-5);
      border-bottom: 1px solid var(--color-line);
    }
    tr:first-child {
      border-top: 2px solid var(--color-text);
    }
    tbody th,
    td {
      padding: 0;
      border: 0;
    }
    tbody th {
      margin-bottom: var(--space-3);
      font-size: var(--text-lg);
    }
    td + td {
      margin-top: var(--space-3);
    }
    td::before {
      content: attr(data-label);
      display: block;
      color: var(--color-text-muted);
      font-size: var(--text-sm);
      font-weight: var(--weight-semibold);
    }
  }
</style>
