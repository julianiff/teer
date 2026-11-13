<script lang="ts">
  // The change the agent proposes for the unit, after its code change:
  // what it answers and who wrote it, the summary, the second model's
  // word, then each hunk in a box like the code change, its state on its
  // head. "Put in" picks the hunk, then presses `fix.apply` (teer asks
  // first); "Put all in" presses `fix.apply_all`. On a pull request they
  // become suggestions for the review. While a change is on its way, what
  // it is doing.
  import { api } from "../lib/api";
  import type { HunkState, ProposalView } from "../lib/types";
  import Act from "./Act.svelte";
  import Code from "./Code.svelte";

  let { proposal }: { proposal: ProposalView } = $props();

  const verb = $derived(proposal.suggest ? "Suggest" : "Put in");
  const open = $derived(proposal.hunks.filter((h) => h.state === "proposed").length);
  const STATE: Record<HunkState, string> = {
    proposed: "proposed",
    applied: "put in",
    suggested: "suggested",
  };
  const GLYPH = { high: "▲", medium: "◆", low: "●" } as const;
  const pick = (i: number) => api.click({ kind: "Fix", i });
</script>

<section class="proposal" aria-label="Proposed change">
  <h3 class="section">
    Proposed change
    {#if proposal.answers}
      <span class="for">
        for
        {#if proposal.severity}
          <span class="risk-{proposal.severity}">{GLYPH[proposal.severity]} {proposal.answers} ({proposal.severity})</span>
        {:else}
          <span>{proposal.answers}</span>
        {/if}
      </span>
    {/if}
    <span class="muted by">{proposal.label}</span>
  </h3>
  {#if proposal.waiting}
    <p class="waiting"><span class="spin"></span> {proposal.waiting}</p>
  {/if}
  {#if proposal.summary}<p class="summary">{proposal.summary}</p>{/if}
  {#if proposal.second}
    {#if proposal.second.agree}
      <p class="ok small">✓ check agrees</p>
    {:else}
      <p class="risk-medium small">⚠ disputed: {proposal.second.why}</p>
    {/if}
  {/if}
  {#if proposal.dropped}
    <p class="muted small">{proposal.dropped} hunk{proposal.dropped === 1 ? "" : "s"} dropped (teer.log says why)</p>
  {/if}
  {#if open > 1}
    <div class="all">
      <Act id="fix.apply_all" label={proposal.suggest ? "Suggest all" : "Put all in"} kind="primary" />
    </div>
  {/if}

  {#each proposal.hunks as h, i (i)}
    <Code code={h.code} title={h.place} current={proposal.current === i}>
      {#snippet tools()}
        <span class="state s-{h.state}">{STATE[h.state]}</span>
        {#if h.state === "proposed"}
          <Act id="fix.apply" label={verb} before={() => pick(i)} />
        {/if}
      {/snippet}
      {#snippet caption()}
        {#if h.why}<span class="muted">{h.why}</span>{/if}
        {#if !h.parses}<span class="risk-medium">⚠ doesn't parse</span>{/if}
      {/snippet}
    </Code>
  {/each}
</section>

<style>
  .proposal {
    max-width: 980px;
  }
  .for {
    text-transform: none;
    letter-spacing: 0;
    font-weight: 600;
    color: var(--text);
    margin-left: 4px;
  }
  .by {
    text-transform: none;
    letter-spacing: 0;
    font-weight: 400;
    margin-left: 8px;
  }
  p {
    margin: 4px 0;
  }
  .summary {
    font-size: 14px;
  }
  .small {
    font-size: 12px;
  }
  .ok {
    color: var(--ok);
  }
  .all {
    margin: 8px 0 0;
  }
  .state {
    font-size: 11px;
  }
  .s-applied {
    color: var(--ok);
  }
  .s-suggested {
    color: var(--accent);
  }
  .waiting {
    color: var(--muted);
  }
  .spin {
    display: inline-block;
    width: 10px;
    height: 10px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
