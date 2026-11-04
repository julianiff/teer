<script lang="ts">
  // At the top of the start screen: the repositories saved there, each
  // with the pull requests waiting for your review or assigned to you, a
  // click away from their review; a click on its name reviews its own
  // changes. GitHub is asked once per repository, and again for all of
  // them on "Refresh all", whose icon turns while it asks.
  // "+ Add repository" opens the add sheet; with none saved, it is all
  // there is.
  import { untrack } from "svelte";
  import { api } from "../lib/api";
  import type { ListedPr, Listing, Saved } from "../lib/types";
  import { ago } from "../lib/when";

  let {
    saved,
    busy = false,
    onopen,
    onforget,
    onadd,
  }: {
    saved: Saved[];
    busy?: boolean;
    onopen: (path: string, pr?: string) => void;
    onforget: (path: string) => void;
    onadd: () => void;
  } = $props();

  interface Inbox {
    listing: Listing | null;
    error: string | null;
    loading: boolean;
  }
  let inbox = $state<Record<string, Inbox>>({});

  async function load(path: string) {
    inbox[path] = { listing: inbox[path]?.listing ?? null, error: null, loading: true };
    try {
      inbox[path] = { listing: await api.listPrs(path), error: null, loading: false };
    } catch (e) {
      inbox[path] = { listing: null, error: String(e), loading: false };
    }
  }

  // A repository saved since is asked about once.
  $effect(() => {
    for (const s of saved) if (!untrack(() => s.path in inbox)) void load(s.path);
  });

  /** What waits for you: for your review, then assigned to you. */
  function waiting(l: Listing): ListedPr[] {
    const seen = new Set(l.needs_review.map((p) => p.number));
    return [...l.needs_review, ...l.assigned.filter((p) => !seen.has(p.number))];
  }

  const firstLine = (s: string) => s.split("\n")[0];
  const loading = $derived(saved.some((s) => inbox[s.path]?.loading));
</script>

{#snippet refreshIcon(spin: boolean)}
  <svg class:spin viewBox="0 0 24 24" width="14" height="14" aria-hidden="true">
    <path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" />
    <path d="M21 3v5h-5" />
  </svg>
{/snippet}

<section class="card saved" aria-label="Your repositories">
  <div class="top">
    <h3 class="section">Your repositories</h3>
    <span class="spacer"></span>
    {#if saved.length}
      <button
        class="icon all"
        title="Fetch the pull requests of every repository again"
        disabled={loading}
        onclick={() => saved.forEach((s) => void load(s.path))}
      >
        {@render refreshIcon(loading)} Refresh all
      </button>
    {/if}
    <button class="btn add" onclick={onadd}><span aria-hidden="true">+</span> Add repository</button>
  </div>
  {#if !saved.length}
    <div class="empty">
      <p>No repositories yet.</p>
      <p class="muted">Add one to see the pull requests waiting for your review in it, or drop its folder here.</p>
    </div>
  {/if}
  {#each saved as s (s.path)}
    {@const box = inbox[s.path]}
    {@const prs = box?.listing ? waiting(box.listing) : []}
    <div class="repo">
      <div class="rhead">
        <button class="name" disabled={busy} title="Review its changes: uncommitted, or the branch since its base" onclick={() => onopen(s.path)}>
          {s.name}
        </button>
        {#if box?.listing}
          <span class="count" class:none={!prs.length} title="Waiting for your review or assigned to you">{prs.length}</span>
        {/if}
        <span class="path muted mono" title={s.path}>{s.path}</span>
        <button class="icon forget" title="Forget it here; its reviews stay" aria-label="Forget {s.name}" onclick={() => onforget(s.path)}>✕</button>
      </div>
      {#if box?.loading && !box.listing}
        <p class="small muted">Asking GitHub…</p>
      {:else if box?.error}
        <p class="small muted" title={box.error}>{firstLine(box.error)}</p>
      {:else if box?.listing && !prs.length}
        <p class="small muted">Nothing waits for your review.</p>
      {:else if prs.length}
        <ul class="prs">
          {#each prs as p (p.number)}
            <li>
              <button disabled={busy} onclick={() => onopen(s.path, p.url)}>
                <span class="num">#{p.number}</span>
                <span class="title">{p.title}</span>
                {#if p.draft}<span class="tag">draft</span>{/if}
                <span class="small muted meta">@{p.author}{p.updated_at ? ` · ${ago(p.updated_at)}` : ""}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/each}
</section>

<style>
  .card {
    width: min(720px, 100%);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 12px 18px 14px;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 2px;
  }
  .spacer {
    flex: 1;
  }
  .add {
    padding: 3px 10px;
    font-size: 12px;
  }
  .add span {
    font-weight: 700;
    margin-right: 2px;
  }
  .empty {
    border: 1px dashed var(--border);
    border-radius: 8px;
    padding: 14px 16px;
    margin: 8px 0 2px;
    text-align: center;
  }
  .empty p {
    margin: 2px 0;
    font-size: 13px;
  }
  .top h3 {
    margin: 4px 0;
  }
  .icon {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    border: 1px solid transparent;
    background: none;
    border-radius: 6px;
    color: var(--muted);
    cursor: pointer;
    padding: 2px 6px;
    font-size: 12px;
    line-height: 1.4;
  }
  .icon:hover:not(:disabled) {
    border-color: var(--border);
    background: var(--sunken);
    color: var(--text);
  }
  .icon:disabled {
    cursor: default;
  }
  svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  svg.spin {
    animation: spin 0.9s linear infinite;
    color: var(--accent);
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .repo {
    padding: 8px 0 4px;
  }
  .repo + .repo {
    border-top: 1px solid var(--border);
  }
  .rhead {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .name {
    border: 0;
    background: none;
    padding: 0;
    font-weight: 700;
    white-space: nowrap;
    cursor: pointer;
  }
  .name:hover:not(:disabled) {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .count {
    font-size: 11px;
    font-weight: 700;
    color: #fff;
    background: var(--accent);
    border-radius: 9px;
    padding: 0 6px;
  }
  .count.none {
    color: var(--muted);
    background: var(--sunken);
  }
  .path {
    flex: 1;
    min-width: 0;
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .small {
    font-size: 12px;
    margin: 4px 0 0;
  }
  ul {
    list-style: none;
    margin: 4px 0 0;
    padding: 0;
  }
  li button {
    width: 100%;
    display: flex;
    gap: 10px;
    align-items: baseline;
    text-align: left;
    background: none;
    border: 0;
    padding: 5px 8px;
    border-radius: 6px;
    cursor: pointer;
  }
  li button:hover {
    background: var(--select);
  }
  .num {
    font-family: var(--mono);
    color: var(--muted);
    min-width: 48px;
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    margin: 0;
    white-space: nowrap;
  }
  .tag {
    font-size: 11px;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 4px;
    color: var(--muted);
  }
</style>
