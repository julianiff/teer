<script lang="ts">
  // The unit view: the unit's text on the left, the tiles beside it. Over
  // both, the way back, the pending units before and after, and the closer
  // looks, each opening its tile; on the card, the decision; on the code,
  // its hunks and where it leads; on a tile, zoom and close. A click
  // focuses a tile; zoom fills the view with it. Under the card, the change
  // the agent proposes, with buttons that put its hunks in.
  import { api } from "../lib/api";
  import { ui } from "../lib/ui.svelte";
  import type { TileId, UnitScreen } from "../lib/types";
  import Act from "./Act.svelte";
  import Card from "./Card.svelte";
  import Proposal from "./Proposal.svelte";
  import TileView from "./TileView.svelte";
  import UnitActions from "./UnitActions.svelte";

  let { unit }: { unit: UnitScreen } = $props();
  let text: HTMLElement | undefined = $state();

  // Another unit starts at the top.
  let shown = "";
  $effect(() => {
    if (unit.card.id !== shown) {
      shown = unit.card.id;
      text?.scrollTo({ top: 0 });
    }
  });

  const focus = (id: TileId | null) => api.click({ kind: "Tile", id });
  const zoomed = $derived(unit.zoom ? unit.focus : null);
  const open = (id: TileId) => unit.tiles.some((t) => t.id === id);

  // The closer looks, each the action that opens its tile.
  const LOOKS: { action: string; label: string; tile: TileId }[] = [
    { action: "ask.open", label: "Ask", tile: "Ask" },
    { action: "tool.tests", label: "Tests", tile: "Tests" },
    { action: "tool.security", label: "Security", tile: "Security" },
  ];
</script>

<div class="unitscreen">
  {#if ui.buttons}
  <nav class="unitnav" aria-label="The unit view">
    <Act id="view.back" label="Back" icon="←" kind="icon" title="back to the list" run={() => api.click({ kind: "Back" })} />
    <span class="sep"></span>
    <Act id="move.prev_pending" label="Previous" icon="↑" kind="icon" />
    <Act id="move.next_pending" label="Next pending" icon="↓" kind="icon" />
    <span class="spacer"></span>
    <span class="muted small">Look closer</span>
    {#each LOOKS as l (l.action)}
      <Act id={l.action} label={l.label} current={open(l.tile)} />
    {/each}
    <Act id="tool.fix" label="Propose a change" current={!!unit.proposal} />
  </nav>
  {/if}

  <div class="unit" class:zoom={unit.zoom}>
    {#if !zoomed}
      <!-- A click focuses the text, as tab does; the keys are teer's. -->
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <div
        class="text scroll"
        class:focus={unit.focus === null}
        data-scroll="text"
        bind:this={text}
        role="region"
        aria-label="The unit"
        onmousedown={() => unit.focus !== null && focus(null)}
      >
        <Card card={unit.card} second={unit.second} sticky proposed={false}>
          {#snippet actions()}<UnitActions on="unit" />{/snippet}
          {#snippet codeTools()}
            <Act id="move.prev_hunk" icon="↑" kind="icon" showKey={false} />
            <Act id="move.next_hunk" icon="↓" kind="icon" showKey={false} />
            <Act id="goto.definition" label="What it calls" kind="icon" />
            <Act id="goto.callers" label="Callers" kind="icon" />
            <Act id="goto.editor" label="Editor" kind="icon" />
          {/snippet}
        </Card>
        {#if unit.proposal}
          <Proposal proposal={unit.proposal} />
        {/if}
      </div>
    {/if}
    {#if unit.tiles.length}
      <aside class="tiles" class:only={zoomed}>
        {#each unit.tiles as tile (tile.id)}
          {#if !zoomed || zoomed === tile.id}
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <section
              class="tile"
              class:focus={unit.focus === tile.id}
              aria-label={tile.title}
              onmousedown={() => unit.focus !== tile.id && focus(tile.id)}
            >
              <div class="th">
                <h3>{tile.title}</h3>
                <Act
                  id="toggle.zoom"
                  icon={zoomed === tile.id ? "⤡" : "⤢"}
                  kind="icon"
                  showKey={false}
                  title={zoomed === tile.id ? "back beside the text" : "fill the view with it"}
                />
                <Act
                  id="view.back"
                  icon="✕"
                  kind="icon"
                  showKey={false}
                  title="close the tile"
                  run={() => api.click({ kind: "CloseTile", id: tile.id })}
                />
              </div>
              <div class="body scroll" data-scroll="tile-{tile.id}">
                <TileView {tile} typing={unit.typing} />
              </div>
            </section>
          {/if}
        {/each}
      </aside>
    {/if}
  </div>
</div>

<style>
  .unitscreen {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .unitnav {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 6px;
    padding: 5px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
  }
  .sep {
    width: 1px;
    height: 16px;
    background: var(--border);
    margin: 0 4px;
  }
  .spacer {
    flex: 1;
  }
  .small {
    font-size: 11px;
    margin-right: 2px;
  }
  .unit {
    flex: 1;
    min-height: 0;
    display: flex;
    min-width: 0;
  }
  .text {
    flex: 1 1 58%;
    min-width: 0;
    padding: 16px 22px 60px;
    border-top: 2px solid transparent;
  }
  .text.focus {
    border-top-color: var(--accent);
  }
  .tiles {
    flex: 1 1 42%;
    min-width: 320px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 10px;
    border-left: 1px solid var(--border);
    background: var(--sunken);
    resize: horizontal;
    overflow: hidden;
  }
  .tiles.only {
    flex: 1;
    border-left: 0;
  }
  .tile {
    flex: 1 1 0;
    min-height: 120px;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: 9px;
    background: var(--panel);
    overflow: hidden;
  }
  .tile.focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .th {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 2px 6px 2px 12px;
    border-bottom: 1px solid var(--border);
  }
  .th h3 {
    flex: 1;
    margin: 0;
    padding: 4px 0;
    font-size: 12px;
  }
  .tile .body {
    flex: 1;
    padding: 8px 12px 16px;
  }
</style>
