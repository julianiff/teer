<script lang="ts">
  // ⌘K: every action, found by what it does or its keys, run on ⏎.
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import type { Hint } from "../lib/types";

  let { onclose }: { onclose: () => void } = $props();
  let all = $state<Hint[]>([]);
  let query = $state("");
  let sel = $state(0);
  let field: HTMLInputElement | undefined = $state();

  onMount(async () => {
    field?.focus();
    all = await api.actions();
  });

  const found = $derived(
    all.filter((h) => {
      const q = query.trim().toLowerCase();
      return !q || h.what.toLowerCase().includes(q) || h.keys.toLowerCase().includes(q) || h.action.includes(q);
    }),
  );

  function run(h: Hint | undefined) {
    if (!h) return;
    onclose();
    void api.action(h.action);
  }

  function key(e: KeyboardEvent) {
    if (e.key === "Escape") onclose();
    else if (e.key === "ArrowDown") sel = Math.min(sel + 1, found.length - 1);
    else if (e.key === "ArrowUp") sel = Math.max(sel - 1, 0);
    else if (e.key === "Enter") run(found[sel]);
    else return;
    e.preventDefault();
  }
</script>

<div class="backdrop" role="presentation" onclick={onclose}>
  <div class="palette" role="dialog" aria-modal="true" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
    <input type="text" bind:this={field} bind:value={query} placeholder="What do you want to do?" onkeydown={key} oninput={() => (sel = 0)} />
    <ul>
      {#each found as h, i (h.action)}
        <li>
          <button class:selected={i === sel} onclick={() => run(h)} onmouseenter={() => (sel = i)}>
            <span>{h.what}</span>
            <span class="key">{h.keys}</span>
          </button>
        </li>
      {/each}
    </ul>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
    background: color-mix(in srgb, var(--bg) 40%, transparent);
    z-index: 20;
  }
  .palette {
    width: min(560px, calc(100% - 32px));
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 18px 60px rgba(0, 0, 0, 0.3);
    overflow: hidden;
  }
  input {
    width: 100%;
    border: 0 !important;
    border-bottom: 1px solid var(--border) !important;
    border-radius: 0 !important;
    padding: 12px 14px !important;
    font-size: 15px !important;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 6px;
    max-height: 50vh;
    overflow: auto;
  }
  button {
    display: flex;
    justify-content: space-between;
    width: 100%;
    border: 0;
    background: none;
    padding: 6px 10px;
    border-radius: 6px;
    text-align: left;
    cursor: pointer;
  }
  button.selected {
    background: var(--select);
  }
</style>
