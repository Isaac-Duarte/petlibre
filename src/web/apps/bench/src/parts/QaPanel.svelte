<script lang="ts">
  // The guided QA run takes the top of the page while it runs: what to do
  // now, large, then each check with the answers recorded so far. A tone
  // marks each new prompt, so the operator can watch the feeder instead.
  import type { QaView } from "@wellfed/api";
  import { Badge, toneOf } from "@wellfed/ui";
  import { ANSWER_SECONDS, CHECK_NAMES, guidance, subject } from "../lib/qa";

  let { qa, now }: { qa: QaView; now: number } = $props();

  let audio: AudioContext | null = $state(null);
  let lastPrompt: string | null = null;

  const says = $derived(guidance(qa));
  const waited = $derived(qa.prompt?.asked != null ? Math.max(0, now - qa.prompt.asked) : null);
  const promptKey = $derived(
    qa.prompt ? `${qa.capture}:${qa.prompt.ask}:${qa.prompt.target}:${qa.prompt.asked}` : null,
  );

  function tone(freq: number, length: number) {
    if (!audio) return;
    const oscillator = audio.createOscillator();
    const gain = audio.createGain();
    const t0 = audio.currentTime;
    oscillator.frequency.value = freq;
    oscillator.connect(gain);
    gain.connect(audio.destination);
    gain.gain.setValueAtTime(0.25, t0);
    gain.gain.exponentialRampToValueAtTime(0.001, t0 + length);
    oscillator.start(t0);
    oscillator.stop(t0 + length + 0.02);
  }

  // Cue each new prompt once; the feeder's own cue may be masked by the bench.
  $effect(() => {
    const key = promptKey;
    if (key && key !== lastPrompt && lastPrompt !== null) tone(880, 0.18);
    lastPrompt = key ?? "";
  });

  function enableSound() {
    audio ??= new AudioContext();
    void audio.resume();
    tone(880, 0.12);
  }
</script>

<section class="qa" aria-label="Guided QA" aria-live="polite">
  <div class="top">
    <h2>Guided QA</h2>
    <code>{qa.capture}</code>
  </div>

  <p class="big">{says.headline}</p>
  <p class="detail">{says.detail}</p>
  {#if qa.prompt?.ask === "press" && waited !== null}
    <p class="detail">{Math.max(0, Math.ceil(ANSWER_SECONDS - waited))} s left to press</p>
  {:else if waited !== null}
    <p class="detail">Asked {Math.floor(waited)} s ago</p>
  {/if}

  <ol class="checks">
    {#each qa.checks as check (check.check)}
      <li class:cur={check.state === "running"}>
        <span class="name">{CHECK_NAMES[check.check]}</span>
        <Badge tone={check.state === "waiting" ? "muted" : toneOf(check.state)} label={check.state} />
        {#each check.items as item, i (i)}
          <span class="item" class:bad={!item.passed} title={item.reason ?? "pass"}>
            {subject(item.ask, item.target)}{item.passed ? "" : `: ${item.reason ?? "fail"}`}
          </span>
        {/each}
      </li>
    {/each}
  </ol>

  {#if qa.unreadable > 0}
    <p class="detail">
      {qa.unreadable} unreadable QA record{qa.unreadable === 1 ? "" : "s"}: the audit will refuse this run.
    </p>
  {/if}

  <div class="controls">
    {#if audio}<span>Sound on</span>{:else}<button type="button" onclick={enableSound}>Enable sound</button>{/if}
  </div>
</section>

<style>
  .qa {
    background: var(--color-surface);
    color: var(--color-text);
    border: 1px solid var(--color-line);
    border-radius: var(--radius-control);
    padding: var(--space-5) var(--space-6);
    margin-bottom: var(--space-5);
  }
  .top {
    display: flex;
    justify-content: space-between;
    gap: var(--space-3);
    flex-wrap: wrap;
    color: var(--color-text-muted);
  }
  h2 {
    font-weight: 500;
    font-size: var(--text-base);
  }
  .top code {
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }
  .big {
    margin: var(--space-3) 0 var(--space-2);
    font-size: var(--text-xxl);
    font-weight: 600;
    letter-spacing: -0.03em;
    line-height: 1.1;
  }
  .detail {
    margin: 0 0 var(--space-1);
    color: var(--color-text-muted);
  }
  .checks {
    list-style: none;
    margin: var(--space-4) 0 0;
    padding: 0;
    font-size: var(--text-sm);
  }
  .checks li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
    padding: 4px 0;
    color: var(--color-text-muted);
  }
  .checks li.cur {
    color: var(--color-text);
    font-weight: 600;
  }
  .name {
    width: 6.5em;
  }
  .item {
    font-size: var(--text-xs);
    padding: 1px 8px;
    border-radius: var(--radius-pill);
    background: var(--color-sunken);
    color: var(--color-text);
  }
  .item.bad {
    background: var(--color-fault-soft);
    color: var(--color-fault);
    text-decoration: underline wavy;
  }
  .controls {
    display: flex;
    gap: var(--space-4);
    align-items: center;
    margin-top: var(--space-4);
    font-size: var(--text-sm);
  }
  .controls button {
    height: 30px;
    padding: 0 14px;
    border-radius: var(--radius-pill);
    border: 0;
    background: var(--color-accent);
    color: var(--color-accent-text);
    font-weight: 600;
    cursor: pointer;
  }
  @media (max-width: 760px), (pointer: coarse) {
    .controls button {
      height: 44px;
      padding: 0 var(--space-4);
    }
  }
  @media (max-width: 600px) {
    .qa {
      padding: var(--space-4);
    }
    .big {
      font-size: var(--text-xl);
    }
  }
</style>
