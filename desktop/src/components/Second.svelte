<script lang="ts">
  // What a second model said about the description.
  import type { SecondModel } from "../lib/types";
  let { second }: { second: SecondModel } = $props();
</script>

<section class="second">
  <h3 class="section">Check <span class="muted by">{second.label}</span></h3>
  {#if second.failed}
    <p class="muted">{second.failed}</p>
  {:else}
    <p>{second.head}</p>
    <ul>
      {#each second.verdicts as v, i (i)}
        <li>
          <span class={v.agree ? "ok" : "bad"}>{v.agree ? "✓" : "✗"}</span>
          <div>
            <p>“{v.statement}”</p>
            <p class="muted small">{v.why} ({v.cites})</p>
          </div>
        </li>
      {/each}
      {#each second.missed as m, i (i)}
        <li>
          <span class="risk-{m.risk}">+</span>
          <div><p>missed [{m.risk} risk]: {m.statement} <span class="muted">({m.cites})</span></p></div>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .by {
    text-transform: none;
    letter-spacing: 0;
    font-weight: 400;
  }
  ul {
    list-style: none;
    padding: 0;
    margin: 6px 0;
  }
  li {
    display: flex;
    gap: 8px;
    margin: 6px 0;
  }
  p {
    margin: 0;
  }
  .ok {
    color: var(--ok);
  }
  .bad {
    color: var(--bad);
  }
  .small {
    font-size: 12px;
  }
</style>
