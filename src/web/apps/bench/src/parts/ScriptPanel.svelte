<script lang="ts">
  // An operator script takes the top of the page: one instruction, large,
  // with a countdown to the next, a tone at each change and optional speech.
  import type { ScriptView } from "@petlibre/api";
  import { countdown, scriptPosition } from "../lib/script";

  let { script, now }: { script: ScriptView; now: number } = $props();

  let audio: AudioContext | null = $state(null);
  let speak = $state(false);
  let lastKey: string | null = null;

  const position = $derived(script.started ? scriptPosition(script.steps, script.started, now) : null);

  function tone(freq: number, length: number, at: number) {
    if (!audio) return;
    const oscillator = audio.createOscillator();
    const gain = audio.createGain();
    const t0 = audio.currentTime + at;
    oscillator.frequency.value = freq;
    oscillator.connect(gain);
    gain.connect(audio.destination);
    gain.gain.setValueAtTime(0.25, t0);
    gain.gain.exponentialRampToValueAtTime(0.001, t0 + length);
    oscillator.start(t0);
    oscillator.stop(t0 + length + 0.02);
  }

  function cue(text: string) {
    if (/^done/i.test(text)) tone(660, 0.9, 0);
    else if (/move the probe/i.test(text)) {
      tone(520, 0.15, 0);
      tone(520, 0.15, 0.25);
    } else tone(880, 0.18, 0);
    if (speak && "speechSynthesis" in window) {
      speechSynthesis.cancel();
      speechSynthesis.speak(new SpeechSynthesisUtterance(text.replace(/[():]/g, " ")));
    }
  }

  // Cue each new step once; a step already well past on page load stays quiet.
  $effect(() => {
    const p = position;
    if (!p?.current || !script.started) return;
    const key = `${script.started}:${p.index}`;
    if (key === lastKey) return;
    if (lastKey !== null || p.clock - p.current.at < 2) cue(p.current.text);
    lastKey = key;
  });

  function enableSound() {
    audio ??= new AudioContext();
    void audio.resume();
    tone(880, 0.12, 0);
  }
</script>

<section class="script" aria-label="Operator script" aria-live="polite">
  <div class="top">
    <h2>What to do now{script.title ? `: ${script.title}` : ""}</h2>
    {#if position}<span class="clock">Test clock {countdown(position.clock)}</span>{/if}
  </div>

  {#if !position}
    <p class="big">Get ready</p>
    <p class="next">
      {script.capture_exists
        ? "Waiting for the feeder to start the test (LEDs blinking)"
        : "Waiting for the capture to start"}
    </p>
  {:else}
    <p class="big">{position.current?.text ?? "Get ready"}</p>
    {#if position.next}
      {@const from = position.current?.at ?? 0}
      <p class="next">Next in {countdown(position.next.at - position.clock)}: {position.next.text}</p>
      <div class="track">
        <div style:width="{Math.min(100, (100 * (position.clock - from)) / (position.next.at - from))}%"></div>
      </div>
    {/if}
    <ol class="steps">
      {#each script.steps as step, i (i)}
        <li class:cur={i === position.index} class:past={i < position.index}>
          <span class="at">{countdown(step.at)}</span>{step.text}
        </li>
      {/each}
    </ol>
  {/if}

  <div class="controls">
    {#if audio}<span>Sound on</span>{:else}<button type="button" onclick={enableSound}>Enable sound</button>{/if}
    <label><input type="checkbox" bind:checked={speak} /> Speak steps</label>
  </div>
</section>

<style>
  .script {
    background: var(--color-text);
    color: var(--color-background);
    border-radius: var(--radius-control);
    padding: var(--space-5) var(--space-6);
    margin-bottom: var(--space-5);
  }
  .top {
    display: flex;
    justify-content: space-between;
    gap: var(--space-3);
    flex-wrap: wrap;
  }
  h2 {
    font-weight: 500;
    font-size: var(--text-base);
    opacity: 0.75;
  }
  .clock {
    font-variant-numeric: tabular-nums;
    opacity: 0.75;
  }
  .big {
    margin: var(--space-3) 0 var(--space-2);
    font-size: var(--text-xxl);
    font-weight: 600;
    letter-spacing: -0.03em;
    line-height: 1.1;
  }
  .next {
    margin: 0;
    opacity: 0.75;
  }
  .track {
    height: 4px;
    margin-top: var(--space-3);
    background: color-mix(in srgb, var(--color-background) 20%, transparent);
    border-radius: var(--radius-pill);
    overflow: hidden;
  }
  .track div {
    height: 100%;
    background: var(--color-background);
  }
  .steps {
    list-style: none;
    margin: var(--space-4) 0 0;
    padding: 0;
    font-size: var(--text-sm);
    opacity: 0.8;
  }
  .steps li {
    padding: 2px 0;
  }
  .at {
    display: inline-block;
    width: 3.5em;
    font-variant-numeric: tabular-nums;
    opacity: 0.6;
  }
  .cur {
    font-weight: 600;
  }
  .past {
    text-decoration: line-through;
    opacity: 0.5;
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
    background: var(--color-background);
    color: var(--color-text);
    font-weight: 600;
    cursor: pointer;
  }
  @media (max-width: 600px) {
    .script {
      padding: var(--space-4);
    }
    .big {
      font-size: var(--text-xl);
    }
  }
</style>
