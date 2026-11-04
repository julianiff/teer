<script lang="ts">
  // A button where its action acts: a word or an icon, its key beside it
  // and in the tooltip. A click presses the key (the session runs it as
  // typed), after `before` (selecting the row it is on); `run` stands in
  // for a press where a click means more than the key (back, whatever
  // tile has the focus). Dimmed while it needs an agent; a toggle shows
  // its state. Not drawn for an action teer doesn't offer, nor while the
  // buttons are hidden, when a link is its words alone.
  import { api } from "../lib/api";
  import { tip, useActions } from "../lib/actions";
  import { ui } from "../lib/ui.svelte";

  let {
    id,
    label = "",
    icon = "",
    kind = "plain",
    showKey = true,
    current = false,
    title = "",
    before,
    run,
  }: {
    id: string;
    label?: string;
    icon?: string;
    kind?: "plain" | "primary" | "icon" | "link";
    showKey?: boolean;
    /** Shown as on: the tile it opens is open. */
    current?: boolean;
    /** What the tooltip says it does, for a click that isn't the key's. */
    title?: string;
    before?: () => Promise<unknown>;
    run?: () => unknown;
  } = $props();

  const actions = useActions();
  const a = $derived(actions()[id]);

  async function press(e: MouseEvent) {
    e.stopPropagation();
    if (run) return void run();
    if (before) await before();
    void api.action(id);
  }
</script>

{#if a && !ui.buttons}
  {#if kind === "link" && label}<span>{label}</span>{/if}
{:else if a}
  <button
    class="act {kind}"
    class:on={a.on || current}
    class:off={a.off}
    aria-pressed={a.on ?? undefined}
    aria-label={label || title || a.what}
    title={tip(a, title || a.what)}
    onclick={press}
    ondblclick={(e) => e.stopPropagation()}
  >
    {#if icon}<span class="icon" aria-hidden="true">{icon}</span>{/if}
    {#if label}<span>{label}</span>{/if}
    {#if showKey && a.keys}<kbd>{a.keys}</kbd>{/if}
  </button>
{/if}

<style>
  .act {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    border: 1px solid var(--border);
    background: var(--panel);
    border-radius: 6px;
    padding: 2px 8px;
    font-size: 12px;
    line-height: 1.6;
    cursor: pointer;
    white-space: nowrap;
  }
  .act:hover {
    border-color: var(--accent);
  }
  .act:active {
    background: var(--select);
  }
  kbd {
    font-size: 10px;
    color: var(--muted);
    border-bottom-width: 1px;
    padding: 0 4px;
    background: transparent;
  }
  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
  .primary:active {
    background: var(--accent);
    filter: brightness(0.92);
  }
  .primary kbd {
    color: rgba(255, 255, 255, 0.85);
    border-color: rgba(255, 255, 255, 0.45);
  }
  .icon {
    font-size: 12px;
  }
  .act.icon {
    border-color: transparent;
    background: none;
    color: var(--muted);
    padding: 2px 6px;
  }
  .act.icon:hover {
    color: var(--text);
    background: var(--sunken);
    border-color: var(--border);
  }
  .link {
    border: 0;
    background: none;
    padding: 0 2px;
    font-size: inherit;
    color: inherit;
    text-decoration: underline dotted var(--faint);
    text-underline-offset: 3px;
  }
  .link:hover {
    color: var(--accent);
    text-decoration-color: var(--accent);
  }
  .link:active {
    background: none;
  }
  .act.on {
    background: var(--accent-bg);
    border-color: var(--accent);
    color: var(--accent);
  }
  .off {
    opacity: 0.45;
  }
</style>
