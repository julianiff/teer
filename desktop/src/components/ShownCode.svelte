<script lang="ts">
  // Lines of a file an answer shows, as teer read them.
  import type { Shown } from "../lib/types";
  let { shown, selected = false }: { shown: Shown; selected?: boolean } = $props();
</script>

<div class="shown" class:selected>
  <div class="head">
    <span class="mono">{shown.cite.file}:{shown.cite.start}{shown.cite.end > shown.cite.start ? `-${shown.cite.end}` : ""}</span>
    {#if shown.in_change}<span class="tag">in this change</span>{/if}
  </div>
  {#if shown.why}<p class="why muted">{shown.why}</p>{/if}
  <pre>{#each shown.code as line, i (i)}<span class:changed={shown.changed?.includes(shown.cite.start + i)}><span class="num">{shown.cite.start + i}</span>{line}
</span>{/each}</pre>
</div>

<style>
  .shown {
    border: 1px solid var(--border);
    border-radius: 7px;
    margin: 8px 0;
    overflow: hidden;
  }
  .shown.selected {
    border-color: var(--accent);
  }
  .head {
    display: flex;
    justify-content: space-between;
    font-size: 11px;
    padding: 3px 8px;
    background: var(--sunken);
  }
  .tag {
    color: var(--accent);
  }
  .why {
    margin: 4px 8px;
    font-size: 12px;
  }
  pre {
    margin: 0;
    padding: 4px 0;
    font: 12px/1.5 var(--mono);
    overflow-x: auto;
  }
  .num {
    display: inline-block;
    width: 5ch;
    text-align: right;
    padding-right: 1ch;
    color: var(--faint);
    user-select: none;
  }
  .changed {
    background: var(--added);
    display: inline-block;
    min-width: 100%;
  }
</style>
