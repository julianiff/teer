<script lang="ts">
  // One unit of the file on the queue's right side: a slim head (its
  // level, its name, where it stands), one line on what it does, and its
  // code change with the statements written in. A click on the head opens
  // the unit view. The `selected` one is the unit open and note act on:
  // their buttons are on its head.
  import { api } from "../lib/api";
  import type { Card } from "../lib/types";
  import Act from "./Act.svelte";
  import Code from "./Code.svelte";
  import Mark from "./Mark.svelte";

  let { card, selected = false, marked = false }: { card: Card; selected?: boolean; marked?: boolean } = $props();

  const line = $derived(card.summary ?? card.nbc_reason ?? card.prep_note);
</script>

<section class="unit" class:marked data-unit={card.id} aria-label={card.name}>
  <div class="head">
    <button class="open" title="Open {card.name}" onclick={() => api.click({ kind: "Open", id: card.id })}>
      <Mark level={card.level} calm={card.decision === "Accepted"} />
      <span class="name" class:done={card.decision === "Accepted"}>{card.name}</span>
    </button>
    {#if card.decision === "Accepted"}
      <span class="ok" title="Accepted">✓</span>
    {:else if card.decision === "Investigate"}
      <span class="flag" title="Flagged">⚑</span>
    {/if}
    {#if card.disputed}<span class="warn" title="A second model disputes the description">⚠</span>{/if}
    {#if card.note}<span class="pen" title={card.note.text}>✎</span>{/if}
    {#if selected}
      <span class="tools">
        <Act id="view.open" label="Open" kind="icon" />
        <Act id="note.edit" label="Note" kind="icon" />
      </span>
    {/if}
  </div>
  {#if line}<p class="line muted" title={line}>{line}</p>{/if}
  {#if card.code}
    <Code code={card.code} bare />
  {:else if card.statements.length}
    <ul class="statements">
      {#each card.statements as s, i (i)}
        <li><span class="risk-{s.risk} mono">{s.symbol}</span> <span>{s.text}</span></li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .unit {
    /* Under the file's head when scrolled to. */
    scroll-margin-top: 56px;
    padding: 10px 0 6px 12px;
    border-left: 2px solid transparent;
    max-width: 980px;
  }
  .unit.marked {
    border-left-color: var(--accent);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 24px;
  }
  .open {
    display: inline-flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
    border: 0;
    background: none;
    padding: 0;
    cursor: pointer;
  }
  .name {
    font: 600 13px/1.4 var(--mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name.done {
    color: var(--muted);
    font-weight: 400;
  }
  .open:hover .name {
    color: var(--accent);
  }
  .ok {
    color: var(--ok);
  }
  .flag {
    color: var(--flag);
  }
  .warn {
    color: var(--look);
  }
  .pen {
    color: var(--accent);
  }
  .tools {
    display: flex;
    gap: 2px;
  }
  .line {
    margin: 0 0 0 20px;
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow: hidden;
  }
  .statements {
    list-style: none;
    margin: 4px 0 0 20px;
    padding: 0;
  }
  .unit :global(.code) {
    margin: 6px 0 0;
  }
</style>
