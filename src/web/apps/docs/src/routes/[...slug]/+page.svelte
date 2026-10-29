<script lang="ts">
  import { resolve } from "$app/paths";
  import { CopyButton } from "@wellfed/ui";
  import { loadMarkdown } from "#lib/api/markdown.ts";
  import { OG_IMAGE, SITE } from "#lib/links.ts";
  import ModeSwitch from "#lib/ModeSwitch.svelte";
  import Toc from "#lib/Toc.svelte";
  import type { PageProps } from "./$types";

  let { data }: PageProps = $props();
  let doc = $derived(data.doc);
  let title = $derived(doc.slug === "" ? "Wellfed docs" : `${doc.title} · Wellfed docs`);
  let url = $derived(doc.slug === "" ? `${SITE}/` : `${SITE}/${doc.slug}`);

  // The page's Markdown, fetched once the page is up so the copy itself is
  // synchronous: a clipboard write after an await can lose the click's
  // permission in some browsers. Until it arrives there is no button.
  let markdown = $state<{ slug: string; text: string }>();
  $effect(() => {
    const slug = doc.slug;
    loadMarkdown(slug).then(
      (text) => (markdown = { slug, text }),
      () => (markdown = undefined),
    );
  });
</script>

<svelte:head>
  <title>{title}</title>
  <meta name="description" content={doc.description} />
  <link rel="canonical" href={url} />
  <meta property="og:type" content="article" />
  <meta property="og:site_name" content="Wellfed docs" />
  <meta property="og:title" content={doc.title} />
  <meta property="og:description" content={doc.description} />
  <meta property="og:url" content={url} />
  <meta property="og:image" content={OG_IMAGE} />
  <meta name="twitter:card" content="summary_large_image" />
</svelte:head>

<main id="main" class="content">
  <article data-pagefind-body>
    <div class="page-head">
      {#if data.section}<p class="eyebrow" data-pagefind-ignore>{data.section}</p>{/if}
      <div class="page-tools" data-pagefind-ignore>
        {#if markdown?.slug === doc.slug}<CopyButton text={markdown.text} label="Copy as Markdown" />{/if}
        {#if doc.hasDetails}<ModeSwitch />{/if}
      </div>
    </div>
    <div class="prose">
      <!-- eslint-disable-next-line svelte/no-at-html-tags -- rendered at build time from the repository's own docs/ -->
      {@html doc.html}
    </div>
  </article>

  <footer class="page-foot">
    <a href={doc.editUrl} rel="external" class="edit">Edit this page on GitHub</a>
    <nav aria-label="Previous and next">
      {#if data.previous}
        <a href={resolve("/[...slug]", { slug: data.previous.slug })} rel="prev" class="step">
          <span>Previous</span>{data.previous.label}
        </a>
      {/if}
      {#if data.next}
        <a href={resolve("/[...slug]", { slug: data.next.slug })} rel="next" class="step next">
          <span>Next</span>{data.next.label}
        </a>
      {/if}
    </nav>
  </footer>
</main>

<aside class="outline">
  {#key doc.slug}<Toc headings={doc.headings} />{/key}
</aside>
