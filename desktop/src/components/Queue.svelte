<script lang="ts">
  // The queue: units in level boxes, most attention first; the unit under
  // the cursor beside them. A click selects, a double click looks closer.
  import { api } from "../lib/api";
  import type { Card as CardT, Queue, QueueRow } from "../lib/types";
  import Card from "./Card.svelte";

  let { queue, preview }: { queue: Queue; preview: CardT | null } = $props();
  let list: HTMLElement | undefined = $state();

  // The cursor stays in view.
  $effect(() => {
    void queue.sel;
    list?.querySelector(".row.selected")?.scrollIntoView({ block: "nearest" });
  });

  const byFile = $derived(queue.grouping === "File");

  function name(r: QueueRow) {
    return byFile || !r.whole_file ? r.name : "whole file";
  }
</script>

<div class="queue">
  <div class="list scroll" bind:this={list}>
    {#each queue.boxes as box (box.level)}
      <section class="box">
        <h2 class="lvl-{box.level}">{box.title}</h2>
        {#each box.files as file (box.level + file.path)}
          <div class="file">
            {#if byFile}
              <div class="file-name" class:done={file.done === file.total}>
                <span class="mono">{file.path}</span>
                <span class="muted count">{file.flagged ? "⚑ " : ""}{file.done}/{file.total}</span>
              </div>
            {/if}
            {#each file.rows as r (r.id)}
              <button
                class="row d-{r.decision}"
                class:selected={r.selected}
                class:visual={r.in_visual}
                onclick={() => api.click({ kind: "Row", pos: r.pos })}
                ondblclick={() => api.click({ kind: "Open", id: r.id })}
              >
                <span class="icon lvl-{r.decision === 'Accepted' ? 'Same' : r.level}">{r.icon}</span>
                {#if !byFile}<span class="path mono">{r.path}</span>{/if}
                <span class="name" class:removed={r.removed} class:whole={r.whole_file && !byFile}>
                  {#if r.depth > 0}<span class="faint">{"  ".repeat(Math.min(r.depth, 3) - 1)}↳ </span>{/if}{name(r)}
                </span>
                {#if r.changed}<span class="tag">changed</span>{/if}
                {#if r.disputed}<span class="warn" title="A second model disputes it">⚠</span>{/if}
                {#if r.noted}<span class="pencil" title="Has a note">✎</span>{/if}
                {#if r.preparing}<span class="dot" title="Being described"></span>{/if}
                <span class="state">
                  {r.state}{#if r.note}<span class="muted"> · “</span><span class="note">{r.note}</span><span class="muted">”</span>{/if}
                </span>
              </button>
            {/each}
          </div>
        {/each}
      </section>
    {/each}
    {#if queue.skipped.length}
      <section class="box skipped">
        <h2 class="lvl-Same">Not shown · {queue.skipped.length} file{queue.skipped.length === 1 ? "" : "s"}</h2>
        {#each queue.skipped as s (s.path)}
          <div class="row muted"><span class="mono">{s.path}</span> <span>{s.why}</span></div>
        {/each}
      </section>
    {/if}
    {#if queue.auto_line}
      <p class="auto muted">○ {queue.auto_line}</p>
    {/if}
    {#if !queue.boxes.length && !queue.skipped.length}
      <p class="muted empty">Nothing to review.</p>
    {/if}
  </div>
  <div class="preview scroll" data-scroll="preview">
    {#if preview}
      <Card card={preview} />
    {/if}
  </div>
</div>

<style>
  .queue {
    flex: 1;
    display: flex;
    min-width: 0;
  }
  .list {
    flex: 0 0 min(46%, 640px);
    padding: 12px;
    border-right: 1px solid var(--border);
  }
  .preview {
    flex: 1;
    min-width: 0;
    padding: 16px 22px 40px;
  }
  .box {
    border: 1px solid var(--border);
    border-radius: 9px;
    background: var(--panel);
    margin-bottom: 12px;
    padding: 4px 0 6px;
  }
  .box h2 {
    font-size: 12px;
    font-weight: 700;
    margin: 4px 12px 4px;
  }
  .file + .file {
    margin-top: 2px;
  }
  .file-name {
    display: flex;
    justify-content: space-between;
    padding: 6px 12px 2px;
    font-weight: 600;
  }
  .file-name.done {
    color: var(--muted);
  }
  .row {
    display: flex;
    gap: 8px;
    align-items: baseline;
    width: 100%;
    text-align: left;
    background: none;
    border: 0;
    border-left: 3px solid transparent;
    padding: 3px 12px 3px 9px;
    cursor: default;
    white-space: nowrap;
  }
  .row:hover {
    background: var(--sunken);
  }
  .row.selected {
    background: var(--select);
    border-left-color: var(--accent);
  }
  .row.visual {
    background: var(--visual);
    border-left-color: var(--accent);
  }
  .icon {
    width: 12px;
    text-align: center;
  }
  .path {
    color: var(--muted);
    font-size: 12px;
    max-width: 40%;
    overflow: hidden;
    text-overflow: ellipsis;
    direction: rtl;
    text-align: left;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .name.removed {
    text-decoration: line-through;
  }
  .name.whole {
    color: var(--muted);
  }
  .row.selected .name {
    font-weight: 600;
  }
  .d-Accepted .name,
  .d-Accepted .path {
    color: var(--faint);
  }
  .d-Investigate .name {
    color: var(--flag);
  }
  .state {
    margin-left: auto;
    padding-left: 10px;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .d-Accepted .state {
    color: var(--ok);
  }
  .d-Investigate .state {
    color: var(--flag);
  }
  .note {
    color: var(--text);
  }
  .warn {
    color: var(--look);
  }
  .pencil {
    color: var(--accent);
  }
  .tag {
    font-size: 11px;
    color: var(--accent);
    border: 1px solid var(--accent);
    border-radius: 4px;
    padding: 0 4px;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    align-self: center;
    animation: pulse 1s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.2;
    }
  }
  .auto,
  .empty {
    padding: 0 6px;
  }
</style>
