<script lang="ts">
  // A level's mark: its shape in its colour, loud for what needs a look,
  // quiet for the routine. `calm` dims it: decided, nothing left to do.
  import { LEVELS } from "../lib/levels";
  import type { Level } from "../lib/types";
  let { level, calm = false }: { level: Level; calm?: boolean } = $props();
</script>

<span class="mark lvl-{level}" class:calm role="img" aria-label={LEVELS[level].title} title={LEVELS[level].title}
  >{LEVELS[level].icon}</span
>

<style>
  .mark {
    display: inline-block;
    width: 14px;
    text-align: center;
    font-size: 12px;
    flex: none;
  }
  .lvl-Routine,
  .lvl-Same {
    opacity: 0.55;
  }
  .calm {
    color: var(--same);
    opacity: 0.45;
  }
</style>
