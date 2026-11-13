<script lang="ts">
  // The queue: units in level boxes, most attention first; the unit under
  // the cursor beside them. A click selects, a double click looks closer.
  // Over the list, how it is listed and finding a unit; on a row under
  // the mouse, accept and flag; on the preview, all the unit can do.
  // Above the boxes, the attack-surface card: a click on an item opens its
  // unit. By exposure (`zs`), a security box leads the boxes.
  import { api } from "../lib/api";
  import { tip, useActions } from "../lib/actions";
  import { ui } from "../lib/ui.svelte";
  import type { Card as CardT, Queue, QueueRow, SurfaceItem } from "../lib/types";
  import Act from "./Act.svelte";
  import Card from "./Card.svelte";
  import UnitActions from "./UnitActions.svelte";

  let { queue, preview }: { queue: Queue; preview: CardT | null } = $props();
  let list: HTMLElement | undefined = $state();

  // The cursor stays in view.
  $effect(() => {
    void queue.sel;
    list?.querySelector(".row.selected")?.scrollIntoView({ block: "nearest" });
  });

  const byFile = $derived(queue.grouping === "File");
  const actions = useActions();
  const grouping = $derived(actions()["toggle.by_file"]);
  const listBy = (files: boolean) => files !== byFile && void api.action("toggle.by_file");
  // A row's buttons act on it: it is selected first.
  const select = (pos: number) => () => api.click({ kind: "Row", pos });

  function name(r: QueueRow) {
    return byFile || !r.whole_file ? r.name : "whole file";
  }

  const securityOrder = $derived(actions()["toggle.security"]);
  const surfaceRows = $derived(queue.surface?.rows ?? []);
  const weightClass = (it: SurfaceItem) =>
    it.weight === "attention" ? "lvl-Attention" : it.weight === "look" ? "lvl-Look" : "";
</script>

