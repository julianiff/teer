import { describe, expect, it } from "vitest";
import { ago } from "./when";

describe("ago", () => {
  const now = new Date("2026-11-04T12:00:00Z");
  it("says how long since as teer pr does", () => {
    expect(ago("2026-11-04T11:59:30Z", now)).toBe("now");
    expect(ago("2026-11-04T11:55:00Z", now)).toBe("5m");
    expect(ago("2026-11-04T09:00:00Z", now)).toBe("3h");
    expect(ago("2026-11-02T12:00:00Z", now)).toBe("2d");
    expect(ago("2026-09-23T12:00:00Z", now)).toBe("6w");
  });
  it("says nothing without a date", () => {
    expect(ago(undefined, now)).toBe("");
    expect(ago("not a date", now)).toBe("");
  });
});
