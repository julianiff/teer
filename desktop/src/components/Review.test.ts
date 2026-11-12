// The review drawn from the views teer sends: the snapshots the Rust
// tests keep (tests/it/snapshots/it__view__view_*.snap) mount without an
// error, each button sits where it acts, and a click presses its action.

import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn(async (..._: unknown[]) => null));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import type { HunkState, HunkView, View } from "../lib/types";
import { ui } from "../lib/ui.svelte";
import Review from "./Review.svelte";

const snapshots = import.meta.glob<string>("../../../tests/it/snapshots/it__view__view_*.snap", {
  query: "?raw",
  import: "default",
  eager: true,
});

function view(name: string): View {
  const file = Object.keys(snapshots).find((f) => f.endsWith(`view_${name}.snap`))!;
  // An insta snapshot: a header between `---` lines, then the JSON.
  const [, , json] = snapshots[file].split(/^---$/m);
  return JSON.parse(json);
}

let app: Record<string, unknown> | null = null;

function draw(v: View): HTMLElement {
  const target = document.createElement("div");
  document.body.append(target);
  app = mount(Review, { target, props: { view: v } });
  flushSync();
  return target;
}

/** The button whose accessible name is `name`. */
function button(root: HTMLElement, name: string): HTMLButtonElement | undefined {
  return [...root.querySelectorAll("button")].find(
    (b) => (b.getAttribute("aria-label") ?? b.textContent?.trim()) === name,
  );
}

beforeAll(() => {
  window.matchMedia = ((q: string) => ({
    matches: false,
    media: q,
    addEventListener() {},
    removeEventListener() {},
  })) as unknown as typeof window.matchMedia;
  Element.prototype.scrollIntoView = () => {};
  Element.prototype.scrollTo = () => {};
});

afterEach(() => {
  if (app) unmount(app);
  app = null;
  document.body.innerHTML = "";
  invoke.mockClear();
  ui.buttons = true;
});

describe("the review's buttons", () => {
  it("put the unit's decision on its preview, the listing over the list", async () => {
    const root = draw(view("queue"));
    const preview = root.querySelector(".preview")!;
    for (const name of ["Accept", "Flag", "Note", "Open", "Undo"]) {
      expect(button(preview as HTMLElement, name), name).toBeDefined();
    }
    const list = root.querySelector(".list") as HTMLElement;
    expect(button(list, "Units")?.getAttribute("aria-pressed")).toBe("true");
    expect(button(list, "Find a unit")).toBeDefined();
    expect(button(root.querySelector("header")!, "Finish")).toBeDefined();
    // A row's accept selects the row, then presses `a`.
    const quick = list.querySelector(".rowwrap .quick button") as HTMLButtonElement;
    quick.click();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledTimes(2));
    expect(invoke.mock.calls[0]).toEqual(["click", { click: { kind: "Row", pos: expect.any(Number) } }]);
    expect(invoke.mock.calls[1]).toEqual(["action", { id: "decide.accept", count: null }]);
  });

  it("put the closer looks over the unit view and zoom and close on a tile", () => {
    const root = draw(view("unit"));
    const nav = root.querySelector(".unitnav") as HTMLElement;
    expect(button(nav, "Back")).toBeDefined();
    expect(button(nav, "Next pending")).toBeDefined();
    // The tests tile is open: its button shows it.
    expect(button(nav, "Tests")?.classList.contains("on")).toBe(true);
    const tile = root.querySelector(".tile") as HTMLElement;
    button(tile, "close the tile")!.click();
    expect(invoke).toHaveBeenCalledWith("click", { click: { kind: "CloseTile", id: expect.any(String) } });
    // The decision is on the card, the hunks on the code.
    expect(button(root.querySelector(".text") as HTMLElement, "Accept")).toBeDefined();
    expect(root.querySelector(".code .tools button")).not.toBeNull();
  });

  it("go, hidden, and leave the counts as words; shown again, come back", () => {
    ui.buttons = false;
    const root = draw(view("unit"));
    expect(root.querySelector(".act")).toBeNull();
    expect(root.querySelector(".unitnav")).toBeNull();
    expect(root.querySelector(".unit-actions")).toBeNull();
    expect(root.querySelector("header .find")).toBeNull();
    expect(root.querySelector("footer")!.textContent).toMatch(/\d+ flagged/);
    // View › Show Buttons tells the window.
    ui.buttons = true;
    flushSync();
    expect(button(root.querySelector(".unitnav") as HTMLElement, "Back")).toBeDefined();
  });

  it("draw the sheet over the queue", () => {
    const root = draw(view("finish"));
    expect(root.querySelector(".modal.Finish")).not.toBeNull();
  });

  it("put a proposed change under the card, each hunk with its button", async () => {
    const hunk = (place: string, state: HunkState): HunkView => ({
      place,
      why: "cast before it is used",
      parses: true,
      state,
      code: {
        added: 1,
        removed: 1,
        rows: [
          { kind: "Removed", line: null, old: 5, pieces: [{ text: "l5", class: null }] },
          { kind: "Added", line: 5, old: null, pieces: [{ text: "L5", class: null }] },
        ],
        notes: [],
        hunks: [0],
      },
    });
    const v = view("unit");
    v.unit!.proposal = {
      answers: "the id reaches the query unbound",
      severity: "high",
      label: "agent (opus)",
      summary: "Casts the id before the query.",
      second: { agree: false, why: "it also drops the log line" },
      dropped: 0,
      hunks: [hunk("a.php:4-6", "proposed"), hunk("a.php:11-13", "proposed"), hunk("b.php:9", "applied")],
      current: 1,
      waiting: null,
      suggest: false,
    };
    const root = draw(v);
    const p = root.querySelector(".text .proposal") as HTMLElement;
    expect(p.textContent).toContain("▲ the id reaches the query unbound (high)");
    expect(p.textContent).toContain("⚠ second model disputes: it also drops the log line");
    const boxes = [...p.querySelectorAll<HTMLElement>(".code")];
    expect(boxes.map((b) => b.querySelector(".head span")!.textContent)).toEqual([
      "a.php:4-6",
      "a.php:11-13",
      "b.php:9",
    ]);
    expect(boxes.map((b) => b.classList.contains("current"))).toEqual([false, true, false]);
    // Put in already: no button.
    expect(boxes[2].textContent).toContain("put in");
    expect(button(boxes[2], "Put in")).toBeUndefined();
    // "Put in" picks its hunk, then presses `p`.
    button(boxes[0], "Put in")!.click();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledTimes(2));
    expect(invoke.mock.calls[0]).toEqual(["click", { click: { kind: "Fix", i: 0 } }]);
    expect(invoke.mock.calls[1]).toEqual(["action", { id: "fix.apply", count: null }]);
    button(p, "Put all in")!.click();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledTimes(3));
    expect(invoke.mock.calls[2]).toEqual(["action", { id: "fix.apply_all", count: null }]);
  });
});
