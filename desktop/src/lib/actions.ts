// The actions a button presses, as teer's view says them: a review puts
// them in context, and each button takes its key, words and state from
// there, so a rebound key shows rebound.

import { getContext, setContext } from "svelte";
import type { ActionView } from "./types";

const KEY = Symbol("actions");

export type Actions = () => Record<string, ActionView>;

export const provideActions = (get: Actions) => setContext(KEY, get);

export const useActions = (): Actions => getContext<Actions>(KEY) ?? (() => ({}));

/** A button's tooltip: what it does and its key, `Accept the unit (a)`. */
export function tip(a: ActionView, what = a.what): string {
  const said = what.charAt(0).toUpperCase() + what.slice(1);
  const key = a.keys ? ` (${a.keys})` : "";
  const off = a.off ? " · needs an agent: Settings (⌘,) sets one up" : "";
  return said + key + off;
}
