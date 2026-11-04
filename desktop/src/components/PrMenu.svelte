<script lang="ts">
  // The top left of a review: what it is of, and a menu of what else to
  // review here, as `teer pr` lists it: the repository's own changes, the
  // pull requests waiting for your review, assigned to you, and yours. One
  // picked opens in a window of its own (or the one showing it already).
  import { api } from "../lib/api";
  import type { ListedPr, Listing } from "../lib/types";
  import { ui } from "../lib/ui.svelte";
  import { ago } from "../lib/when";

  let { subject, home, pr }: { subject: string; home: string; pr: number | null } = $props();

  let listing = $state<Listing | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);
  let filter = $state("");
  let sel = $state(0);
  let field: HTMLInputElement | undefined = $state();
  // The listing is the repository's: another repository's is fetched anew.
  let listed = "";

  interface Item {
    section: string;
    pr: ListedPr | null;
  }

  const items = $derived.by((): Item[] => {
    const q = filter.trim().toLowerCase();
    const matches = (p: ListedPr) =>
      !q ||
      String(p.number).includes(q) ||
      p.title.toLowerCase().includes(q) ||
      p.author.toLowerCase().includes(q) ||
      p.head.toLowerCase().includes(q);
    const out: Item[] = [];
    if (pr !== null && (!q || "your changes".includes(q))) out.push({ section: "", pr: null });
    if (!listing) return out;
    for (const [section, prs] of [
      ["Waiting for your review", listing.needs_review],
      ["Assigned to you", listing.assigned],
      ["Yours", listing.mine],
    ] as const) {
      for (const p of prs.filter(matches)) out.push({ section, pr: p });
    }
    return out;
  });

  async function load() {
    loading = true;
    error = null;
    try {
      listing = await api.listPrs(home);
      listed = home;
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (ui.prMenu) {
      filter = "";
      sel = 0;
      queueMicrotask(() => field?.focus());
      if (listed !== home || !listing) void load();
    }
  });

  function close() {
    ui.prMenu = false;
  }

  function open(item: Item | undefined) {
    if (!item) return;
    if (item.pr?.number === pr) return close();
    void api.openElsewhere(home, item.pr ? item.pr.url : undefined);
    close();
  }

  function key(e: KeyboardEvent) {
    if (e.key === "Escape") close();
    else if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) sel = Math.min(sel + 1, items.length - 1);
    else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) sel = Math.max(sel - 1, 0);
    else if (e.key === "Enter") open(items[sel]);
    else return;
    e.preventDefault();
  }

  const decided = (p: ListedPr) =>
    p.review_decision === "APPROVED"
      ? "approved"
      : p.review_decision === "CHANGES_REQUESTED"
        ? "changes requested"
        : "";
</script>

<div class="wrap" data-own-keys>
  <button class="subject" class:open={ui.prMenu} onclick={() => (ui.prMenu = !ui.prMenu)} title="Pull requests (⇧⌘O)">
    <strong>{subject}</strong>
    <span class="caret">▾</span>
  </button>
  {#if ui.prMenu}
    <div class="scrim" role="presentation" onclick={close}></div>
    <div class="menu" role="dialog" aria-label="Pull requests">
      <div class="top">
        <input
          type="text"
          bind:this={field}
          bind:value={filter}
          placeholder={listing ? `${listing.repo.owner}/${listing.repo.repo}: find a pull request` : "Find a pull request"}
          spellcheck="false"
          onkeydown={key}
          oninput={() => (sel = 0)}
        />
        <button class="refresh" disabled={loading} onclick={load} title="Ask GitHub again">{loading ? "…" : "⟳"}</button>
      </div>
      <ul>
        {#each items as item, i (item.pr?.number ?? "changes")}
          {#if item.section && item.section !== items[i - 1]?.section}
            <li class="section">{item.section}</li>
          {/if}
          <li>
            <button class:selected={i === sel} onclick={() => open(item)} onmouseenter={() => (sel = i)}>
              {#if item.pr}
                <span class="num">#{item.pr.number}</span>
                <span class="title">{item.pr.title}</span>
                {#if item.pr.number === pr}<span class="here">open here</span>{/if}
                {#if item.pr.draft}<span class="tag">draft</span>{/if}
                {#if decided(item.pr)}<span class="tag">{decided(item.pr)}</span>{/if}
                <span class="meta">@{item.pr.author} · {ago(item.pr.updated_at)}</span>
              {:else}
                <span class="num">·</span>
                <span class="title">Your changes in this repository</span>
              {/if}
            </button>
          </li>
        {/each}
      </ul>
      {#if loading && !listing}
        <p class="note">Asking GitHub…</p>
      {:else if error}
        <p class="note bad">{error}</p>
      {:else if listing && !listing.needs_review.length && !listing.assigned.length && !listing.mine.length}
        <p class="note">No open pull request waits for your review, is assigned to you, or is yours.</p>
      {:else if listing && items.length === 0}
        <p class="note">Nothing matches.</p>
      {/if}
      {#if listing?.truncated}<p class="note">GitHub gave the first {listing.needs_review.length + listing.assigned.length + listing.mine.length}; there may be more.</p>{/if}
    </div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
    min-width: 0;
  }
  .subject {
    display: flex;
    gap: 6px;
    align-items: baseline;
    border: 1px solid transparent;
    background: none;
    border-radius: 6px;
    padding: 2px 8px;
    margin-left: -8px;
    cursor: pointer;
    white-space: nowrap;
  }
  .subject:hover,
  .subject.open {
    border-color: var(--border);
    background: var(--sunken);
  }
  .caret {
    color: var(--muted);
    font-size: 11px;
  }
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 15;
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    left: -8px;
    z-index: 16;
    width: min(620px, calc(100vw - 32px));
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 14px 44px rgba(0, 0, 0, 0.28);
    overflow: hidden;
  }
  .top {
    display: flex;
    gap: 6px;
    padding: 8px;
    border-bottom: 1px solid var(--border);
  }
  .top input {
    flex: 1;
  }
  .refresh {
    border: 1px solid var(--border);
    background: var(--panel);
    border-radius: 6px;
    width: 32px;
    cursor: pointer;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 4px;
    overflow: auto;
  }
  .section {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
    padding: 8px 8px 2px;
  }
  li button {
    display: flex;
    gap: 8px;
    align-items: baseline;
    width: 100%;
    text-align: left;
    border: 0;
    background: none;
    padding: 5px 8px;
    border-radius: 6px;
    cursor: pointer;
  }
  li button.selected {
    background: var(--select);
  }
  .num {
    font-family: var(--mono);
    color: var(--muted);
    min-width: 44px;
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
  }
  .tag,
  .here {
    font-size: 11px;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 4px;
    color: var(--muted);
    white-space: nowrap;
  }
  .here {
    color: var(--accent);
    border-color: var(--accent);
  }
  .note {
    margin: 0;
    padding: 8px 12px 10px;
    color: var(--muted);
    font-size: 12px;
  }
  .bad {
    color: var(--bad);
    white-space: pre-wrap;
  }
</style>
