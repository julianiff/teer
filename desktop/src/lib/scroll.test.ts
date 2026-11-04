import { describe, expect, it } from "vitest";
import { target } from "./scroll";

const el = { scrollTop: 400, clientHeight: 600, scrollHeight: 3000 };

describe("target", () => {
  it("scrolls by half pages and pages, count times", () => {
    expect(target(el, "scroll.half_down", 1)).toBe(700);
    expect(target(el, "scroll.half_up", 1)).toBe(100);
    expect(target(el, "scroll.page_down", 2)).toBe(1600);
  });

  it("moves by lines and to the ends", () => {
    expect(target(el, "move.down", 3)).toBe(460);
    expect(target(el, "move.first", 1)).toBe(0);
    expect(target(el, "move.last", 1)).toBe(3000);
  });

  it("goes from hunk to hunk", () => {
    const starts = [100, 380, 900, 1500];
    expect(target(el, "move.next_hunk", 1, starts)).toBe(892);
    expect(target(el, "move.next_hunk", 2, starts)).toBe(1492);
    expect(target(el, "move.prev_hunk", 1, starts)).toBe(372);
    expect(target({ ...el, scrollTop: 0 }, "move.prev_hunk", 1, starts)).toBe(0);
  });

  it("leaves other actions alone", () => {
    expect(target(el, "decide.accept", 1)).toBeNull();
  });
});
