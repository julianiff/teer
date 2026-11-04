<script lang="ts">
  // The commit message or the review summary, written in the window
  // instead of $EDITOR. ⌘⏎ saves, esc cancels.
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";

  let {
    text,
    title,
    onclose,
  }: { text: string; title: string; onclose: () => void } = $props();
  let value = $state("");
  let field: HTMLTextAreaElement | undefined = $state();

  onMount(() => {
    value = text;
    field?.focus();
  });

  function done(save: boolean) {
    void invoke("written", { text: save ? value : null });
    onclose();
  }

  function key(e: KeyboardEvent) {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      done(true);
    } else if (e.key === "Escape") {
      e.preventDefault();
      done(false);
    }
  }
</script>

<div class="backdrop" data-own-keys>
  <div class="editor" role="dialog" aria-modal="true">
    <h2>{title}</h2>
    <textarea bind:this={field} bind:value onkeydown={key} spellcheck="true"></textarea>
    <div class="buttons">
      <span class="muted">⌘⏎ saves · esc cancels</span>
      <span class="spacer"></span>
      <button class="btn" onclick={() => done(false)}>Cancel</button>
      <button class="btn primary" onclick={() => done(true)}>Save</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 25;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 8vh;
    background: color-mix(in srgb, var(--bg) 55%, transparent);
  }
  .editor {
    width: min(760px, calc(100% - 32px));
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 16px 18px;
    box-shadow: 0 18px 60px rgba(0, 0, 0, 0.3);
  }
  h2 {
    font-size: 15px;
    margin: 0 0 10px;
  }
  textarea {
    width: 100%;
    height: 46vh;
    resize: vertical;
    font: 13px/1.55 var(--mono);
    color: var(--text);
    background: var(--sunken);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px 12px;
    outline: none;
  }
  textarea:focus {
    border-color: var(--accent);
  }
  .buttons {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 10px;
  }
  .spacer {
    flex: 1;
  }
</style>
