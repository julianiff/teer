<script lang="ts">
  // The sheet that adds a repository, as settings add an account: its
  // folder (typed, chosen or dropped), checked as it is given, then Add.
  // "Review its changes now" opens a review right away, against a base
  // when one is named. esc or Cancel leaves without saving.
  import { open as pickFolder } from "@tauri-apps/plugin-dialog";
  import { onMount, untrack } from "svelte";
  import { api } from "../lib/api";
  import type { RepoCheck, Saved } from "../lib/types";

  let {
    dir: given = "",
    onclose,
    onadded,
    onreview,
  }: {
    dir?: string;
    onclose: () => void;
    onadded: (saved: Saved[]) => void;
    onreview: (path: string, base: string) => void;
  } = $props();

  // The folder it was opened with (dropped), then the one typed.
  let dir = $state(untrack(() => given));
  let base = $state("");
  let check = $state<RepoCheck | null>(null);
  let problem = $state<string | null>(null);
  let checking = $state(false);
  let saving = $state(false);
  let field: HTMLInputElement | undefined = $state();
  let asked = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const firstLine = (s: string) => s.replace(/^Error:\s*/, "").split("\n")[0];

  /** Check the folder; only the last one asked about answers. */
  async function examine() {
    clearTimeout(timer);
    const d = dir.trim();
    const n = ++asked;
    check = null;
    problem = null;
    if (!d) return;
    checking = true;
    try {
      const c = await api.checkRepo(d);
      if (n === asked) check = c;
    } catch (e) {
      if (n === asked) problem = firstLine(String(e));
    } finally {
      if (n === asked) checking = false;
    }
  }

  function typed() {
    clearTimeout(timer);
    timer = setTimeout(examine, 250);
  }

  async function choose() {
    const d = await pickFolder({ directory: true, title: "A repository to add" });
    if (typeof d === "string") {
      dir = d;
      await examine();
    }
  }

  /** Save it; true once saved. */
  async function save(): Promise<boolean> {
    if (!check || saving) return false;
    saving = true;
    try {
      onadded(await api.saveRepo(check.path));
      return true;
    } catch (e) {
      problem = firstLine(String(e));
      return false;
    } finally {
      saving = false;
    }
  }

  async function add() {
    if (await save()) onclose();
  }

  async function review() {
    const path = check?.path;
    if (path && (await save())) {
      onclose();
      onreview(path, base.trim());
    }
  }

  function key(e: KeyboardEvent) {
    if (e.key === "Escape") onclose();
    else if (e.key === "Enter" && check && (e.target as HTMLElement).tagName === "INPUT") void add();
    else return;
    e.preventDefault();
  }

  onMount(() => {
    field?.focus();
    if (dir) void examine();
    return () => clearTimeout(timer);
  });
</script>

<div class="backdrop" role="presentation" onclick={onclose}>
  <div
    class="sheet"
    role="dialog"
    aria-modal="true"
    aria-labelledby="add-title"
    tabindex="-1"
    data-own-keys
    onclick={(e) => e.stopPropagation()}
    onkeydown={key}
  >
    <h2 id="add-title">Add a repository</h2>
    <p class="lead muted">Its pull requests waiting for your review show on the start screen.</p>

    <label class="field" for="add-dir">Folder</label>
    <div class="row">
      <input
        id="add-dir"
        type="text"
        bind:this={field}
        bind:value={dir}
        oninput={typed}
        placeholder="~/code/shop"
        spellcheck="false"
        autocomplete="off"
      />
      <button class="btn" onclick={choose}>Choose…</button>
    </div>
    <p class="status" aria-live="polite">
      {#if checking}
        <span class="muted">Checking…</span>
      {:else if check}
        <span class="ok">✓</span>
        <strong>{check.name}</strong>
        <span class="muted">
          {check.github ? `· ${check.github} on GitHub` : "· no GitHub remote: no pull requests to list"}
        </span>
      {:else if problem}
        <span class="bad">{problem}</span>
      {:else}
        <span class="faint">A folder in a git repository; a folder inside one adds the whole repository.</span>
      {/if}
    </p>

    <details>
      <summary>Review its changes now</summary>
      <div class="now">
        <label class="field" for="add-base">Base <span class="faint">(optional)</span></label>
        <input id="add-base" type="text" bind:value={base} placeholder="found by itself" spellcheck="false" />
        <button class="btn" disabled={!check || saving} onclick={review}>Add and review its changes</button>
      </div>
    </details>

    <div class="buttons">
      <button class="btn" onclick={onclose}>Cancel</button>
      <button class="btn primary" disabled={!check || saving} onclick={add}>Add repository</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: 12vh 16px 16px;
    background: color-mix(in srgb, var(--bg) 55%, transparent);
  }
  .sheet {
    width: min(520px, 100%);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 18px 60px rgba(0, 0, 0, 0.28);
    padding: 18px 20px 16px;
  }
  h2 {
    font-size: 15px;
    margin: 0;
  }
  .lead {
    margin: 4px 0 14px;
    font-size: 12px;
  }
  .field {
    display: block;
    font-size: 12px;
    font-weight: 600;
    margin-bottom: 4px;
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
    margin: 6px 0 12px;
    font-size: 12px;
  }
  .ok {
    color: var(--ok);
    font-weight: 700;
  }
  .bad {
    color: var(--bad);
  }
  details {
    border-top: 1px solid var(--border);
    padding-top: 10px;
    font-size: 12px;
  }
  summary {
    cursor: pointer;
    color: var(--muted);
  }
  .now {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    margin-top: 10px;
  }
  .now input {
    width: 100%;
    font-family: var(--mono);
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
</style>
