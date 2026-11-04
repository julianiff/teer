// What the window asks of teer, and what it hears back.

import { invoke } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { Click, Hint, Listing, Loaded, Recent, View } from "./types";

export const api = {
  loaded: () => invoke<Loaded>("loaded"),
  openReview: (repo: string, opts: { base?: string; pr?: string; noAgent?: boolean } = {}) =>
    invoke<string | null>("open_review", {
      repo,
      base: opts.base ?? null,
      pr: opts.pr ?? null,
      noAgent: opts.noAgent ?? false,
    }),
  keys: (keys: string) => invoke<void>("keys", { keys }),
  action: (id: string, count?: number) => invoke<void>("action", { id, count: count ?? null }),
  click: (click: Click) => invoke<void>("click", { click }),
  paste: (text: string) => invoke<void>("paste", { text }),
  actions: () => invoke<Hint[]>("actions"),
  recent: () => invoke<Recent[]>("recent_repos"),
  listPrs: (repo: string) => invoke<Listing>("list_prs", { repo }),
  newWindow: () => invoke<void>("new_window"),
  openElsewhere: (repo: string, pr?: string) =>
    invoke<void>("open_elsewhere", { repo, pr: pr ?? null }),
};

/** Listen to `event` sent to this window only. */
export function on<T>(event: string, f: (payload: T) => void): Promise<UnlistenFn> {
  return getCurrentWebviewWindow().listen<T>(event, (e) => f(e.payload));
}

export type { View };
