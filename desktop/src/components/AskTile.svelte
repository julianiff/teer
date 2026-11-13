<script lang="ts">
  // The agent on the unit: its advice (what to do about it) on top, then
  // the conversation, oldest first, the questions offered to ask next
  // (the advice, the security checks, what the last answer leads to), and
  // the question being typed. Links go to units of this change or to the code shown.
  import { api } from "../lib/api";
  import type { AskTile } from "../lib/types";
  import Second from "./Second.svelte";
  import ShownCode from "./ShownCode.svelte";

  let { ask, typing }: { ask: AskTile; typing: boolean } = $props();
</script>

{#if ask.advice}
  {@const a = ask.advice}
  {#if a.result}
    <div class="advice">
      <h4>Advice <span class="muted small">{a.label}</span></h4>
      <p class="answer">{a.result.text}</p>
      {#if a.result.sources.length}
        <ul>{#each a.result.sources as s, i (i)}<li class="muted small">{s}</li>{/each}</ul>
      {/if}
    </div>
  {/if}
  {#if a.waiting}<p class="muted small">⋯ {a.waiting}</p>{/if}
  {#if a.second}<Second second={a.second} />{/if}
{/if}

{#each ask.exchanges as x (x.index)}
  <div class="exchange">
    <p class="q">› {x.question}</p>
    {#if x.state.kind === "Asking"}
      <p class="muted small">⋯ {x.state.step ? `step ${x.state.steps}: ${x.state.step}` : "asking the agent…"}</p>
    {:else if x.state.kind === "Writing"}
      <p class="muted small">⋯ writing…</p>
    {:else if x.state.kind === "Failed"}
      <p class="bad small">✗ {x.state.error}</p>
    {:else if x.state.kind === "NotAnswered"}
      <p class="muted small">not answered</p>
    {/if}
    {#if x.answer !== null}
      <div class="agent-said">
        <p class="muted small">{ask.label}</p>
        <p class="answer">{x.answer}</p>
      </div>
    {/if}
    {#if x.related.length}
      <h4>In this change</h4>
      {#each x.related as r (r.id)}
        <button
          class="related"
          class:selected={r.link !== null && r.link === ask.link}
          disabled={r.link === null}
          onclick={() => r.link !== null && api.click({ kind: "Link", i: r.link })}
        >
          <span class="lvl-{r.level ?? 'Routine'}">●</span>
          <span class="mono">{r.name}</span>
          <span class="muted small">{r.place} · {r.why}</span>
        </button>
      {/each}
    {/if}
    {#each x.shown as s (s.link)}
      <div role="button" tabindex="-1" onclick={() => api.click({ kind: "Link", i: s.link })} onkeydown={() => {}}>
        <ShownCode shown={s.shown} selected={s.link === ask.link} />
      </div>
    {/each}
  </div>
{:else}
  <p class="muted">Ask anything else about this unit; the agent reads the code to answer.</p>
  <p class="muted small">Not sure yet? Bring more light into it:</p>
  {#each ask.tools as t (t.action)}
    <button class="tool" onclick={() => api.action(t.action)}><span class="key">{t.keys}</span> {t.what}</button>
  {/each}
{/each}

{#if ask.suggestions.length}
  <p class="muted small next">Ask next</p>
  {#each ask.suggestions as s, i (s.text)}
    <button class="tool" onclick={() => api.click({ kind: "Suggest", i })}>
      <span class="key">{i + 1}</span>
      <span>{s.text}{#if s.about} <span class="muted small">{s.about}</span>{/if}</span>
    </button>
  {/each}
{/if}

<div class="input" class:typing>
  <span class="prompt">›</span>
  <span class="draft">{ask.draft}</span>{#if typing}<span class="cursor"></span>{/if}
  {#if !typing && !ask.draft}<span class="muted small">K or i types a question</span>{/if}
</div>
{#if ask.web_may_ask}<p class="muted small">The first question that could use the web asks before it does.</p>{/if}

<style>
  .exchange,
  .advice {
    margin-bottom: 14px;
  }
  .advice ul {
    padding-left: 16px;
    margin: 4px 0;
  }
  .next {
    margin: 10px 0 2px;
  }
  .q {
    font-weight: 600;
    margin: 0 0 4px;
  }
  .answer {
    white-space: pre-wrap;
    margin: 0;
  }
  .small {
    font-size: 11px;
  }
  .bad {
    color: var(--bad);
  }
  h4 {
    font-size: 12px;
    margin: 8px 0 2px;
  }
  .related,
  .tool {
    display: flex;
    gap: 6px;
    align-items: baseline;
    width: 100%;
    text-align: left;
    border: 0;
    background: none;
    padding: 2px 4px;
    border-radius: 5px;
    cursor: pointer;
  }
  .related.selected {
    background: var(--select);
  }
  .input {
    position: sticky;
    bottom: -16px;
    margin-top: 10px;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--panel);
  }
  .input.typing {
    border-color: var(--accent);
  }
  .prompt {
    color: var(--accent);
    font-weight: 700;
  }
  .draft {
    white-space: pre-wrap;
  }
  .cursor {
    display: inline-block;
    width: 7px;
    height: 14px;
    background: var(--accent);
    vertical-align: -2px;
  }
</style>
