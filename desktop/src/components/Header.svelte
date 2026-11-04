<script lang="ts">
  // What is reviewed and what the change is called; what teer is doing.
  import type { Header, Screen } from "../lib/types";
  let { header, screen }: { header: Header; screen: Screen } = $props();
</script>

<header>
  <div class="line">
    <strong>{header.subject}</strong>
    {#if header.title}<span class="title">· {header.title}</span>{/if}
    <span class="spacer"></span>
    {#if header.no_agent}<span class="muted">no agent</span>{/if}
    {#if header.preparing}
      <span class="muted">
        <span class="dot"></span> preparing {header.preparing.done}/{header.preparing.total}
      </span>
    {/if}
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
    align-items: baseline;
  }
  .title {
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
