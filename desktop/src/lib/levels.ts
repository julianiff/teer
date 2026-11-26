// A level as a mark: a vertical line in the level's colour (`lvl-*` in
// app.css: red, yellow, grey), and what it stands for.

import type { Level } from "./types";

export const LEVELS: Record<Level, { icon: string; title: string }> = {
  Attention: { icon: "┃", title: "Needs a close look" },
  Look: { icon: "┃", title: "Worth a look" },
  Routine: { icon: "┃", title: "Routine" },
  Same: { icon: "○", title: "No behavior change" },
};