<div class="queue">
  <div class="list scroll" bind:this={list}>
    {#if ui.buttons}
    <div class="listbar">
      {#if grouping}
      <div class="seg" role="group" aria-label="List by" title={tip(grouping)}>
        <button class:on={!byFile} aria-pressed={!byFile} onclick={() => listBy(false)}>Units</button>
        <button class:on={byFile} aria-pressed={byFile} onclick={() => listBy(true)}>Files</button>
      </div>
      {#if grouping.keys}<kbd class="faint">{grouping.keys}</kbd>{/if}
      {/if}
      {#if queue.surface && securityOrder}
        <button
          class="toggle"
          class:on={securityOrder.on}
          aria-pressed={!!securityOrder.on}
          title={tip(securityOrder)}
          onclick={() => api.action("toggle.security")}>Security first</button>
      {/if}
      <span class="spacer"></span>
      <Act id="find.unit" label="Find a unit" icon="⌕" />
    </div>
    {/if}
    {#if queue.surface && surfaceRows.length}
      <section class="box surface" aria-label="Attack surface">
        <h2>attack surface</h2>
        {#each surfaceRows as row (row.class)}
          <div class="srow">
            <span class="slabel muted">{row.label}</span>
            <span class="sitems">
              {#each row.items as it, i (it.unit + it.text)}
                {#if i > 0}<span class="faint"> · </span>{/if}
                <button class="sitem {weightClass(it)}" onclick={() => api.click({ kind: "Open", id: it.unit })}>
                  {#if it.finding}<b>! </b>{:else if it.gone}✗ {/if}{it.text}{#if it.units > 1}<span class="muted"> ×{it.units}</span>{/if}
                </button>
              {/each}
            </span>
          </div>
        {/each}
        <p class="checked muted">checked: {queue.surface.checked} <span class="faint">· not: {queue.surface.not}</span></p>
      </section>
    {:else if queue.surface}
      <p class="checked muted nothing">security: nothing found · checked: {queue.surface.checked} <span class="faint">· not: {queue.surface.not}</span></p>
    {/if}
    {#each queue.boxes as box (box.security ? "security" : box.level)}
      <section class="box">
        <h2 class="lvl-{box.level}">{box.title}</h2>
        {#each box.files as file (file.rows[0]?.pos ?? file.path)}
          <div class="file">
            {#if byFile}
              <div class="file-name" class:done={file.done === file.total}>
                <span class="mono">{file.path}</span>
                <span class="muted count">{file.flagged ? "⚑ " : ""}{file.done}/{file.total}</span>
              </div>
            {/if}
            {#each file.rows as r (r.id)}
              <div class="rowwrap" class:selected={r.selected}>
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
                {#if r.disputed}<span class="warn" title="Disputed">⚠</span>{/if}
                {#if r.noted}<span class="pencil" title="Has a note">✎</span>{/if}
                {#if r.preparing}<span class="dot" title="Being described"></span>{/if}
                <span class="state">
                  {r.state}{#if r.note}<span class="muted"> · “</span><span class="note">{r.note}</span><span class="muted">”</span>{/if}
                </span>
              </button>
              {#if ui.buttons}
                <span class="quick">
                  <Act id="decide.accept" icon="✓" kind="icon" before={select(r.pos)} />
                  <Act id="decide.flag" icon="⚑" kind="icon" before={select(r.pos)} />
                </span>
              {/if}
              </div>
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
    {#if queue.auto_line && !ui.buttons}
      <p class="auto">○ {queue.auto_line}</p>
    {:else if queue.auto_line}
      <button class="auto" aria-pressed={queue.show_auto} onclick={() => api.action("toggle.auto_accepted")}>
        ○ {queue.auto_line}
      </button>
    {/if}
    {#if !queue.boxes.length && !queue.skipped.length}
      <p class="muted empty">Nothing to review.</p>
    {/if}
  </div>
  <div class="preview scroll" data-scroll="preview">
    {#if preview}
      <Card card={preview} sticky>
        {#snippet actions()}<UnitActions on="queue" />{/snippet}
      </Card>
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
  .listbar {
    position: sticky;
    top: -12px;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 8px;
    margin: -12px -12px 10px;
    padding: 10px 12px 8px;
    background: var(--bg);
    border-bottom: 1px solid var(--border);
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--sunken);
    padding: 1px;
  }
  .seg button {
    border: 0;
    background: none;
    border-radius: 5px;
    padding: 1px 10px;
    font-size: 12px;
    line-height: 1.6;
    color: var(--muted);
    cursor: pointer;
  }
  .seg button.on {
    background: var(--panel);
    color: var(--text);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.12);
  }
  .listbar kbd {
    font-size: 10px;
    border-bottom-width: 1px;
    background: transparent;
  }
  .spacer {
    flex: 1;
  }
  .toggle {
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--sunken);
    padding: 1px 10px;
    font-size: 12px;
    line-height: 1.6;
    color: var(--muted);
    cursor: pointer;
  }
  .toggle.on {
    background: var(--panel);
    color: var(--text);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.12);
  }
  .surface {
    padding: 4px 12px 6px;
  }
  .surface h2 {
    margin: 4px 0;
  }
  .srow {
    display: flex;
    gap: 10px;
    align-items: baseline;
    padding: 1px 0;
  }
  .slabel {
    flex: 0 0 96px;
    font-size: 12px;
  }
  .sitems {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sitem {
    border: 0;
    background: none;
    padding: 0;
    font: inherit;
    color: inherit;
    cursor: pointer;
  }
  .sitem:hover {
    text-decoration: underline;
  }
  .checked {
    margin: 4px 0 0;
    font-size: 12px;
  }
  .checked.nothing {
    margin: 0 6px 10px;
  }
  .rowwrap {
    position: relative;
  }
  .quick {
    position: absolute;
    right: 6px;
    top: 50%;
    transform: translateY(-50%);
    display: none;
    gap: 2px;
    padding-left: 18px;
    background: linear-gradient(to right, transparent, var(--sunken) 16px);
  }
  .rowwrap:hover .quick {
    display: flex;
  }
  .rowwrap:hover .row {
    background: var(--sunken);
  }
  .rowwrap:hover .row.selected,
  .rowwrap.selected .quick {
    background: var(--select);
  }
  .rowwrap.selected .quick {
    background: linear-gradient(to right, transparent, var(--select) 16px);
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
  .auto {
    border: 0;
    background: none;
    color: var(--muted);
    padding: 2px 6px;
    border-radius: 5px;
    text-align: left;
    cursor: pointer;
  }
  p.auto {
    margin: 0;
    cursor: default;
  }
  button.auto:hover {
    color: var(--text);
    background: var(--sunken);
  }
  .empty {
    padding: 0 6px;
  }
</style>
