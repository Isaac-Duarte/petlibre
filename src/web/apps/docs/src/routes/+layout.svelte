<script lang="ts">
  import "@wellfed/ui/base.css";
  import "#lib/docs.css";
  import { afterNavigate } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { page } from "$app/state";
  import { GitHubMark } from "@wellfed/ui";
  import type { Snippet } from "svelte";
  import { favicon, lockup, lockupOnLight, themeColor } from "#lib/brand.ts";
  import { HOME, REPO } from "#lib/links.ts";
  import Search from "#lib/Search.svelte";
  import type { LayoutProps } from "./$types";

  let { data, children }: LayoutProps & { children: Snippet } = $props();

  // On phones the sidebar is a drawer under the header, closed after each move.
  let menuOpen = $state(false);
  afterNavigate(() => (menuOpen = false));
</script>

<svelte:head>
  <link rel="icon" href={favicon} type="image/svg+xml" />
  <meta name="theme-color" content={themeColor.light} media="(prefers-color-scheme: light)" />
  <meta name="theme-color" content={themeColor.dark} media="(prefers-color-scheme: dark)" />
</svelte:head>

<a class="skip" href="#main">Skip to content</a>

<header class="top">
  <button
    type="button"
    class="menu"
    aria-expanded={menuOpen}
    aria-controls="sidebar"
    aria-label="Menu"
    onclick={() => (menuOpen = !menuOpen)}
  >
    <svg viewBox="0 0 16 16" aria-hidden="true">
      {#if menuOpen}<path d="M4 4l8 8M12 4l-8 8" />{:else}<path d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11" />{/if}
    </svg>
  </button>
  <a href={resolve("/[...slug]", { slug: "" })} class="home">
    <picture>
      <source srcset={lockup} media="(prefers-color-scheme: dark)" />
      <img src={lockupOnLight} alt="Wellfed" width="139" height="32" />
    </picture>
    <span class="product">Docs</span>
  </a>
  <div class="actions">
    <Search />
    <a href={HOME} rel="external" class="ext">well-fed.dev</a>
    <a href={REPO} rel="external" class="github" aria-label="GitHub"><GitHubMark /></a>
  </div>
</header>

<div class="shell">
  <nav id="sidebar" class="sidebar" class:open={menuOpen} aria-label="Docs">
    {#each data.nav as section (section.label)}
      <div class="group">
        <p>{section.label}</p>
        <ul>
          {#each section.links as link (link.slug)}
            {@const href = resolve("/[...slug]", { slug: link.slug })}
            <li>
              <a {href} aria-current={page.url.pathname === href ? "page" : undefined}>{link.label}</a>
            </li>
          {/each}
        </ul>
      </div>
    {/each}
  </nav>

  {@render children()}
</div>
