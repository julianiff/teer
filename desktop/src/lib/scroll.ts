// The window scrolls its panes itself. teer says which scroll action a key
// ran; this scrolls the pane it is for: the unit's text or the tile in
// focus in the unit view, the preview on the queue, the flagged list.

import type { View } from "./types";

/** Pixels a line scrolls by, as the text is set. */
const LINE = 20;

/** The pane `view` scrolls. */
export function pane(view: View): HTMLElement | null {
  const by = (name: string) => document.querySelector<HTMLElement>(`[data-scroll="${name}"]`);
  if (view.screen === "Unit") {
    const focus = view.unit?.focus;
    return (focus && by(`tile-${focus}`)) || by("text");
  }
  if (view.screen === "Flagged") return by("flagged");
  return by("preview");
}

/** Where `el`'s hunks start, from its top. */
function hunks(el: HTMLElement): number[] {
  const top = el.getBoundingClientRect().top - el.scrollTop;
  return [...el.querySelectorAll<HTMLElement>("[data-hunk]")].map(
    (h) => h.getBoundingClientRect().top - top,
  );
}

/** Where scrolling `el` for `action`, `count` times, goes; `null` for an action that doesn't scroll. */
export function target(
  el: { scrollTop: number; clientHeight: number; scrollHeight: number },
  action: string,
  count: number,
  starts: number[] = [],
): number | null {
  const page = el.clientHeight;
  const at = el.scrollTop;
  const n = Math.max(1, count);
  switch (action) {
    case "scroll.half_down":
      return at + (page / 2) * n;
    case "scroll.half_up":
      return at - (page / 2) * n;
    case "scroll.page_down":
      return at + page * n;
    case "scroll.page_up":
      return at - page * n;
    case "move.down":
      return at + LINE * n;
    case "move.up":
      return at - LINE * n;
    case "move.first":
      return 0;
    case "move.last":
      return el.scrollHeight;
    case "move.next_hunk": {
      const next = starts.filter((s) => s > at + 4);
      return next.length ? next[Math.min(n, next.length) - 1] - 8 : at;
    }
    case "move.prev_hunk": {
      const prev = starts.filter((s) => s < at - 12).reverse();
      return prev.length ? prev[Math.min(n, prev.length) - 1] - 8 : 0;
    }
    default:
      return null;
  }
}

export function scrollFor(view: View, action: string, count: number) {
  const el = pane(view);
  if (!el) return;
  const to = target(el, action, count, action.includes("hunk") ? hunks(el) : []);
  if (to !== null) el.scrollTo({ top: Math.max(0, to), behavior: "smooth" });
}
