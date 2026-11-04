<script lang="ts">
  // What is open over the screen: the keys, a question, the notes, a jump
  // list, the commits, the `:w` sheet. Keys still go to teer; a click picks.
  import { api } from "../lib/api";
  import type { Overlay } from "../lib/types";

  let { overlay }: { overlay: Overlay } = $props();
  let box: HTMLElement | undefined = $state();

  $effect(() => {
    void overlay;
    box?.querySelector(".selected")?.scrollIntoView({ block: "nearest" });
  });

  const pick = (i: number) => api.click({ kind: "Item", i });
  const go = (i: number) => {
    void pick(i);
    void api.action("view.open");
  };
  const stateWord = (d: string) => (d === "Investigate" ? "flagged" : d.toLowerCase());
  const jumpTitle = (o: Extract<Overlay, { kind: "Jump" }>) =>
    o.list === "Find"
      ? "Find a unit"
      : "Definition" in o.list
        ? `What ${o.list.Definition.of.split("::").pop()} calls`
        : `What calls ${o.list.Callers.of.split("::").pop()}`;
</script>

<div class="backdrop">
  <div class="modal {overlay.kind}" bind:this={box} role="dialog" aria-modal="true">
    {#if overlay.kind === "Help"}
      <h2>Keys · {overlay.of}</h2>
      <div class="help">
        {#each overlay.groups as g (g.name)}
          <section>
            <h3>{g.name}</h3>
            {#each g.rows as r (r.keys)}
              <div class="hrow"><span class="key">{r.keys}</span><span>{r.what}</span></div>
            {/each}
          </section>
        {/each}
      </div>
      <p class="muted foot">esc closes · ⌘K finds any action</p>
    {:else if overlay.kind === "ConfirmReset"}
      <h2>Reset every decision?</h2>
      <p>{overlay.decisions} decision{overlay.decisions === 1 ? "" : "s"} go back to pending, the ones accepted for you too.</p>
      <div class="buttons">
        <button class="btn primary" onclick={() => api.keys("y")}>Reset</button>
        <button class="btn" onclick={() => api.keys("<esc>")}>Keep them</button>
      </div>
    {:else if overlay.kind === "ConfirmWeb"}
      <h2>May questions use the web?</h2>
      <p>The agent may search the web to answer questions this session.</p>
      <div class="buttons">
        <button class="btn primary" onclick={() => api.keys("y")}>Yes</button>
        <button class="btn" onclick={() => api.keys("n")}>No</button>
      </div>
    {:else if overlay.kind === "ConfirmDeleteNote"}
      <h2>Delete the note on {overlay.name}?</h2>
      <div class="buttons">
        <button class="btn primary" onclick={() => api.keys("y")}>Delete</button>
        <button class="btn" onclick={() => api.keys("<esc>")}>Keep it</button>
      </div>
    {:else if overlay.kind === "Note"}
      <h2>Note on {overlay.name}</h2>
      <div class="editor">{overlay.text}<span class="cursor"></span></div>
      <p class="muted foot">⏎ saves · esc cancels · {overlay.text.length}/{overlay.max}</p>
    {:else if overlay.kind === "Notes"}
      <h2>Notes · {overlay.notes.length}</h2>
      <ul class="list">
        {#each overlay.notes as n, i (n.id)}
          <li>
            <button class:selected={i === overlay.sel} onclick={() => pick(i)} ondblclick={() => go(i)}>
              <span class="lvl-{n.level}">●</span>
              <strong class="mono">{n.name}</strong>
              <span class="muted small">{n.place} · {stateWord(n.decision)}</span>
              <span class="text">{n.text}</span>
              {#if n.stale}<span class="muted small">(code changed)</span>{/if}
            </button>
          </li>
        {:else}
          <li class="muted">No notes yet. i writes one on the unit on show.</li>
        {/each}
      </ul>
    {:else if overlay.kind === "Jump"}
      <h2>{jumpTitle(overlay)}</h2>
      {#if overlay.list === "Find"}
        <div class="editor">/{overlay.query}<span class="cursor"></span></div>
      {/if}
      <ul class="list">
        {#each overlay.items as it, i (it.id)}
          <li>
            <button class:selected={i === overlay.sel} onclick={() => pick(i)} ondblclick={() => go(i)}>
              <span class="lvl-{it.level}">●</span>
              <strong class="mono">{it.name}</strong>
              <span class="muted small">{it.place}</span>
              <span class="muted small right">{stateWord(it.decision)}</span>
            </button>
          </li>
        {:else}
          <li class="muted">Nothing matches.</li>
        {/each}
      </ul>
    {:else if overlay.kind === "Commits"}
      <h2>Commits and description</h2>
      {#if overlay.pr}
        <h3>{overlay.pr.title} <span class="muted">@{overlay.pr.author}</span></h3>
        {#if overlay.pr.body}<p class="body">{overlay.pr.body}</p>{/if}
      {/if}
      <ul class="list">
        {#each overlay.commits as c, i (c.sha)}
          <li>
            <button class:selected={i === overlay.sel && overlay.unit === null} onclick={() => pick(i)}>
              <span class="mono muted">{c.short}</span>
              <span>{c.subject}</span>
              <span class="muted small right">{c.author} · {c.date}</span>
            </button>
          </li>
        {:else}
          <li class="muted">No commits: these are uncommitted changes.</li>
        {/each}
      </ul>
      {#if overlay.commits[overlay.sel]?.body}
        <p class="body">{overlay.commits[overlay.sel].body}</p>
      {/if}
      {#if overlay.units.length}
        <h3>Units it changed</h3>
        <ul class="list">
          {#each overlay.units as u, i (u.id)}
            <li>
              <button class:selected={overlay.unit === i} onclick={() => api.click({ kind: "Open", id: u.id })}>
                <span class="lvl-{u.level}">●</span> <span class="mono">{u.name}</span>
                <span class="muted small">{u.place}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    {:else if overlay.kind === "Finish"}
      <h2>Finish · {overlay.subject}</h2>
      {#if overlay.not_shown}<p class="muted small">{overlay.not_shown}</p>{/if}
      <ul class="steps">
        {#each overlay.steps as s, i (s.step)}
          <li>
            <button class:selected={i === overlay.sel} onclick={() => pick(i)} ondblclick={() => api.keys("<cr>")}>
              {#if i === 0}<span class="default">default</span>{/if}
              {s.label}
            </button>
          </li>
        {/each}
      </ul>
      <div class="details">
        {#if overlay.confirm}
          <p class="confirm">Units are still pending. {overlay.confirm} anyway? <span class="key">y</span> yes · <span class="key">n</span> no</p>
        {:else if overlay.editing !== null}
          <div class="editor">{overlay.editing}<span class="cursor"></span></div>
        {:else if overlay.details.kind === "Text"}
          <p class="muted">{overlay.details.text}</p>
        {:else if overlay.details.kind === "Commit"}
          {@const d = overlay.details}
          <p>Commits {d.files.length} file{d.files.length === 1 ? "" : "s"}{d.branch ? ` on ${d.branch}` : d.detached ? " on a detached HEAD" : ""}:</p>
          <ul class="files mono">
            {#each d.files as [sym, path] (path)}<li><span class="sym s{sym}">{sym}</span> {path}</li>{/each}
          </ul>
          {#if d.others}<p class="muted small">{d.others} other changed file{d.others === 1 ? " is" : "s are"} not part of this review and will not be included.</p>{/if}
          <h3>Message <span class="muted small">{d.draft === "Pending" ? "drafting…" : d.draft === "Drafted" ? "drafted by the agent" : d.draft === "Template" ? "template" : ""}</span></h3>
          <pre class="message">{d.message}</pre>
        {:else if overlay.details.kind === "Post"}
          {@const d = overlay.details}
          <p class="muted small">{d.head_check}</p>
          {#if d.needs_comment}
            <p>Write the comment first: <span class="key">i</span> or ⏎ opens the editor.</p>
          {:else}
            <h3>Summary comment</h3>
            <pre class="message">{d.body}</pre>
            <h3>{d.inline.length ? `${d.inline.length} inline comment${d.inline.length === 1 ? "" : "s"}` : "No inline comments."}</h3>
            {#each d.inline as c, i (i)}
              <p class="small"><span class="mono">{c.path}:{c.line}{c.old ? " (old)" : ""}</span> {c.body}</p>
            {/each}
          {/if}
        {/if}
      </div>
      {#if overlay.notes.length}
        <h3>Notes · {overlay.notes.length}</h3>
        <ul class="list">
          {#each overlay.notes as n (n.id)}
            <li class="note"><span class="lvl-{n.level}">●</span> <strong class="mono">{n.name}</strong> <span class="muted small">{n.place}</span> {n.text}</li>
          {/each}
        </ul>
      {/if}
      <div class="buttons">
        <button class="btn primary" onclick={() => api.keys("<cr>")}>{overlay.steps[overlay.sel]?.verb ?? "go"} ⏎</button>
        <button class="btn" onclick={() => api.keys("<esc>")}>Close</button>
      </div>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--bg) 55%, transparent);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: 6vh 16px;
    z-index: 10;
  }
  .modal {
    width: min(760px, 100%);
    max-height: 86vh;
    overflow: auto;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 18px 60px rgba(0, 0, 0, 0.25);
    padding: 16px 20px;
  }
  .modal.Help {
    width: min(1000px, 100%);
  }
  h2 {
    font-size: 15px;
    margin: 0 0 10px;
  }
  h3 {
    font-size: 12px;
    margin: 14px 0 4px;
  }
  .help {
    columns: 3 260px;
    column-gap: 24px;
  }
  .help section {
    break-inside: avoid;
    margin-bottom: 12px;
  }
  .help h3 {
    margin-top: 0;
    color: var(--muted);
  }
  .hrow {
    display: grid;
    grid-template-columns: 90px 1fr;
    gap: 8px;
    padding: 1px 0;
  }
  .foot {
    font-size: 12px;
    margin: 10px 0 0;
  }
  .buttons {
    display: flex;
    gap: 8px;
    margin-top: 14px;
  }
  .list,
  .steps,
  .files {
    list-style: none;
    padding: 0;
    margin: 4px 0;
  }
  .list button,
  .steps button {
    display: flex;
    gap: 8px;
    align-items: baseline;
    width: 100%;
    text-align: left;
    border: 0;
    background: none;
    padding: 4px 8px;
    border-radius: 6px;
    cursor: pointer;
  }
  .list button.selected,
  .steps button.selected {
    background: var(--select);
    box-shadow: inset 3px 0 0 var(--accent);
  }
  .steps button {
    font-size: 14px;
    padding: 6px 10px;
  }
  .default {
    font-size: 10px;
    text-transform: uppercase;
    color: var(--accent);
    border: 1px solid var(--accent);
    border-radius: 4px;
    padding: 0 4px;
  }
  .right {
    margin-left: auto;
  }
  .text {
    flex: 1;
  }
  .small {
    font-size: 11px;
  }
  .editor {
    border: 1px solid var(--accent);
    border-radius: 7px;
    padding: 8px 10px;
    white-space: pre-wrap;
    min-height: 34px;
  }
  .cursor {
    display: inline-block;
    width: 7px;
    height: 14px;
    background: var(--accent);
    vertical-align: -2px;
  }
  .details {
    border-top: 1px solid var(--border);
    margin-top: 8px;
    padding-top: 8px;
  }
  .message,
  .body {
    white-space: pre-wrap;
    font: 12.5px/1.5 var(--mono);
    background: var(--sunken);
    border-radius: 6px;
    padding: 8px 10px;
    margin: 4px 0;
  }
  .body {
    font-family: var(--sans);
  }
  .sym.s\+ {
    color: var(--added-sign);
  }
  .sym.s− {
    color: var(--removed-sign);
  }
  .confirm {
    color: var(--look);
  }
  .note {
    padding: 2px 8px;
  }
</style>
