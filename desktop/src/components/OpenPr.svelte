<script lang="ts">
  // "Open a pull request": paste its link and it opens in the clone of its
  // repository among yours, at once; typed, ⏎ opens it. With no clone of
  // it yet, its folder is asked for once and saved.
  import { open as pickFolder } from "@tauri-apps/plugin-dialog";
  import { api } from "../lib/api";
  import type { PrLink, Saved } from "../lib/types";

  let {
    busy = false,
    onopen,
    onsaved,
  }: {
    busy?: boolean;
    onopen: (path: string, url: string) => void;
    onsaved: (saved: Saved[]) => void;
  } = $props();

  let url = $state("");
  let link = $state<PrLink | null>(null);
  let problem = $state<string | null>(null);
  let checking = $state(false);
  let field: HTMLInputElement | undefined = $state();
  let asked = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const firstLine = (s: string) => s.replace(/^Error:\s*/, "").split("\n")[0];

  /** Read the link; `open` it once its clone is known. */
  async function examine(open = false) {
    clearTimeout(timer);
    const u = url.trim();
    const n = ++asked;
    link = null;
    problem = null;
    if (!u) return;
    checking = true;
    try {
      const l = await api.prLink(u);
      if (n !== asked) return;
      link = l;
      if (open) go();
    } catch (e) {
      if (n === asked) problem = firstLine(String(e));
    } finally {
      if (n === asked) checking = false;
    }
  }

  function go() {
    if (link?.clone && !busy) onopen(link.clone.path, link.url);
  }

  /** The window's menu (Open Pull Request…) comes here. */
  export function focus() {
    field?.focus();
  }

  function typed() {
    clearTimeout(timer);
    timer = setTimeout(() => examine(), 300);
  }

  function key(e: KeyboardEvent) {
    if (e.key !== "Enter") return;
    e.preventDefault();
    if (link?.url === url.trim()) go();
    else void examine(true);
  }

  // Pasted, it opens: the value is in only after the paste.
  const pasted = () => setTimeout(() => examine(true));

  /** No clone of it is known: its folder, saved, then it opens. */
  async function locate() {
    if (!link) return;
    const dir = await pickFolder({ directory: true, title: `A clone of ${link.repo}` });
    if (typeof dir !== "string") return;
    try {
      onsaved(await api.saveRepo(dir));
    } catch (e) {
      problem = firstLine(String(e));
      return;
    }
    await examine(true);
    if (link && !link.clone) problem = `That folder isn't a clone of ${link.repo}.`;
  }
</script>

<section class="card" aria-labelledby="pr-title">
  <label id="pr-title" class="title" for="pr-url">Open a pull request</label>
  <div class="row">
    <input
      id="pr-url"
      type="text"
      inputmode="url"
      bind:this={field}
      bind:value={url}
      oninput={typed}
      onpaste={pasted}
      onkeydown={key}
      placeholder="Paste its link: https://github.com/owner/repo/pull/123"
      spellcheck="false"
      autocomplete="off"
    />
    <button class="btn primary" disabled={!link?.clone || busy} onclick={go}>Open</button>
  </div>
  <p class="status" aria-live="polite">
    {#if checking}
      <span class="muted">Reading the link…</span>
    {:else if problem}
      <span class="bad">{problem}</span>
    {:else if link?.clone}
      <span class="ok">✓</span> {link.repo} #{link.number}
      <span class="muted">· opens in your clone</span> <strong>{link.clone.name}</strong>
    {:else if link}
      {link.repo} #{link.number}: none of your repositories is a clone of {link.repo}.
      <button class="link" onclick={locate}>Choose its folder…</button>
    {/if}
  </p>
</section>

<style>
  .card {
    width: min(720px, 100%);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 12px 18px 10px;
  }
  .title {
    display: block;
    font-size: 12px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
    margin-bottom: 8px;
  }
  .row {
    display: flex;
    gap: 8px;
  }
  .row input {
    flex: 1;
    font-family: var(--mono);
  }
  .status {
    min-height: 18px;
    margin: 6px 0 0;
    font-size: 12px;
  }
  .ok {
    color: var(--ok);
    font-weight: 700;
  }
  .bad {
    color: var(--bad);
  }
  .link {
    border: 0;
    background: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font-size: inherit;
  }
  .link:hover {
    text-decoration: underline;
  }
</style>
