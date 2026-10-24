<script lang="ts">
  // A modal dialog over bits-ui: focus trap, Escape, outside click, scroll
  // lock and the title as the accessible name. `side` docks it to the right
  // edge as a drawer, full width on phones.
  import { Dialog } from "bits-ui";
  import type { Snippet } from "svelte";

  let {
    open = $bindable(true),
    onclose,
    title,
    meta,
    children,
    side = false,
  }: {
    open?: boolean;
    /** Called once the user dismisses it: Escape, the close button or outside. */
    onclose?: () => void;
    /** The heading, which also names the dialog for assistive technology. */
    title: Snippet;
    meta?: Snippet;
    children: Snippet;
    side?: boolean;
  } = $props();

  function onOpenChange(next: boolean) {
    if (!next) onclose?.();
  }
</script>

<Dialog.Root bind:open {onOpenChange}>
  <Dialog.Portal>
    <Dialog.Overlay class="pl-dialog-overlay" />
    <Dialog.Content class={["pl-dialog", side && "pl-dialog-side"]}>
      <header class="pl-dialog-header">
        <div>
          <Dialog.Title level={2} class="pl-dialog-title">{@render title()}</Dialog.Title>
          {#if meta}<p class="pl-dialog-meta">{@render meta()}</p>{/if}
        </div>
        <Dialog.Close class="pl-dialog-close" aria-label="Close">
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" /></svg>
        </Dialog.Close>
      </header>
      {@render children()}
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>

<style>
  /* bits-ui portals the content to <body>, outside this component's scope. */
  :global(.pl-dialog-overlay) {
    position: fixed;
    inset: 0;
    z-index: 10;
    background: color-mix(in srgb, var(--color-scrim) 45%, transparent);
  }
  :global(.pl-dialog) {
    position: fixed;
    z-index: 11;
    top: 50%;
    left: 50%;
    width: min(560px, calc(100vw - 2 * var(--space-4)));
    max-height: calc(100dvh - 2 * var(--space-4));
    overflow-y: auto;
    transform: translate(-50%, -50%);
    background: var(--color-surface);
    color: var(--color-text);
    border: 1px solid var(--color-line);
    border-radius: var(--radius-card);
    box-shadow: var(--shadow-overlay);
  }
  :global(.pl-dialog.pl-dialog-side) {
    top: 0;
    right: 0;
    bottom: 0;
    left: auto;
    width: min(640px, 100vw);
    max-height: none;
    transform: none;
    border-width: 0 0 0 1px;
    border-radius: var(--radius-card) 0 0 var(--radius-card);
    animation: pl-dialog-slide var(--motion-base) ease-out;
  }
  @keyframes pl-dialog-slide {
    from {
      transform: translateX(24px);
      opacity: 0;
    }
  }
  .pl-dialog-header {
    position: sticky;
    top: 0;
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-3);
    padding: var(--space-5) var(--space-5) var(--space-3);
    background: var(--color-surface);
    border-bottom: 1px solid var(--color-line);
  }
  :global(.pl-dialog-title) {
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }
  .pl-dialog-meta {
    margin: var(--space-1) 0 0;
    color: var(--color-text-muted);
    font-size: var(--text-sm);
  }
  :global(.pl-dialog-close) {
    display: grid;
    flex: none;
    place-items: center;
    width: 32px;
    height: 32px;
    border: 1px solid var(--color-line);
    border-radius: var(--radius-pill);
    background: var(--color-surface);
    cursor: pointer;
  }
  @media (pointer: coarse) {
    :global(.pl-dialog-close) {
      width: 44px;
      height: 44px;
    }
  }
  :global(.pl-dialog-close svg) {
    width: 14px;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
  }
</style>
