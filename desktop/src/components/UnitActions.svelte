<script lang="ts">
  // What the unit on show can be done with, on its card under its name:
  // the decision first, then a closer look, undo at the end. `on` names
  // the screen: the queue's preview; the unit view, whose closer looks are
  // the tiles beside it; the flagged list, flagged already.
  import { ui } from "../lib/ui.svelte";
  import Act from "./Act.svelte";

  let { on }: { on: "queue" | "unit" | "flagged" } = $props();
</script>

{#if ui.buttons}
<div class="unit-actions" role="toolbar" aria-label="The unit">
  <div class="group">
    <Act id="decide.accept" label="Accept" icon="✓" kind="primary" />
    {#if on !== "flagged"}<Act id="decide.flag" label="Flag" icon="⚑" />{/if}
    <Act id="note.edit" label="Note" icon="✎" />
    {#if on !== "flagged"}<Act id="decide.accept_file" label="Accept the file" />{/if}
  </div>
  {#if on !== "unit"}
    <div class="group">
      <Act id="view.open" label="Open" />
      <Act id="ask.open" label="Ask" />
      <Act id="tool.advice" label="Advice" />
      <Act id="tool.tests" label="Tests" />
    </div>
  {/if}
  <span class="spacer"></span>
  <Act id="history.undo" label="Undo" icon="↶" kind="icon" />
</div>
{/if}

<style>
  .unit-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 14px;
    padding: 8px 0 2px;
  }
  .group {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .group + .group {
    padding-left: 14px;
    border-left: 1px solid var(--border);
  }
  .spacer {
    flex: 1;
  }
</style>
