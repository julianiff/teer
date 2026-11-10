<script lang="ts">
  // A tile's inside: an answer (the advice, the tests, old against new,
  // the diagram, the security findings), the conversation, or what the
  // unit calls outside. Everything the agent wrote is plain text.
  import { api } from "../lib/api";
  import type { Audit, Cite, Finding, Outcome, Risk, SecurityClass, Tile } from "../lib/types";
  import AskTile from "./AskTile.svelte";
  import Second from "./Second.svelte";
  import ShownCode from "./ShownCode.svelte";

  let { tile, typing }: { tile: Tile; typing: boolean } = $props();

  const dark = window.matchMedia("(prefers-color-scheme: dark)");
  let isDark = $state(dark.matches);
  $effect(() => {
    const f = () => (isDark = dark.matches);
    dark.addEventListener("change", f);
    return () => dark.removeEventListener("change", f);
  });

  const out = (o: Outcome) => (o.kind === "missing" ? "—" : o.kind === "error" ? `error: ${o.text}` : o.text);
  const reach = (r: { how: string; line?: number; via?: string; hops?: number }) =>
    r.how === "calls" ? `calls it at line ${r.line}` : r.how === "through" ? `through ${r.via} (${r.hops} calls)` : "names it";

  // The Security tile, as the terminal draws it (`security::Class::word`).
  const WORD: Record<SecurityClass, string> = {
    authorization: "auth",
    entry_point: "entry points",
    injection: "injection",
    secret: "secrets",
    exposure: "data exposure",
    supply_chain: "packages",
    ci: "ci",
    config: "config",
    crypto: "crypto",
    llm: "llm",
  };
  const GLYPH: Record<Risk, string> = { high: "▲", medium: "◆", low: "●" };
  const WEIGHT: Record<string, string> = { attention: "▲", look: "◆", note: "·" };
  const RANK: Record<Risk, number> = { high: 0, medium: 1, low: 2 };
  const at = (c: Cite) => `${c.file}:${c.start}${c.end > c.start ? `-${c.end}` : ""}`;
  const stands = (f: Finding) => f.path_verified && (!f.second || f.second.agree);
  // Those that stand first, most severe first.
  const ordered = (fs: Finding[]) =>
    [...fs].sort((a, b) => Number(stands(b)) - Number(stands(a)) || RANK[a.severity] - RANK[b.severity]);
  const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
  // Never "safe": no finding is only no finding.
  const head = (a: Audit) =>
    a.findings.length === 0
      ? "No finding."
      : plural(a.findings.length, "finding", "findings") +
        (a.holds.length ? ` · ${plural(a.holds.length, "check holds", "checks hold")}` : "");
</script>

