<script lang="ts">
  // A review: the header, the screen teer is on, the bar, and what is open over them.
  import type { View } from "../lib/types";
  import Header from "./Header.svelte";
  import Bar from "./Bar.svelte";
  import Queue from "./Queue.svelte";
  import UnitView from "./UnitView.svelte";
  import Flagged from "./Flagged.svelte";
  import OverlayView from "./OverlayView.svelte";

  let { view }: { view: View } = $props();
</script>

<div class="review">
  <Header header={view.header} screen={view.screen} />
  <div class="screen">
    {#if view.screen === "Unit" && view.unit}
      <UnitView unit={view.unit} />
    {:else if view.screen === "Flagged" && view.flagged}
      <Flagged flagged={view.flagged} />
    {:else}
      <Queue queue={view.queue} preview={view.preview} />
    {/if}
  </div>
  <Bar bar={view.bar} />
  {#if view.overlay.kind !== "None"}
    <OverlayView overlay={view.overlay} />
  {/if}
</div>

<style>
  .review {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .screen {
    flex: 1;
    min-height: 0;
    display: flex;
  }
</style>
