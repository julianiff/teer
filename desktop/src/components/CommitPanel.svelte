<script lang="ts">
  // What git and its hooks print while the commit runs.
  let {
    lines,
    done,
    ok,
    onclose,
  }: { lines: string[]; done: boolean; ok: boolean; onclose: () => void } = $props();
  let box: HTMLElement | undefined = $state();
  $effect(() => {
    void lines.length;
    box?.scrollTo({ top: box.scrollHeight });
  });
</script>

<div class="panel" class:failed={done && !ok}>
  <div class="head">
    <strong>{done ? (ok ? "Committed" : "The commit failed") : "Committing…"}</strong>
    <span class="spacer"></span>
    {#if done}<button class="btn" onclick={onclose}>Close</button>{/if}
  </div>
  <pre bind:this={box}>{lines.join("\n") || (done ? "" : "git commit …")}</pre>
</div>

<style>
  .panel {
    position: fixed;
    right: 16px;
    bottom: 48px;
    width: min(620px, calc(100% - 32px));
    z-index: 22;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.25);
    overflow: hidden;
  }
  .panel.failed {
    border-color: var(--bad);
  }
  .head {
    display: flex;
    align-items: center;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
  }
  .spacer {
    flex: 1;
  }
  pre {
    margin: 0;
    padding: 8px 12px;
    max-height: 260px;
    overflow: auto;
    font: 12px/1.5 var(--mono);
    white-space: pre-wrap;
  }
</style>
