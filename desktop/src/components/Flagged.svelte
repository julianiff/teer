<script lang="ts">
  // The flagged units one after another; the selected one stays in view
  // and carries what can be done with it. Over the list, the way back and
  // the flagged units before and after.
  import { api } from "../lib/api";
  import type { FlaggedScreen } from "../lib/types";
  import Act from "./Act.svelte";
  import Card from "./Card.svelte";
  import UnitActions from "./UnitActions.svelte";

  let { flagged }: { flagged: FlaggedScreen } = $props();
  let list: HTMLElement | undefined = $state();

  $effect(() => {
    void flagged.sel;
    list?.querySelector(".entry.selected")?.scrollIntoView({ block: "start", behavior: "smooth" });
  });
</script>

{#snippet selected()}<UnitActions on="flagged" />{/snippet}

<div class="screen">
  <nav class="top" aria-label="The flagged list">
    <Act id="view.back" label="Back" icon="←" kind="icon" />
    <h2>Flagged · {flagged.entries.length}</h2>
    <span class="spacer"></span>
    <Act id="move.prev_pending" label="Previous" icon="↑" kind="icon" />
    <Act id="move.next_pending" label="Next" icon="↓" kind="icon" />
  </nav>
  <div class="flagged scroll" data-scroll="flagged" bind:this={list}>
    {#each flagged.entries as card, i (card.id)}
      <div
        class="entry"
        class:selected={i === flagged.sel}
        role="button"
        tabindex="-1"
        onclick={() => api.click({ kind: "Flagged", i })}
        ondblclick={() => api.click({ kind: "Open", id: card.id })}
        onkeydown={() => {}}
      >
        <Card {card} actions={i === flagged.sel ? selected : undefined} />
      </div>
    {/each}
    {#if !flagged.entries.length}
      <p class="muted">Nothing is flagged.</p>
    {/if}
  </div>
</div>

<style>
  .screen {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
  }
  .spacer {
    flex: 1;
  }
  h2 {
    font-size: 13px;
    margin: 0 0 0 6px;
  }
  .flagged {
    flex: 1;
    min-height: 0;
    padding: 16px 22px 60px;
  }
  .entry {
    border: 1px solid var(--border);
    border-left: 3px solid var(--border);
    border-radius: 9px;
    padding: 12px 16px;
    margin-bottom: 14px;
    background: var(--panel);
  }
  .entry.selected {
    border-left-color: var(--accent);
    background: var(--select);
  }
</style>
