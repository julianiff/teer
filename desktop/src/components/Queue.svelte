<script lang="ts">
  // The queue: the tree of the changed files on the left, the selected
  // file's code changes on the right. In front of each file, its level as
  // a mark: what needs a look stands out, the routine stays quiet; a
  // folded directory has the mark of what is under it. After the name,
  // only what there is to act on or know: flagged, disputed, changed,
  // being described. A file with every unit accepted is dimmed.
  // A click selects a row (a directory folds or opens), a double click
  // opens the file. On a file's row under the mouse and on the file's
  // head, accept and flag; on the selected unit's head, open and note.
  // Under the tree, the units accepted for you and finding a unit; over
  // it, the attack-surface card while the security lens is on.
  import { api } from "../lib/api";
  import { ui } from "../lib/ui.svelte";
  import type { FilePreview, Queue, SurfaceItem, TreeRow } from "../lib/types";
  import Act from "./Act.svelte";
  import Mark from "./Mark.svelte";
  import UnitChange from "./UnitChange.svelte";

  let { queue, preview, auto }: { queue: Queue; preview: FilePreview | null; auto: number } = $props();
  let tree: HTMLElement | undefined = $state();
  let pane: HTMLElement | undefined = $state();

  const at = $derived(queue.rows.find((r) => r.selected) ?? null);

  // The cursor stays in view.
  $effect(() => {
    void at?.id;
    tree?.querySelector(".row.selected")?.scrollIntoView({ block: "nearest" });
  });

  // Another file starts at the top; a unit further down comes into view.
  const shown = $derived(preview ? `${preview.path}\n${preview.selected}` : "");
  $effect(() => {
    void shown;
    if (!pane || !preview) return;
    const first = preview.cards[0]?.id === preview.selected;
    const unit = [...pane.querySelectorAll<HTMLElement>("[data-unit]")].find(
      (u) => u.dataset.unit === preview.selected,
    );
    if (first || !unit) pane.scrollTo({ top: 0 });
    else unit.scrollIntoView({ block: "start" });
  });

  // A row's buttons act on it: it is selected first.
  const select = (id: string) => () => api.click({ kind: "Row", id });
  // A directory's mark: only while it is folded.
  const mark = (r: TreeRow) => (r.kind === "Dir" ? r.folded : r.kind === "File") && r.level;
  const slash = $derived(preview ? preview.path.lastIndexOf("/") + 1 : 0);

  const surfaceRows = $derived(queue.surface?.rows ?? []);
  const weightClass = (it: SurfaceItem) =>
    it.weight === "attention" ? "lvl-Attention" : it.weight === "look" ? "lvl-Look" : "";
</script>

