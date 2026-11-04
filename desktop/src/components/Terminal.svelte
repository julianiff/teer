<script lang="ts">
  // The terminal pane: the agent CLI the notes went to, or a terminal
  // editor, in a pseudo-terminal teer runs. Its keys are its own; when the
  // program exits the pane says so, and closes.
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";
  import { on } from "../lib/api";

  let { id, title, onclose }: { id: number; title: string; onclose: () => void } = $props();
  let host: HTMLElement | undefined = $state();
  let exited = $state<number | null>(null);

  const decode = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));

  onMount(() => {
    const css = getComputedStyle(document.documentElement);
    const term = new Terminal({
      fontFamily: css.getPropertyValue("--mono"),
      fontSize: 13,
      cursorBlink: true,
      allowProposedApi: false,
      theme: {
        background: css.getPropertyValue("--sunken").trim(),
        foreground: css.getPropertyValue("--text").trim(),
        cursor: css.getPropertyValue("--accent").trim(),
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host!);
    const resize = () => {
      fit.fit();
      void invoke("pty_resize", { id, cols: term.cols, rows: term.rows }).catch(() => {});
    };
    resize();
    term.focus();
    term.onData((data) => void invoke("pty_write", { id, data }).catch(() => {}));
    const observer = new ResizeObserver(resize);
    observer.observe(host!);
    const offs = [
      on<{ id: number; data: string }>("pty-data", (d) => d.id === id && term.write(decode(d.data))),
      on<{ id: number; code: number }>("pty-exit", (e) => {
        if (e.id !== id) return;
        exited = e.code;
        if (e.code === 0) setTimeout(onclose, 900);
      }),
    ];
    return () => {
      observer.disconnect();
      offs.forEach((p) => p.then((off) => off()));
      term.dispose();
    };
  });

  function stop() {
    if (exited === null) void invoke("pty_kill", { id });
    else onclose();
  }
</script>

<div class="pane" data-own-keys>
  <div class="head">
    <strong>{title}</strong>
    {#if exited === null}
      <span class="muted">runs here; quit it to go back to the review</span>
    {:else}
      <span class={exited === 0 ? "ok" : "bad"}>exited{exited ? ` with ${exited}` : ""}</span>
    {/if}
    <span class="spacer"></span>
    <button class="btn" onclick={stop}>{exited === null ? "Stop it" : "Close"}</button>
  </div>
  <div class="term" bind:this={host}></div>
</div>

<style>
  .pane {
    position: fixed;
    inset: 0;
    z-index: 30;
    display: flex;
    flex-direction: column;
    background: var(--sunken);
  }
  .head {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 8px 14px;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
  }
  .spacer {
    flex: 1;
  }
  .term {
    flex: 1;
    min-height: 0;
    padding: 8px 10px;
  }
  .ok {
    color: var(--ok);
  }
  .bad {
    color: var(--bad);
  }
</style>
