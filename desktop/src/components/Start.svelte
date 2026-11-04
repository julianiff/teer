<script lang="ts">
  // The start screen: a repository to review (picked, recent, or dropped),
  // its changes or a pull request of it.
  import { open as pickFolder } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import type { ListedPr, Listing, Recent } from "../lib/types";

  // What the last review asked for came to instead: nothing to review in
  // that repository, or why it couldn't be opened.
  let {
    mode = $bindable(null),
    said = null,
  }: {
    mode: "folder" | "pr" | null;
    said?: { repo: string; text: string; error: boolean } | null;
  } = $props();

  let recent = $state<Recent[]>([]);
  let repo = $state("");
  let base = $state("");
  let pr = $state("");
  let noAgent = $state(false);
  let error = $state<string | null>(null);
  let note = $state<string | null>(null);
  let busy = $state(false);
  let listing = $state<Listing | null>(null);
  let listing_error = $state<string | null>(null);
  let prField: HTMLInputElement | undefined = $state();

  onMount(async () => {
    if (said) {
      // Back from "Opening…": the same repository, and what came of it.
      repo = said.repo;
      if (said.error) error = said.text;
      else {
        note = said.text;
        // Nothing to review here: perhaps a pull request of it.
        void loadPrs();
      }
    }
    recent = await api.recent();
    if (!repo && recent[0]) repo = recent[0].path;
  });

  $effect(() => {
    if (mode === "folder") {
      mode = null;
      void choose();
    } else if (mode === "pr") {
      mode = null;
      prField?.focus();
    }
  });

  async function choose() {
    const dir = await pickFolder({ directory: true, title: "A repository to review" });
    if (typeof dir === "string") {
      repo = dir;
      listing = null;
      await start();
    }
  }

  async function start(with_pr?: string) {
    if (!repo || busy) return;
    busy = true;
    error = null;
    note = null;
    try {
      const said = await api.openReview(repo, {
        base: base || undefined,
        pr: with_pr ?? (pr || undefined),
        noAgent,
      });
      if (said) note = said;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function loadPrs() {
    if (!repo) return;
    listing = null;
    listing_error = null;
    try {
      listing = await api.listPrs(repo);
    } catch (e) {
      listing_error = String(e);
    }
  }

  const sections = (l: Listing): [string, ListedPr[]][] => [
    ["Waiting for your review", l.needs_review],
    ["Assigned to you", l.assigned],
    ["Yours", l.mine],
  ];

  function dropped(e: DragEvent) {
    e.preventDefault();
    const path = e.dataTransfer?.getData("text/uri-list") || e.dataTransfer?.getData("text/plain");
    if (path) {
      repo = decodeURIComponent(path.replace(/^file:\/\//, "").trim());
      void start();
    }
  }
</script>

<main
  class="start"
  role="region"
  aria-label="Start"
  ondragover={(e) => e.preventDefault()}
  ondrop={dropped}
>
  <section class="hero">
    <h1>teer</h1>
    <p class="muted">The essence of every revision: review changes by what they mean.</p>
  </section>

  <section class="card">
    <div class="repo">
      <input type="text" bind:value={repo} placeholder="/path/to/a/repository" spellcheck="false" />
      <button class="btn" onclick={choose}>Choose…</button>
    </div>
    <div class="options">
      <label>
        <span>Base</span>
        <input type="text" bind:value={base} placeholder="found by itself" spellcheck="false" />
      </label>
      <label>
        <span>Pull request</span>
        <input
          type="text"
          bind:this={prField}
          bind:value={pr}
          placeholder="482 or a GitHub URL"
          spellcheck="false"
          onkeydown={(e) => e.key === "Enter" && start()}
        />
      </label>
      <label class="check">
        <input type="checkbox" bind:checked={noAgent} />
        <span>Without an agent</span>
      </label>
    </div>
    <div class="go">
      <button class="btn primary" disabled={!repo || busy} onclick={() => start()}>
        {busy ? "Opening…" : pr ? `Review PR ${pr}` : "Review the changes"}
      </button>
      <button class="btn" disabled={!repo} onclick={loadPrs}>Pull requests…</button>
    </div>
    {#if note}<p class="note">{note}</p>{/if}
    {#if error}<pre class="error">{error}</pre>{/if}
  </section>

  {#if listing || listing_error}
    <section class="card">
      {#if listing_error}
        <pre class="error">{listing_error}</pre>
      {:else if listing}
        <h3 class="section">{listing.repo.owner}/{listing.repo.repo}</h3>
        {#each sections(listing) as [title, prs] (title)}
          {#if prs.length}
            <h4>{title}</h4>
            <ul class="prs">
              {#each prs as p (p.number)}
                <li>
                  <button onclick={() => start(String(p.url))}>
                    <span class="num">#{p.number}</span>
                    <span class="title">{p.title}</span>
                    <span class="muted">@{p.author}{p.draft ? " · draft" : ""}</span>
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
        {/each}
        {#if !listing.needs_review.length && !listing.assigned.length && !listing.mine.length}
          <p class="muted">No open pull requests are waiting, assigned to you, or yours.</p>
        {/if}
      {/if}
    </section>
  {/if}

  {#if recent.length}
    <section class="card">
      <h3 class="section">Recent</h3>
      <ul class="recent">
        {#each recent as r (r.path)}
          <li>
            <button
              onclick={() => {
                repo = r.path;
                void start();
              }}
            >
              <span class="name">{r.name}</span>
              <span class="path muted mono">{r.path}</span>
            </button>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
  <p class="faint drop">Or drop a folder here, or on Teer in the Dock.</p>
</main>

<style>
  .start {
    height: 100%;
    overflow: auto;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 48px 16px;
    gap: 16px;
  }
  .hero {
    text-align: center;
  }
  .hero h1 {
    font: 600 34px/1 var(--mono);
    margin: 0 0 6px;
    letter-spacing: -0.02em;
  }
  .card {
    width: min(720px, 100%);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 16px 18px;
  }
  .repo {
    display: flex;
    gap: 8px;
  }
  .repo input {
    flex: 1;
    font-family: var(--mono);
  }
  .options {
    display: flex;
    gap: 12px;
    margin-top: 12px;
    flex-wrap: wrap;
    align-items: end;
  }
  .options label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 12px;
    color: var(--muted);
  }
  .options label.check {
    flex-direction: row;
    align-items: center;
    gap: 6px;
    padding-bottom: 7px;
  }
  .go {
    display: flex;
    gap: 8px;
    margin-top: 14px;
  }
  .note {
    margin: 12px 0 0;
  }
  .error {
    margin: 12px 0 0;
    color: var(--bad);
    white-space: pre-wrap;
    font: 12px/1.5 var(--mono);
  }
  ul {
    list-style: none;
    margin: 0;
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
    padding: 6px 8px;
    border-radius: 6px;
    cursor: pointer;
  }
  li button:hover {
    background: var(--select);
  }
  .recent .name {
    font-weight: 600;
    min-width: 120px;
  }
  .recent .path {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .prs .num {
    font-family: var(--mono);
    color: var(--muted);
    min-width: 48px;
  }
  .prs .title {
    flex: 1;
  }
  h4 {
    margin: 12px 0 4px;
    font-size: 12px;
    color: var(--muted);
  }
  .drop {
    font-size: 12px;
  }
</style>
