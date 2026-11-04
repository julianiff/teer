<script lang="ts">
  // The settings: which agent prepares reviews (Claude Code, Codex, a model
  // API), its fast and second model, a test answer, and what teer finds of
  // the programs it runs. Saved into teer's config, comments kept; the
  // review open in the window takes them at once.
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";

  interface Preset {
    name: string;
    about: string;
    api: boolean;
    custom: boolean;
    base_url: string;
    key_env: string;
    suggest: [string, string];
    found: boolean | null;
  }
  interface SetupState {
    presets: Preset[];
    current: number;
    base_url: string;
    key_env: string;
    fast: string | null;
    second: string | null;
    second_opinion: boolean;
    config_path: string;
  }
  interface ToolView {
    name: string;
    needed: boolean;
    state: "Ready" | "Broken" | "Missing";
    found: string;
    about: string;
  }

  let { repo = null, onclose }: { repo?: string | null; onclose: () => void } = $props();

  let s = $state<SetupState | null>(null);
  let preset = $state(0);
  let baseUrl = $state("");
  let keyEnv = $state("");
  let fast = $state("");
  let second = $state("");
  let useSecond = $state(true);
  let models = $state<string[]>([]);
  let modelsError = $state<string | null>(null);
  let tools = $state<ToolView[] | null>(null);
  let test = $state<{ ok: boolean; text: string } | null>(null);
  let testing = $state(false);
  let error = $state<string | null>(null);

  const chosen = $derived(s?.presets[preset]);
  const choice = () => ({
    preset,
    base_url: baseUrl,
    key_env: keyEnv,
    fast,
    second: useSecond && second ? second : null,
  });

  onMount(async () => {
    try {
      s = await invoke<SetupState>("setup_state");
      preset = s.current;
      baseUrl = s.base_url || s.presets[preset].base_url;
      keyEnv = s.key_env || s.presets[preset].key_env;
      fast = s.fast ?? s.presets[preset].suggest[0];
      second = s.second ?? s.presets[preset].suggest[1];
      useSecond = s.second_opinion;
      void loadModels();
    } catch (e) {
      error = String(e);
    }
    invoke<ToolView[]>("setup_tools", { repo })
      .then((t) => (tools = t))
      .catch((e) => (error = String(e)));
  });

  function pick(i: number) {
    if (!s || i === preset) return;
    preset = i;
    const p = s.presets[i];
    baseUrl = p.base_url;
    keyEnv = p.key_env;
    fast = p.suggest[0];
    second = p.suggest[1];
    test = null;
    void loadModels();
  }

  async function loadModels() {
    models = [];
    modelsError = null;
    try {
      models = await invoke<string[]>("setup_models", { choice: choice() });
      if (!fast && models[0]) fast = models[0];
    } catch (e) {
      modelsError = String(e);
    }
  }

  async function runTest() {
    testing = true;
    test = null;
    try {
      const ms = await invoke<number>("setup_test", { choice: choice() });
      test = { ok: true, text: `It answered in ${(ms / 1000).toFixed(1)} s.` };
    } catch (e) {
      test = { ok: false, text: String(e) };
    } finally {
      testing = false;
    }
  }

  async function save() {
    try {
      await invoke("setup_save", { choice: choice() });
      onclose();
    } catch (e) {
      error = String(e);
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onclose()} />

<div class="backdrop" data-own-keys>
  <div class="settings" role="dialog" aria-modal="true">
    <h2>Settings</h2>
    {#if error}<pre class="error">{error}</pre>{/if}
    {#if s}
      <h3 class="section">The agent that prepares reviews</h3>
      <ul class="presets">
        {#each s.presets as p, i (p.name)}
          <li>
            <label class:chosen={i === preset}>
              <input type="radio" name="preset" checked={i === preset} onchange={() => pick(i)} />
              <strong>{p.name}</strong>
              <span class="muted">{p.about}</span>
              {#if p.found === true}<span class="ok">{p.api ? "key set" : "installed"}</span>{/if}
              {#if p.found === false}<span class="faint">{p.api ? `$${p.key_env} isn't set` : "not installed"}</span>{/if}
            </label>
          </li>
        {/each}
      </ul>

      {#if chosen?.api}
        <div class="fields">
          <label>
            <span>Base URL</span>
            <input type="text" bind:value={baseUrl} placeholder="the format's own" spellcheck="false" onchange={loadModels} />
          </label>
          <label>
            <span>Key in the variable</span>
            <input type="text" bind:value={keyEnv} placeholder="none (a local server)" spellcheck="false" onchange={loadModels} />
          </label>
        </div>
      {/if}

      <h3 class="section">Models</h3>
      <div class="fields">
        <label>
          <span>Fast model, describes every unit</span>
          <input type="text" list="models" bind:value={fast} spellcheck="false" />
        </label>
        <label>
          <span>
            <input type="checkbox" bind:checked={useSecond} />
            Second model, checks what the first one says
          </span>
          <input type="text" list="models" bind:value={second} disabled={!useSecond} spellcheck="false" />
        </label>
        <datalist id="models">
          {#each models as m (m)}<option value={m}></option>{/each}
        </datalist>
      </div>
      {#if modelsError}<p class="muted small">Couldn't list the models: {modelsError}</p>{/if}
      <div class="test">
        <button class="btn" disabled={testing || !fast} onclick={runTest}>{testing ? "Asking…" : "Test it"}</button>
        {#if test}<span class={test.ok ? "ok" : "bad"}>{test.text}</span>{/if}
      </div>

      <h3 class="section">Programs teer runs</h3>
      {#if tools}
        <ul class="tools">
          {#each tools as t (t.name)}
            <li>
              <span class="state {t.state}">{t.state === "Ready" ? "✓" : t.state === "Broken" ? "!" : "✗"}</span>
              <strong class="mono">{t.name}</strong>
              <span class="muted">{t.found}{t.needed && t.state !== "Ready" ? " · needed" : ""}</span>
              <span class="about">{t.about}</span>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="muted">Looking…</p>
      {/if}

      <p class="faint small">Saved to {s.config_path}, the terminal's config too.</p>
    {/if}
    <div class="buttons">
      <button class="btn" onclick={onclose}>Cancel</button>
      <button class="btn primary" disabled={!s || !fast} onclick={save}>Save</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 26;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding: 5vh 16px;
    background: color-mix(in srgb, var(--bg) 55%, transparent);
  }
  .settings {
    width: min(780px, 100%);
    max-height: 90vh;
    overflow: auto;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 16px 20px;
    box-shadow: 0 18px 60px rgba(0, 0, 0, 0.3);
  }
  h2 {
    font-size: 16px;
    margin: 0;
  }
  ul {
    list-style: none;
    padding: 0;
    margin: 4px 0;
  }
  .presets label {
    display: flex;
    gap: 8px;
    align-items: baseline;
    padding: 5px 8px;
    border-radius: 6px;
    cursor: pointer;
  }
  .presets label.chosen {
    background: var(--select);
  }
  .fields {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px 14px;
    margin: 8px 0;
  }
  .fields label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 12px;
    color: var(--muted);
  }
  .test {
    display: flex;
    gap: 10px;
    align-items: center;
    margin: 8px 0;
  }
  .tools li {
    display: grid;
    grid-template-columns: 18px 120px auto;
    gap: 2px 8px;
    padding: 4px 0;
  }
  .tools .about {
    grid-column: 3;
    font-size: 12px;
    color: var(--muted);
  }
  .state.Ready {
    color: var(--ok);
  }
  .state.Broken {
    color: var(--look);
  }
  .state.Missing {
    color: var(--bad);
  }
  .ok {
    color: var(--ok);
  }
  .bad,
  .error {
    color: var(--bad);
  }
  .error {
    white-space: pre-wrap;
    font: 12px/1.5 var(--mono);
  }
  .small {
    font-size: 11px;
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 14px;
  }
</style>
