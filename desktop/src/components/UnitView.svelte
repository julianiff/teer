<script lang="ts">
  // The unit view: the unit's text on the left, the tiles beside it, the
  // keys under them. A click focuses a tile; zoom fills the view with it.
  import { api } from "../lib/api";
  import type { TileId, UnitScreen } from "../lib/types";
  import Card from "./Card.svelte";
  import TileView from "./TileView.svelte";

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
</script>

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
      <Card card={unit.card} second={unit.second} />
    </div>
  {/if}
  {#if unit.tiles.length || !zoomed}
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
            <h3>{tile.title}</h3>
            <div class="body scroll" data-scroll="tile-{tile.id}">
              <TileView {tile} typing={unit.typing} />
            </div>
          </section>
        {/if}
      {/each}
      {#if !zoomed}
        <section class="keys">
          {#each unit.keys as g (g.name)}
            <div class="group">
              <span class="muted gname">{g.name}</span>
              {#each g.items as item (item.action)}
                <button class="item" class:off={item.off} onclick={() => api.action(item.action)}>
                  <span class="key">{item.keys}</span> {item.what}
                </button>
              {/each}
            </div>
          {/each}
        </section>
      {/if}
    </aside>
  {/if}
</div>

<style>
  .unit {
    flex: 1;
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
  .tile h3 {
    margin: 0;
    padding: 6px 12px;
    font-size: 12px;
    border-bottom: 1px solid var(--border);
  }
  .tile .body {
    flex: 1;
    padding: 8px 12px 16px;
  }
  .keys {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px 4px 2px;
  }
  .group {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    align-items: baseline;
  }
  .gname {
    width: 76px;
    font-size: 11px;
  }
  .item {
    border: 0;
    background: none;
    padding: 0;
    cursor: pointer;
    font-size: 12px;
  }
  .item.off {
    opacity: 0.4;
  }
</style>
