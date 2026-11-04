import { describe, expect, it } from "vitest";
import { notation, type KeyLike } from "./keys";

const k = (key: string, more: Partial<KeyLike> = {}): KeyLike => ({
  key,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...more,
});

describe("notation", () => {
  it("passes characters through, case and all", () => {
    expect(notation(k("a"))).toBe("a");
    expect(notation(k("G", { shiftKey: true }))).toBe("G");
    expect(notation(k("]"))).toBe("]");
    expect(notation(k(":"))).toBe(":");
  });

  it("names the keys teer names", () => {
    expect(notation(k(" "))).toBe("<space>");
    expect(notation(k("Enter"))).toBe("<cr>");
    expect(notation(k("Escape"))).toBe("<esc>");
    expect(notation(k("Backspace"))).toBe("<bs>");
    expect(notation(k("Tab"))).toBe("<tab>");
    expect(notation(k("Tab", { shiftKey: true }))).toBe("<s-tab>");
    expect(notation(k("<"))).toBe("<lt>");
  });

  it("writes control and option as prefixes", () => {
    expect(notation(k("u", { ctrlKey: true }))).toBe("<c-u>");
    expect(notation(k("R", { ctrlKey: true, shiftKey: true }))).toBe("<c-r>");
    expect(notation(k("å", { altKey: true, code: "KeyA" }))).toBe("<a-a>");
  });

  it("leaves command keys and lone modifiers to the Mac", () => {
    expect(notation(k("c", { metaKey: true }))).toBeNull();
    expect(notation(k("Shift", { shiftKey: true }))).toBeNull();
    expect(notation(k("F5"))).toBeNull();
    expect(notation(k("a", { isComposing: true }))).toBeNull();
  });
});
