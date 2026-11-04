<script lang="ts">
  // The bottom bar: the counts, what the keys wait for, the status. The
  // flagged and the noted counts open their lists.
  import { api } from "../lib/api";
  import type { Bar } from "../lib/types";
  import Act from "./Act.svelte";
  let { bar }: { bar: Bar } = $props();
</script>

{#snippet counts(zero: boolean)}
  {#if bar.flagged > 0}
    <span class="sep">·</span>
    <span class="flagged">
      <Act id="tool.flagged" label="{bar.flagged} flagged" kind="link" showKey={false} title="the flagged list" />
    </span>
  {:else if zero}
    <span class="sep">·</span>
    <span>0 flagged</span>
  {/if}
  {#if bar.notes > 0}
    <span class="sep">·</span>
    <Act id="tool.notes" label="{bar.notes} note{bar.notes === 1 ? '' : 's'}" kind="link" showKey={false} />
  {/if}
{/snippet}

<footer>
  {#if bar.note}
    <span class="label">Note on <strong>{bar.note.name}</strong></span>
    <span class="typed">{bar.note.text}<span class="cursor"></span></span>
    <span class="spacer"></span>
    <span class="muted">⏎ saves · esc cancels · {bar.note.text.length}/{bar.note.max}</span>
  {:else}
    {#if bar.pending > 0}
      <span><strong>{bar.pending} pending</strong></span>
      {@render counts(true)}
      <span class="sep">·</span>
      <span class="muted">{bar.accepted} accepted</span>
    {:else}
      <strong>Nothing pending.</strong>
      <button class="key" onclick={() => api.action("finish.open")}>{bar.finish_key}</button>
      <strong>to finish</strong>
      {@render counts(false)}
    {/if}

    {#if bar.command !== null}
      <span class="command mono">:{bar.command}<span class="cursor"></span></span>
      {#each bar.commands as c (c.keys)}
        <button class="hint" onclick={() => api.action(c.action)}>
          <span class="key">{c.keys}</span> <span class="muted">{c.what}</span>
        </button>
      {/each}
    {:else if bar.typed}
      <span class="typed mono">{bar.typed}</span>
      {#each bar.follow as h (h.keys + h.action)}
        <button class="hint" onclick={() => api.action(h.action)}>
          <span class="key">{h.keys}</span> {h.what}
        </button>
      {/each}
    {:else if bar.visual !== null}
      <strong class="visual">-- VISUAL --</strong>
      {#each bar.visual_actions as h (h.action)}
        <button class="hint" onclick={() => api.action(h.action)}>
          <span class="key">{h.keys}</span> {h.what}
        </button>
      {/each}
      <span class="muted">{bar.visual} unit{bar.visual === 1 ? "" : "s"}</span>
    {/if}

    <span class="spacer"></span>
    {#if bar.waiting}
      <span class="waiting">{bar.waiting}</span>
    {:else if bar.status}
      <span class="status muted">{bar.status}</span>
    {:else}
      <button class="hint" onclick={() => api.action("help.open")}>
        <span class="key">{bar.help_key}</span> <span class="muted">keys</span>
      </button>
    {/if}
  {/if}
</footer>

<style>
  footer {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 6px 14px;
    border-top: 1px solid var(--border);
    background: var(--panel);
    min-height: 34px;
    white-space: nowrap;
    overflow: hidden;
  }
  .sep {
    color: var(--faint);
  }
  .flagged {
    color: var(--flag);
  }
  .spacer {
    flex: 1;
  }
  .status,
  .waiting {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .waiting {
    color: var(--accent);
  }
  .typed,
  .command {
    margin-left: 14px;
    color: var(--accent);
    font-weight: 600;
  }
  .hint {
    border: 0;
    background: none;
    padding: 0 4px;
    cursor: pointer;
  }
  button.key {
    cursor: pointer;
  }
  .visual {
    margin-left: 14px;
  }
  .cursor {
    display: inline-block;
    width: 7px;
    height: 14px;
    background: var(--accent);
    vertical-align: -2px;
    margin-left: 1px;
  }
  .label {
    color: var(--muted);
  }
</style>