{#if tile.body.kind === "Ask"}
  <AskTile ask={tile.body} {typing} />
{:else if tile.body.kind === "Outside"}
  {#each tile.body.shown as s, i (i)}
    <ShownCode shown={s} />
  {:else}
    <p class="muted">Nothing outside the review.</p>
  {/each}
{:else}
  {@const a = tile.body}
  <p class="label muted">{a.label}</p>
  {#if a.facts?.length}
    <h4>teer found</h4>
    <ul class="plain">
      {#each a.facts as f, i (i)}
        <li>
          <span class="weight-{f.weight}">{WEIGHT[f.weight]}</span>
          {f.text}
          <span class="muted small">
            {WORD[f.class]}{f.subject ? ` · ${f.subject}` : ""}{f.lines
              ? ` · line ${f.lines.start}${f.side === "old" ? " (old)" : ""}`
              : ""}{f.origin ? ` · ${f.origin.tool} ${f.origin.rule}` : ""}
          </span>
        </li>
      {/each}
    </ul>
  {/if}
  {#if a.result}
    {@const r = a.result}
    {#if r.audit}
      {@const au = r.audit}
      <h4>{head(au)}</h4>
      {#if r.text}<p>{r.text}</p>{/if}
      {#each ordered(au.findings) as f, i (i)}
        <div class="finding" class:weak={!stands(f)}>
          <p>
            <span class="risk-{f.severity}">{GLYPH[f.severity]}</span>
            <strong>{f.title}</strong>
            <span class="muted small">({f.severity} · {WORD[f.class]})</span>
          </p>
          <ul class="plain path">
            <li><span class="muted">source</span> <span class="mono small">{at(f.source)}</span></li>
            {#each f.path as h, k (k)}
              <li>→ {h.symbol} <span class="mono small muted">{at(h.cite)}</span></li>
            {/each}
            <li><span class="muted">sink</span> <span class="mono small">{at(f.sink)}</span></li>
          </ul>
          {#if f.missing}<p class="missing">✗ {f.missing}</p>{/if}
          {#if f.exploit}<p class="muted small">{f.exploit}</p>{/if}
          {#if !f.path_verified}
            <p class="risk-medium small">path not verified: teer's call graph doesn't confirm every call</p>
          {/if}
          {#if f.second?.agree}
            <p class="ok small">second model agrees</p>
          {:else if f.second}
            <p class="bad small">
              ⚠ second model disputes: {f.second.why}{f.second.evidence ? ` (${at(f.second.evidence)})` : ""}
            </p>
          {/if}
          {#if f.proof?.state === "unproven"}
            <p class="muted small">unproven: {f.proof.why}</p>
          {:else if f.proof}
            <p class={f.proof.state === "proven" ? "bad small" : "muted small"}>
              {f.proof.state === "proven" ? "proven" : "not shown"}: {f.proof.case}
            </p>
            <p class="muted small">before: {f.proof.before} · after: {f.proof.after}</p>
          {/if}
        </div>
      {/each}
      {#if au.holds.length}
        <ul class="plain">
          {#each au.holds as h, i (i)}
            <li><span class="ok">✓</span> {h.what} <span class="mono small muted">{at(h.cite)}</span></li>
          {/each}
        </ul>
      {/if}
      <p class="muted small">checked: {au.checked.map((c) => WORD[c]).join(" · ")}</p>
      {#if au.not_checked.length}
        <p class="muted small">not: {au.not_checked.join(" · ")}</p>
      {/if}
      {#if au.dropped}
        <p class="muted small">
          {plural(au.dropped, "finding", "findings")} dropped: their lines didn't check out (teer.log says which)
        </p>
      {/if}
    {:else if a.svg}
      <p>{r.text}</p>
      <div class="diagram" style={isDark ? a.svg.dark : a.svg.light}>{@html a.svg.svg}</div>
      <button class="link" onclick={() => api.action("goto.browser")}>Open it in the browser</button>
    {:else if r.behavior}
      <p>{r.text}</p>
      <table>
        <thead><tr><th>case</th><th>before</th><th>after</th><th></th></tr></thead>
        <tbody>
          {#each r.behavior.cases as c, i (i)}
            {@const changed = JSON.stringify(c.before) !== JSON.stringify(c.after)}
            {@const surprise = changed !== (c.expect === "different")}
            <tr class:changed class:surprise>
              <td><strong>{c.name}</strong><div class="muted small">{c.input}</div></td>
              <td class="mono">{out(c.before)}</td>
              <td class="mono">{out(c.after)}</td>
              <td>{surprise ? "⚠ unexpected" : changed ? "Δ" : "="}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if r.behavior.tests}
        <p class="muted small">Project tests: {r.behavior.tests.kind.replaceAll("_", " ")}{"detail" in r.behavior.tests ? ` — ${r.behavior.tests.detail}` : ""}</p>
      {/if}
      <p class="muted small mono">{r.behavior.command.join(" ")}</p>
    {:else if r.coverage}
      {#if r.text}<p>{r.text}</p>{/if}
      {#if r.coverage.search && !r.coverage.search.tests.length}
        <p class="muted">teer found no test that reaches it.</p>
      {:else if r.coverage.search}
        <h4>Found by teer · {r.coverage.search.total}</h4>
        <ul>
          {#each r.coverage.search.tests as t, i (i)}
            <li><span class="mono">{t.name}</span> <span class="muted small">{t.cite.file}:{t.cite.start} · {reach(t.reach)}</span></li>
          {/each}
        </ul>
      {/if}
      {#if r.coverage.judged}
        <h4>Covers it</h4>
        <ul>
          {#each r.coverage.covered as t, i (i)}
            <li><span class="mono">{t.name}</span> — {t.checks}</li>
          {:else}
            <li class="muted">No test covers it.</li>
          {/each}
        </ul>
        {#if r.coverage.gaps.length}
          <h4>Not covered</h4>
          <ul>
            {#each r.coverage.gaps as g, i (i)}
              <li><span class="risk-{g.risk}">●</span> <strong>{g.case}</strong>: {g.danger}</li>
            {/each}
          </ul>
        {/if}
      {/if}
    {:else}
      <p class="answer">{r.text}</p>
      {#if r.sources.length}
        <h4>Sources</h4>
        <ul>{#each r.sources as s, i (i)}<li class="muted small">{s}</li>{/each}</ul>
      {/if}
    {/if}
  {/if}
  {#if a.waiting}
    <p class="waiting"><span class="spin"></span> {a.waiting}</p>
  {/if}
  {#if a.ask}
    <button class="ask" onclick={() => api.action(a.ask!.action)}>
      <span class="key">{a.ask.keys}</span> {a.ask.what}
    </button>
  {/if}
  {#if a.none}<p class="muted">{a.none}</p>{/if}
  {#if a.second}<Second second={a.second} />{/if}
{/if}

<style>
  .label {
    font-size: 11px;
    margin: 0 0 6px;
  }
  .answer {
    white-space: pre-wrap;
  }
  .diagram :global(svg) {
    max-width: 100%;
    height: auto;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
  }
  th {
    text-align: left;
    color: var(--muted);
    font-weight: 500;
  }
  td,
  th {
    padding: 3px 6px;
    border-bottom: 1px solid var(--border);
    vertical-align: top;
  }
  tr.changed td:last-child {
    color: var(--accent);
  }
  tr.surprise td:last-child {
    color: var(--attention);
  }
  h4 {
    font-size: 12px;
    margin: 10px 0 4px;
  }
  ul {
    padding-left: 16px;
    margin: 4px 0;
  }
  .small {
    font-size: 11px;
  }
  .waiting {
    color: var(--muted);
  }
  .spin {
    display: inline-block;
    width: 10px;
    height: 10px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  ul.plain {
    list-style: none;
    padding-left: 0;
  }
  .finding {
    margin: 10px 0;
  }
  .finding p {
    margin: 2px 0;
  }
  .finding.weak strong {
    font-weight: 500;
  }
  .path {
    padding-left: 18px !important;
  }
  .missing,
  .bad {
    color: var(--bad);
  }
  .ok {
    color: var(--ok);
  }
  .weight-attention {
    color: var(--attention);
  }
  .weight-look {
    color: var(--look);
  }
  .weight-note {
    color: var(--muted);
  }
  .ask,
  .link {
    border: 0;
    background: none;
    padding: 4px 0;
    cursor: pointer;
    text-align: left;
    font-weight: 600;
  }
  .link {
    color: var(--accent);
    font-weight: 400;
  }
</style>
