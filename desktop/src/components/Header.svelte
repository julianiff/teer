<script lang="ts">
  // What is reviewed and what the change is called, its commits beside
  // it; what teer is doing; and what the whole review does: find an
  // action, finish (the way out, stronger once nothing is pending).
  import type { Header, Screen } from "../lib/types";
  import { ui } from "../lib/ui.svelte";
  import Act from "./Act.svelte";
  import PrMenu from "./PrMenu.svelte";
  let { header, screen, done }: { header: Header; screen: Screen; done: boolean } = $props();

  const commits = $derived(
    header.pr !== null
      ? "description and commits"
      : `${header.commits} commit${header.commits === 1 ? "" : "s"}`,
  );
</script>

<header>
  <div class="line">
    <Act id="tool.mode" label={header.mode === "all" ? "all tools" : header.mode} title="The mode: what this review is for" />
    <PrMenu subject={header.subject} home={header.home} pr={header.pr} />
    {#if header.title}<span class="title">· {header.title}</span>{/if}
    {#if header.pr !== null || header.commits > 0}
      <Act id="tool.history" label={commits} kind="link" showKey={false} />
    {/if}
    <span class="spacer"></span>
    {#if header.no_agent}<span class="muted">no agent</span>{/if}
    {#if header.preparing}
      <span class="muted">
        <span class="dot"></span> preparing {header.preparing.done}/{header.preparing.total}
      </span>
    {/if}
    {#if ui.buttons}
      <button class="find" title="Every action, found by what it does (⌘K)" onclick={() => (ui.palette = true)}>
        <span aria-hidden="true">⌕</span> Actions <kbd>⌘K</kbd>
      </button>
    {/if}
    <Act id="finish.open" label="Finish" kind={done ? "primary" : "plain"} />
  </div>
  {#if screen === "Queue" && header.summary}
    <p class="summary muted">{header.summary}</p>
  {/if}
  {#if header.agent_note}
    <p class="agent-note">{header.agent_note}</p>
  {/if}
</header>

<style>
  header {
    padding: 10px 16px 8px;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
  }
  .line {
    display: flex;
    gap: 8px;
    align-items: center;
    white-space: nowrap;
  }
  .find {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    border: 1px solid var(--border);
    background: var(--sunken);
    color: var(--muted);
    border-radius: 6px;
    padding: 2px 8px;
    font-size: 12px;
    line-height: 1.6;
    cursor: pointer;
  }
  .find:hover {
    border-color: var(--accent);
    color: var(--text);
  }
  .find kbd {
    font-size: 10px;
    color: var(--muted);
    border-bottom-width: 1px;
    background: transparent;
  }
  .title {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  .summary {
    margin: 2px 0 0;
  }
  .agent-note {
    margin: 6px 0 0;
    color: var(--look);
  }
  .dot {
    display: inline-block;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
    animation: pulse 1s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.25;
    }
  }
</style>
