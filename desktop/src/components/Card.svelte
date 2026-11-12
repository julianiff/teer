<script lang="ts">
  // One unit as the preview, the unit view and the flagged list show it:
  // why it has its level, what the agent said, the code change with the
  // statements in it, the note, the answers asked about it. What the agent
  // wrote is plain text. What can be done with the unit goes under its
  // name (`actions`), staying in view as the card scrolls when `sticky`;
  // what can be done with its code goes on the code's head (`codeTools`).
  // A change the agent proposed is said in a line, with `proposed` (the
  // unit view shows the change itself instead).
  import type { Snippet } from "svelte";
  import type { Card } from "../lib/types";
  import Act from "./Act.svelte";
  import Code from "./Code.svelte";
  import Second from "./Second.svelte";
  import type { SecondModel } from "../lib/types";

  let {
    card,
    second = null,
    title = true,
    sticky = false,
    proposed = true,
    actions,
    codeTools,
  }: {
    card: Card;
    second?: SecondModel | null;
    title?: boolean;
    sticky?: boolean;
    proposed?: boolean;
    actions?: Snippet;
    codeTools?: Snippet;
  } = $props();
</script>

<article class="card">
  {#if title}
    <div class="head" class:sticky>
      <div class="title">
        <span class="lvl-{card.level} icon">{card.icon}</span>
        <strong class="name">{card.name}</strong>
        <span class="muted mono place">{card.place}</span>
        <span class="state d-{card.decision}">{card.state}</span>
      </div>
      {@render actions?.()}
    </div>
  {/if}

  {#if card.why.length || card.reasons.length}
    <h3 class="section">Why this level</h3>
    {#if card.why.length}
      <p class="why"><span class="lvl-{card.level}">{card.icon}</span> <span class="muted">{card.why.join(" · ")}</span></p>
    {/if}
    {#each card.reasons as r, i (i)}
      <p class="reason"><span class="muted">·</span> {r}</p>
    {/each}
  {:else}
    <h3 class="section">Why this level</h3>
    <p class="why"><span class="lvl-{card.level}">{card.icon}</span> <span class="muted">{card.level_title}</span></p>
  {/if}
  {#if card.commits.length}
    <p class="commits muted">commits: {card.commits.map((c) => `${c.short} ${c.subject}`).join(" · ")}</p>
  {/if}
  {#if proposed && card.proposed}
    <p class="proposed">
      <span class="pen">✎</span> a change is proposed: {card.proposed} hunk{card.proposed === 1 ? "" : "s"}
      <Act id="tool.fix" label="Show it" kind="link" />
    </p>
  {/if}

  <h3 class="section">What changed</h3>
  {#if card.nbc_reason}<p class="muted">{card.nbc_reason}</p>{/if}
  {#if card.agent_label}
    <div class="agent-said">
      <p class="muted label">{card.agent_label}</p>
      {#if card.summary}<p class="summary">{card.summary}</p>{/if}
      {#if !card.code && card.statements.length}
        <ul class="statements">
          {#each card.statements as s, i (i)}
            <li>
              <span class="risk-{s.risk} sym">{s.symbol}</span>
              <span>{s.text}</span>
              {#if s.cites}<span class="muted cites">{s.cites}</span>{/if}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {:else if card.prep_note}
    <p class="muted">{card.prep === "Pending" ? "⋯ " : ""}{card.prep_note}</p>
  {/if}

  {#if card.code}
    <Code code={card.code} statements={card.statements} tools={codeTools} />
  {/if}

  {#if card.note}
    <div class="note">
      <span class="who">✎ you</span>
      <p>{card.note.text}</p>
      {#if card.note.stale}<p class="muted small">The code changed since this note.</p>{/if}
    </div>
  {/if}

  {#each card.answers as a, i (i)}
    <div class="answer">
      <span class="q">? {a.question}</span>
      <p>{a.answer}</p>
      {#if a.shown}<p class="muted small">{a.shown} piece{a.shown === 1 ? "" : "s"} of code in the conversation</p>{/if}
    </div>
  {/each}

  {#if second}
    <Second {second} />
  {/if}
</article>

<style>
  .card {
    max-width: 980px;
  }
  .head {
    border-bottom: 1px solid var(--border);
    padding-bottom: 6px;
  }
  /* Under the scroll pane's 16px top padding: flush with its edge. */
  .head.sticky {
    position: sticky;
    top: -16px;
    z-index: 2;
    margin-top: -16px;
    padding-top: 16px;
    background: var(--bg);
  }
  .title {
    display: flex;
    gap: 8px;
    align-items: baseline;
  }
  .icon {
    font-size: 14px;
  }
  .name {
    font: 600 15px/1.3 var(--mono);
  }
  .place {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .state {
    margin-left: auto;
    color: var(--muted);
    white-space: nowrap;
  }
  .d-Accepted {
    color: var(--ok);
  }
  .d-Investigate {
    color: var(--flag);
  }
  p {
    margin: 4px 0;
  }
  .reason {
    padding-left: 2px;
  }
  .commits {
    font-size: 12px;
  }
  .proposed {
    font-size: 13px;
  }
  .pen {
    color: var(--accent);
  }
  .agent-said .label {
    font-size: 12px;
    margin-top: 0;
  }
  .summary {
    font-size: 14px;
  }
  .statements {
    list-style: none;
    padding: 0;
    margin: 6px 0;
  }
  .statements li {
    display: flex;
    gap: 8px;
    margin: 3px 0;
  }
  .sym {
    font-family: var(--mono);
    width: 1ch;
  }
  .cites {
    margin-left: auto;
    font-size: 11px;
    white-space: nowrap;
  }
  .note,
  .answer {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 6px 12px 8px;
    margin: 10px 0;
    background: var(--panel);
  }
  .note .who {
    font-weight: 700;
    color: var(--accent);
    font-size: 12px;
  }
  .answer {
    border-color: var(--accent);
  }
  .answer .q {
    font-weight: 600;
    color: var(--accent);
  }
  .small {
    font-size: 12px;
  }
</style>
