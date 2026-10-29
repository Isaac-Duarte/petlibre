<script lang="ts">
  // The question a job waits on (`install`'s prompts), with the buttons
  // that answer it. It sits right above the job's output, where the
  // installer prints what goes with a question, such as a recovery code.
  // An answer is sent once: a refused one says why, and the buttons stay
  // off for that question until the server moves on.
  import { AnswerRefused, answer } from "@wellfed/api";
  import type { Answer, PendingPrompt } from "@wellfed/api";
  import { Panel } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";
  import { choices, refusalText } from "../lib/prompt";

  const prompt = $derived(app.state?.prompt ?? null);
  /** The question an answer went out for, `job:seq`. */
  let sent = $state<string | null>(null);
  let refused = $state<string | null>(null);

  const key = (asked: PendingPrompt) => `${String(asked.job)}:${String(asked.seq)}`;

  async function send(asked: PendingPrompt, reply: Answer) {
    sent = key(asked);
    refused = null;
    try {
      await answer(asked.job, asked.seq, reply);
    } catch (error) {
      refused = error instanceof AnswerRefused ? refusalText(error.refusal) : String(error);
    }
  }
</script>

{#if prompt}
  {@const asked = prompt}
  <Panel title="Job {asked.job} asks">
    {#snippet meta()}question {asked.seq}, {asked.prompt.kind}{/snippet}
    <div class="prompt" role="alert">
      <p>{asked.text}</p>
      <div class="buttons">
        {#each choices(asked.expects) as choice (choice.answer)}
          <button
            type="button"
            disabled={sent === key(asked) || !app.connected}
            onclick={() => void send(asked, choice.answer)}
          >
            {choice.label}
          </button>
        {/each}
      </div>
      {#if refused && sent === key(asked)}
        <p class="refused" role="status">Not taken: {refused}</p>
      {/if}
    </div>
  </Panel>
{/if}

<style>
  .prompt {
    border-left: 3px solid var(--color-warning);
    padding-left: var(--space-4);
  }
  p {
    margin: 0;
    line-height: 1.5;
  }
  .buttons {
    display: flex;
    gap: var(--space-3);
    margin-top: var(--space-4);
  }
  button {
    min-width: 88px;
    min-height: 36px;
    padding: 0 var(--space-4);
    border: 1px solid var(--color-line);
    border-radius: var(--radius-card);
    background: var(--color-surface);
    font-weight: 600;
    cursor: pointer;
  }
  button:disabled {
    color: var(--color-text-muted);
    cursor: default;
  }
  .refused {
    margin-top: var(--space-3);
    color: var(--color-fault);
    font-size: var(--text-sm);
  }
  @media (max-width: 600px) {
    button {
      min-height: 44px;
      flex: 1;
    }
  }
</style>