<div class="queue">
  <div class="side">
    <div class="tree scroll" bind:this={tree}>
      {#if queue.surface}
        <section class="surface" aria-label="Attack surface">
          <h2>
            <span>Attack surface</span>
            <Act id="toggle.security" icon="✕" kind="icon" showKey={false} title="hide the attack surface" />
          </h2>
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
          {#if !surfaceRows.length}<p class="checked">Nothing found.</p>{/if}
          <p class="checked muted">Checked: {queue.surface.checked}</p>
          <p class="checked faint">Not checked: {queue.surface.not}</p>
        </section>
      {/if}
      {#each queue.rows as r (r.id)}
        <div class="rowwrap" class:selected={r.selected}>
          <button
            class="row {r.kind}"
            class:selected={r.selected}
            class:visual={r.in_visual}
            class:done={r.file?.done}
            aria-expanded={r.kind === "Dir" ? !r.folded : undefined}
            title={r.why ?? r.path}
            onclick={() => api.click({ kind: "Row", id: r.id })}
            ondblclick={() => r.kind === "File" && api.action("view.open")}
          >
            <span class="gutter">
              {#if mark(r)}<Mark level={r.level!} calm={r.file?.done || r.settled} />{/if}
            </span>
            <span class="indent" style="width: {r.depth * 14}px"></span>
            <span class="tri" class:open={!r.folded} aria-hidden="true">{#if r.kind === "Dir"}<i></i>{/if}</span>
            <span class="name">{r.name}</span>
            {#if r.file?.flagged}<span class="flag" title="Flagged">⚑</span>{/if}
            {#if r.file?.disputed}<span class="warn" title="A second model disputes the description">⚠</span>{/if}
            {#if r.file?.changed}<span class="changed" title="Changed since your last look">↻</span>{/if}
            {#if r.file?.preparing}<span class="dot" title="Being described"></span>{/if}
          </button>
          {#if ui.buttons && r.kind === "File"}
            <span class="quick">
              <Act id="decide.accept" icon="✓" kind="icon" before={select(r.id)} />
              <Act id="decide.flag" icon="⚑" kind="icon" before={select(r.id)} />
            </span>
          {/if}
        </div>
      {/each}
      {#if !queue.rows.length}
        <p class="muted empty">Nothing to review.</p>
      {/if}
    </div>
    {#if auto > 0 || ui.buttons}
      <div class="foot">
        {#if auto > 0 && ui.buttons}
          <Act id="toggle.auto_accepted" label="{auto} accepted for you" kind="icon" />
        {:else if auto > 0}
          <span class="muted">{auto} accepted for you</span>
        {/if}
        <span class="spacer"></span>
        <Act id="find.unit" icon="⌕" kind="icon" />
      </div>
    {/if}
  </div>
  <div class="preview scroll" data-scroll="preview" bind:this={pane}>
    {#if preview}
      <div class="filehead">
        <h2 class="mono" title={preview.path}>
          <span class="muted">{preview.path.slice(0, slash)}</span>{preview.path.slice(slash)}
        </h2>
        {#if ui.buttons}
          <div class="acts" role="toolbar" aria-label="The file">
            <Act id="decide.accept" label="Accept" icon="✓" kind="primary" />
            <Act id="decide.flag" label="Flag" icon="⚑" />
            <Act id="history.undo" label="Undo" icon="↶" kind="icon" />
          </div>
        {/if}
      </div>
      {#each preview.cards as card (card.id)}
        <UnitChange
          {card}
          selected={card.id === preview.selected}
          marked={card.id === preview.selected && preview.cards.length > 1}
        />
      {/each}
    {/if}
  </div>
</div>

<style>
  .queue {
    flex: 1;
    display: flex;
    min-width: 0;
  }
  .side {
    flex: 0 0 clamp(230px, 28%, 380px);
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--border);
    background: var(--panel);
  }
  .tree {
    flex: 1;
    min-height: 0;
    padding: 8px 0;
  }
  .preview {
    flex: 1;
    min-width: 0;
    padding: 0 22px 40px 10px;
  }
  .rowwrap {
    position: relative;
  }
  .row {
    display: flex;
    gap: 5px;
    align-items: center;
    width: 100%;
    text-align: left;
    background: none;
    border: 0;
    border-left: 3px solid transparent;
    padding: 2px 10px 2px 3px;
    cursor: default;
    white-space: nowrap;
  }
  .rowwrap:hover .row {
    background: var(--sunken);
  }
  .row.selected,
  .rowwrap:hover .row.selected {
    background: var(--select);
    border-left-color: var(--accent);
  }
  .row.visual,
  .rowwrap:hover .row.visual {
    background: var(--visual);
    border-left-color: var(--accent);
  }
  .gutter {
    flex: none;
    width: 14px;
    display: inline-flex;
  }
  .indent {
    flex: none;
  }
  /* A directory's triangle; a file keeps its place, so names line up. */
  .tri {
    flex: none;
    width: 10px;
    display: inline-flex;
    justify-content: center;
  }
  .tri i {
    border: 4px solid transparent;
    border-left: 5px solid var(--muted);
    border-right: 0;
  }
  .tri.open i {
    transform: rotate(90deg);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .Dir .name {
    color: var(--muted);
  }
  .done .name,
  .Skipped .name {
    color: var(--faint);
  }
  .Skipped .name {
    font-style: italic;
  }
  .flag {
    color: var(--flag);
  }
  .warn {
    color: var(--look);
  }
  .changed {
    color: var(--accent);
  }
  .dot {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    animation: pulse 1s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.2;
    }
  }
  .quick {
    position: absolute;
    right: 4px;
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
  .rowwrap.selected .quick {
    background: linear-gradient(to right, transparent, var(--select) 16px);
  }
  .foot {
    display: flex;
    align-items: center;
    min-height: 30px;
    padding: 2px 6px;
    border-top: 1px solid var(--border);
    font-size: 12px;
  }
  .foot .muted {
    padding: 0 6px;
  }
  .spacer {
    flex: 1;
  }
  .empty {
    padding: 0 14px;
  }
  .filehead {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 44px;
    margin: 0 -22px 0 -10px;
    padding: 6px 22px 6px 22px;
    background: var(--bg);
    border-bottom: 1px solid var(--border);
  }
  .filehead h2 {
    flex: 1;
    min-width: 0;
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .filehead h2 .muted {
    font-weight: 400;
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .surface {
    margin: 0 10px 8px;
    padding: 4px 10px 6px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
  }
  .surface h2 {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 12px;
    font-weight: 700;
    margin: 0;
  }
  .srow {
    display: flex;
    gap: 10px;
    align-items: baseline;
    padding: 1px 0;
  }
  .slabel {
    flex: 0 0 84px;
    font-size: 12px;
  }
  .sitems {
    min-width: 0;
  }
  .sitem {
    border: 0;
    background: none;
    padding: 0;
    font: inherit;
    color: inherit;
    text-align: left;
    cursor: pointer;
  }
  .sitem:hover {
    text-decoration: underline;
  }
  .checked {
    margin: 4px 0 0;
    font-size: 12px;
  }
</style>
