// A key pressed in the window, in the notation teer's keymap reads
// (Neovim's: `a`, `G`, `<space>`, `<cr>`, `<c-u>`, `<s-tab>`). teer
// resolves it, so `[keys]` rebinding applies here too. ⌘ combinations are
// the menu's and never reach teer.

const NAMED: Record<string, string> = {
  Enter: "cr",
  Escape: "esc",
  Backspace: "bs",
  Tab: "tab",
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
  Home: "home",
  End: "end",
  PageUp: "pageup",
  PageDown: "pagedown",
  " ": "space",
};

export interface KeyLike {
  key: string;
  code?: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  isComposing?: boolean;
}

/** The notation of `e`; `null` for what teer doesn't take (⌘, modifiers alone). */
export function notation(e: KeyLike): string | null {
  if (e.metaKey || e.isComposing) return null;
  const mod = e.ctrlKey ? "c-" : e.altKey ? "a-" : "";
  let name = NAMED[e.key];
  if (e.key === "Tab" && e.shiftKey) name = "s-tab";
  if (name) {
    if (name === "s-tab") return "<s-tab>";
    return `<${mod}${name}>`;
  }
  if ([...e.key].length !== 1) return null;
  if (mod) {
    // With ⌥ on a Mac the key is what it types (å): the key's own letter.
    const letter = e.altKey && e.code?.startsWith("Key") ? e.code.slice(3).toLowerCase() : e.key.toLowerCase();
    return `<${mod}${letter}>`;
  }
  if (e.key === "<") return "<lt>";
  return e.key;
}
