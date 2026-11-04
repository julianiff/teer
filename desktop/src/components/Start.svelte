<script lang="ts">
  // The start screen: a pull request's link to open, your repositories
  // with what waits for your review in each (added through the add sheet,
  // as settings add an account), the recent ones not saved. A folder
  // dropped on the window opens the sheet with it (Tauri takes every drop,
  // so a dragged link never reaches the page: links are pasted). Every
  // repository opened here is saved, the newest first.
  import { open as pickFolder } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import type { Recent, Saved } from "../lib/types";
  import AddRepo from "./AddRepo.svelte";
  import OpenPr from "./OpenPr.svelte";
  import SavedRepos from "./SavedRepos.svelte";

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
  let saved = $state<Saved[]>([]);
  // The recent ones not saved: the saved ones are above.
  const others = $derived(recent.filter((r) => !saved.some((s) => s.path === r.path)));
  let noAgent = $state(false);
  let error = $state<string | null>(null);
  let note = $state<string | null>(null);
  let busy = $state(false);
  /** The add sheet, open with this folder in it; `null` when closed. */
  let adding = $state<string | null>(null);
  let prBox: ReturnType<typeof OpenPr> | undefined = $state();

  onMount(() => {
    if (said) {
      // Back from "Opening…": what came of it; with nothing to review,
      // perhaps a pull request of it, above.
      if (said.error) error = said.text;
      else {
        note = said.text;
        void remember(said.repo);
      }
    }
    void api.savedRepos().then((s) => (saved = s));
    void api.recent().then((r) => (recent = r));
    // Folders dropped on the window: the sheet, with the first of them.
    const off = getCurrentWebviewWindow().onDragDropEvent((e) => {
      if (e.payload.type === "drop" && e.payload.paths[0]) adding = e.payload.paths[0];
    });
    return () => void off.then((f) => f());
  });

  $effect(() => {
    if (mode === "folder") {
      mode = null;
      void choose();
    } else if (mode === "pr") {
      mode = null;
      prBox?.focus();
    }
  });

  /** File › Open Folder…: a folder, saved and reviewed. */
  async function choose() {
    const dir = await pickFolder({ directory: true, title: "A repository to review" });
    if (typeof dir === "string") await open(dir);
  }

  /** Save the repository `dir` is in; a folder outside one isn't. */
  async function remember(dir: string) {
    try {
      saved = await api.saveRepo(dir);
    } catch {
      // Not a repository: opening it says so.
    }
  }

  async function forget(path: string) {
    saved = await api.forgetRepo(path);
  }

  /** Review `repo`: its changes (against `base`), or pull request `pr`. */
  async function open(repo: string, pr?: string, base?: string) {
    if (busy) return;
    void remember(repo);
    busy = true;
    error = null;
    note = null;
    try {
      const said = await api.openReview(repo, { base: base || undefined, pr, noAgent });
      if (said) note = said;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function add(path: string) {
    try {
      saved = await api.saveRepo(path);
    } catch (e) {
      error = String(e);
    }
  }
</script>

<main class="start" role="region" aria-label="Start">
  <section class="hero">
    <h1>teer</h1>
    <p class="muted">The essence of every revision: review changes by what they mean.</p>
  </section>

  {#if note || error}
    <section class="said" class:bad={!!error} role="status">
      {#if error}<pre>{error}</pre>{:else}<p>{note}</p>{/if}
    </section>
  {/if}

  <OpenPr bind:this={prBox} {busy} onopen={(path, url) => open(path, url)} onsaved={(s) => (saved = s)} />

  <SavedRepos {saved} {busy} onopen={(path, pr) => open(path, pr)} onforget={forget} onadd={() => (adding = "")} />

  {#if others.length}
    <section class="card">
      <h3 class="section">Recent</h3>
      <ul class="recent">
        {#each others as r (r.path)}
          <li>
            <button class="go" disabled={busy} title="Review its changes" onclick={() => open(r.path)}>
              <span class="name">{r.name}</span>
              <span class="path muted mono">{r.path}</span>
            </button>
            <button class="plus" title="Add to your repositories" aria-label="Add {r.name} to your repositories" onclick={() => add(r.path)}>+</button>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  <footer>
    <label class="check">
      <input type="checkbox" bind:checked={noAgent} />
      <span>Open reviews without an agent</span>
    </label>
    <span class="faint">Drop a repository's folder on the window to add it.</span>
  </footer>

  {#if adding !== null}
    <AddRepo
      dir={adding}
      onclose={() => (adding = null)}
      onadded={(s) => (saved = s)}
      onreview={(path, base) => open(path, undefined, base)}
    />
  {/if}
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
  .card,
  .said {
    width: min(720px, 100%);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 12px 18px;
  }
  .said p,
  .said pre {
    margin: 0;
  }
  .said.bad {
    border-color: var(--bad);
  }
  .said pre {
    color: var(--bad);
    white-space: pre-wrap;
    font: 12px/1.5 var(--mono);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .recent li {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .go {
    flex: 1;
    min-width: 0;
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
  .go:hover {
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
  .plus {
    border: 1px solid transparent;
    background: none;
    border-radius: 6px;
    color: var(--muted);
    font-weight: 700;
    width: 26px;
    cursor: pointer;
  }
  .plus:hover {
    border-color: var(--border);
    background: var(--sunken);
    color: var(--text);
  }
  footer {
    width: min(720px, 100%);
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 6px 16px;
    font-size: 12px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
  }
</style>
