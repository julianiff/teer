<script lang="ts">
  // A tile's inside: an answer (the tests, the diagram, the security
  // findings), the advice and the conversation, or what the
  // unit calls outside. Everything the agent wrote is plain text. A tile
  // with a textual result can send it to an agent session (`s`).
  import { api } from "../lib/api";
  import type { Audit, Cite, Finding, Risk, SecurityClass, Tile } from "../lib/types";
  import Act from "./Act.svelte";
  import AskTile from "./AskTile.svelte";
  import Second from "./Second.svelte";
  import ShownCode from "./ShownCode.svelte";

  let { tile, typing }: { tile: Tile; typing: boolean } = $props();

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
  // What `s` sends from it: the last answer (else the advice), the
  // findings that stand (else teer's facts), the gaps no test covers.
  // teer says why when there is nothing after all.
  const sendable = (t: Tile): boolean => {
    const b = t.body;
    // The last answer of the conversation, else the advice.
    if (b.kind === "Ask") return b.exchanges.some((e) => !!e.answer) || !!b.advice?.result?.text.trim();
    if (b.kind !== "Answer" || b.waiting) return false;
    const r = b.result;
    switch (t.id) {
      case "Security":
        return !!r?.audit?.findings.some(stands) || !!b.facts?.length;
      case "Tests":
        return !!r?.coverage?.judged && r.coverage.gaps.length > 0;
      default:
        return false;
    }
  };
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
{:else if tile.body.kind === "Note"}
  <p class="note">{tile.body.text}</p>
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
            <p class="ok small">check agrees</p>
          {:else if f.second}
            <p class="bad small">
              ⚠ disputed: {f.second.why}{f.second.evidence ? ` (${at(f.second.evidence)})` : ""}
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
{#if sendable(tile)}
  <div class="send">
    <Act id="send.session" label="Send to session" before={() => api.click({ kind: "Tile", id: tile.id })} />
  </div>
{/if}

<style>
  .label {
    font-size: 11px;
    margin: 0 0 6px;
  }
  .answer {
    white-space: pre-wrap;
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
  .send {
    margin-top: 10px;
  }
</style>
