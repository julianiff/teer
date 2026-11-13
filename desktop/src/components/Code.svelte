<script lang="ts">
  // A unit's code change: diff rows coloured by syntax class, each
  // comment of the mode written in under the row it cites. A proposed hunk is drawn
  // the same way, under its own `title`; the `current` one (that
  // `fix.apply` puts in) has the accent border.
  // `tools`: what can be done with it, on its head; `caption`: a line
  // under the head (why a proposed hunk is there).
  import type { Snippet } from "svelte";
  import type { Code, CodeNote } from "../lib/types";
  let {
    code,
    tools,
    caption,
    title = "code change",
    current = false,
  }: {
    code: Code;
    tools?: Snippet;
    caption?: Snippet;
    title?: string;
    current?: boolean;
  } = $props();

  const width = $derived(
    Math.max(3, ...code.rows.map((r) => String(r.line ?? r.old ?? "").length)),
  );
  const after = (k: number | null) => code.notes.filter((n) => n.after === k);
  const hunk = (k: number) => code.hunks.includes(k);
</script>

{#snippet said(n: CodeNote)}
  <div class="said">
    {#if n.symbol}<span class={n.risk ? `risk-${n.risk}` : ""}>{n.symbol}</span>{/if}
    <div class="body">
      {#if n.title}<div class="title {n.risk ? `risk-${n.risk}` : 'muted'}">{n.title}</div>{/if}
      {#each n.text as p, i (i)}<p>{p}</p>{/each}
    </div>
    <span class="muted cites">{n.cites}</span>
  </div>
{/snippet}

<div class="code" class:current>
  <div class="head">
    <span>{title}</span>
    {#if tools}<span class="tools">{@render tools()}</span>{/if}
    <span class="counts"><span class="add">+{code.added}</span> <span class="del">−{code.removed}</span></span>
  </div>
  {#if caption}<div class="caption">{@render caption()}</div>{/if}
  <div class="rows" style="--digits: {width}ch">
    {#each code.rows as row, k (k)}
      {#if row.kind === "Gap"}
        <div class="row gap"><span class="num">…</span></div>
      {:else}
        <div class="row {row.kind}" data-hunk={hunk(k) ? "" : undefined}>
          <span class="num" class:old={row.kind === "Removed"}>{row.kind === "Removed" ? row.old : row.line}</span>
          <span class="sign">{row.kind === "Added" ? "+" : row.kind === "Removed" ? "-" : " "}</span>
          <span class="text">{#each row.pieces as p, i (i)}<span class={p.class ? `c-${p.class}` : ""}>{p.text}</span>{/each}</span>
        </div>
      {/if}
      {#each after(k) as n, i (i)}{@render said(n)}{/each}
    {/each}
    {#each after(null) as n, i (i)}{@render said(n)}{/each}
  </div>
</div>

<style>
  .code {
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
    background: var(--panel);
    margin: 10px 0;
  }
  .code.current {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 10px;
    font-size: 11px;
    color: var(--muted);
    border-bottom: 1px solid var(--border);
    background: var(--sunken);
  }
  .caption {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    padding: 4px 10px 0;
    font-size: 12px;
  }
  .tools {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 4px;
  }
  .counts {
    margin-left: auto;
  }
  .add {
    color: var(--added-sign);
  }
  .del {
    color: var(--removed-sign);
  }
  .rows {
    font: 12.5px/1.55 var(--mono);
    overflow-x: auto;
    padding: 4px 0;
  }
  .row {
    display: flex;
    white-space: pre;
    min-width: max-content;
  }
  .row.Added {
    background: var(--added);
  }
  .row.Removed {
    background: var(--removed);
  }
  .num {
    width: calc(var(--digits) + 1.2ch);
    padding-right: 1ch;
    text-align: right;
    color: var(--faint);
    user-select: none;
    flex: none;
  }
  .num.old {
    font-style: italic;
  }
  .sign {
    width: 2ch;
    flex: none;
    user-select: none;
  }
  .Added .sign {
    color: var(--added-sign);
  }
  .Removed .sign {
    color: var(--removed-sign);
  }
  .gap .num {
    color: var(--faint);
  }
  .said {
    margin: 4px 12px 6px calc(var(--digits) + 2.2ch);
    padding: 6px 10px;
    border: 1px solid var(--note-border);
    background: var(--note);
    border-radius: 7px;
    font: 13px/1.45 var(--sans);
    white-space: normal;
    display: flex;
    gap: 8px;
    max-width: 72ch;
  }
  .said p {
    margin: 0;
  }
  .said p + p {
    margin-top: 2px;
  }
  .title {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.02em;
  }
  .cites {
    margin-left: auto;
    white-space: nowrap;
    font-size: 11px;
  }
</style>
