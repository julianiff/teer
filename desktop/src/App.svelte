<script lang="ts">
  // The window: the start screen until a review is open, then the review
  // teer sends as a view, and the keys, clicks and pastes sent back.
  import { onMount } from "svelte";
  import { api, on } from "./lib/api";
  import { notation } from "./lib/keys";
  import { scrollFor } from "./lib/scroll";
  import { ui } from "./lib/ui.svelte";
  import type { View } from "./lib/types";
  import Start from "./components/Start.svelte";
  import Review from "./components/Review.svelte";
  import Palette from "./components/Palette.svelte";
  import Terminal from "./components/Terminal.svelte";
  import TextEditor from "./components/TextEditor.svelte";
  import CommitPanel from "./components/CommitPanel.svelte";
  import Settings from "./components/Settings.svelte";

  let view = $state<View | null>(null);
  let opening = $state<string | null>(null);
  let ended = $state<{ line: string | null } | null>(null);
  let palette = $state(false);
  let startMode = $state<"folder" | "pr" | null>(null);
  // What a review the app opened by itself came to, instead of a review.
  let said = $state<string | null>(null);
  // What takes over from the review for a while: the terminal pane, the
  // text editor, the commit's output, the settings.
  let terminal = $state<{ id: number; title: string } | null>(null);
  let editing = $state<{ text: string; title: string } | null>(null);
  let commit = $state<{ lines: string[]; done: boolean; ok: boolean } | null>(null);
  let settings = $state(false);
  const away = $derived(palette || !!terminal || !!editing || settings || ui.prMenu);

  onMount(() => {
    const offs = [
      on<View>("view", (v) => {
        view = v;
        opening = null;
      }),
      on<string>("opening", (repo) => (opening = repo)),
      on<string>("said", (s) => {
        said = s;
        opening = null;
      }),
      on<{ line: string | null }>("ended", (e) => (ended = e)),
      on<{ action: string; count: number }>("scroll", (s) => view && scrollFor(view, s.action, s.count)),
      on<string>("menu", (id) => {
        if (id === "palette" && view) palette = true;
        if (id === "open_pr" && view && !ended) ui.prMenu = true;
        else if (id === "open_folder" || id === "open_pr") {
          if (view && !ended) void api.newWindow();
          else startMode = id === "open_folder" ? "folder" : "pr";
        }
        if (id === "settings") {
          if (view && !ended) void api.action("tool.settings");
          else settings = true;
        }
      }),
      on<{ id: number; title: string }>("pty-open", (t) => (terminal = t)),
      on<{ text: string; title: string }>("edit", (e) => (editing = e)),
      on<null>("settings", () => (settings = true)),
      on<null>("commit-start", () => (commit = { lines: [], done: false, ok: false })),
      on<string>("commit-line", (l) => commit && (commit.lines = [...commit.lines, l])),
      on<boolean>("commit-end", (ok) => {
        if (!commit) return;
        commit.done = true;
        commit.ok = ok;
        if (ok) setTimeout(() => (commit = null), 1500);
      }),
    ];
    // A window that loads again (or opened by the app) finds what it shows.
    api.loaded().then((l) => {
      opening = l.opening;
      if (l.view) view = l.view;
    });
    return () => offs.forEach((p) => p.then((off) => off()));
  });

  function onKey(e: KeyboardEvent) {
    // The start screen and the palette type into their own fields.
    if (!view || ended || away) return;
    if ((e.target as HTMLElement)?.closest?.("input, textarea, [data-own-keys]")) return;
    const n = notation(e);
    if (!n) return;
    e.preventDefault();
    void api.keys(n);
  }

  function onPaste(e: ClipboardEvent) {
    if (!view || ended || away) return;
    if ((e.target as HTMLElement)?.closest?.("input, textarea")) return;
    const text = e.clipboardData?.getData("text/plain");
    if (text) {
      e.preventDefault();
      void api.paste(text);
    }
  }
</script>

<svelte:window onkeydown={onKey} onpaste={onPaste} />

{#if ended}
  <main class="ended">
    <h1>Review over</h1>
    {#if ended.line}<p>{ended.line}</p>{/if}
    <p class="muted">The review is saved; opening it again goes on where you left it.</p>
    <div class="row">
      <button class="btn primary" onclick={() => api.newWindow()}>Open another review</button>
      <button class="btn" onclick={() => window.close()}>Close the window</button>
    </div>
  </main>
{:else if view}
  <Review {view} />
  {#if palette}
    <Palette onclose={() => (palette = false)} />
  {/if}
{:else if opening}
  <main class="opening">
    <div class="spinner"></div>
    <p>Opening <span class="mono">{opening}</span>…</p>
    <p class="muted">teer reads the changes, finds the agent, and restores earlier decisions.</p>
  </main>
{:else}
  <Start bind:mode={startMode} {said} />
{/if}

{#if terminal}
  {#key terminal.id}
    <Terminal id={terminal.id} title={terminal.title} onclose={() => (terminal = null)} />
  {/key}
{/if}
{#if editing}
  <TextEditor text={editing.text} title={editing.title} onclose={() => (editing = null)} />
{/if}
{#if commit}
  <CommitPanel lines={commit.lines} done={commit.done} ok={commit.ok} onclose={() => (commit = null)} />
{/if}
{#if settings}
  <Settings repo={view?.header.repo ?? null} onclose={() => (settings = false)} />
{/if}

<style>
  .ended,
  .opening {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 24px;
    text-align: center;
  }
  .ended h1 {
    font-size: 20px;
    margin: 0 0 8px;
  }
  .row {
    display: flex;
    gap: 10px;
    margin-top: 14px;
  }
  .spinner {
    width: 22px;
    height: 22px;
    border: 3px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    margin-bottom: 10px;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
