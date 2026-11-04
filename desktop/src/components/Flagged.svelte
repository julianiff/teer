<script lang="ts">
  // The flagged units one after another; the selected one stays in view.
  import { api } from "../lib/api";
  import type { FlaggedScreen } from "../lib/types";
  import Card from "./Card.svelte";

  let { flagged }: { flagged: FlaggedScreen } = $props();
  let list: HTMLElement | undefined = $state();

  $effect(() => {
    void flagged.sel;
    list?.querySelector(".entry.selected")?.scrollIntoView({ block: "start", behavior: "smooth" });
  });
</script>

<div class="flagged scroll" data-scroll="flagged" bind:this={list}>
  <h2>Flagged · {flagged.entries.length}</h2>
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
      <Card {card} />
    </div>
  {/each}
  {#if !flagged.entries.length}
    <p class="muted">Nothing is flagged.</p>
  {/if}
</div>

<style>
  .flagged {
    flex: 1;
    padding: 16px 22px 60px;
  }
  h2 {
    font-size: 14px;
    margin: 0 0 12px;
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
