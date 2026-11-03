# teer v2: implementation workflow for Claude Code

2026-11-02 · @jul

## How to hand this off

Export this doc as Markdown into the teer repo as `docs/v2-plan.md`, then start Claude Code in the repo root with the prompt below. It works one phase at a time and stops for your review after Phase 0 and after each phase that changes what users see.

The target flow is drawn in the [teer v2 review flow](https://claude.ai/artifact/A4W4K6R8vA51okdk7XTYZF) page. This doc is the source of truth wherever the two differ.

```markdown
Read docs/v2-plan.md fully before doing anything. It describes how to rework teer's
review flow and keymap into v2, in phases 0 to 8.

Work strictly one phase at a time:
1. Start with Phase 0. Explore the code, then write docs/v2-map.md and stop.
   Do not change any source files in Phase 0.
2. After I approve, do the next phase on its own branch (v2/phase-N-<name>).
3. For each phase: write or update tests first, implement, run the full test suite
   and the linters, then summarize what changed, what you could not do, and any
   decision you had to make that the plan did not cover.
4. Stop after every phase marked "Stop for review" and wait for me.
5. If the plan conflicts with how the code actually works, do not improvise a big
   redesign. Write the conflict and two options into docs/v2-map.md and ask me.

Keep teer working at the end of every phase: `teer` must start, review, and quit
cleanly even if later phases are not done yet.
```

## Goal and scope

v2 makes teer learnable in one session: one start command, one queue, one finish key, and Neovim-style keys that mean the same thing on every screen. The prepare pipeline (split, level, explain, check) stays; only its output and the UI around it change.

v2 is built for two flows. Everything else is secondary and must not add keys or screens to these paths.

1. **Review what your agent wrote.** `teer` → `a`/`f` down the queue → `l` on a ▲ → `K` ask → `f` with a note → `:w` sends notes to the agent → teer re-runs on its own → `a` the changed units → `:w` commits.

2. **Review a teammate's pull request.** `teer pr` (or `teer <url>`) → `a`/`f` down the queue → `l`, `K` to ask → `f` note the problem → `:w` requests changes, or approves if nothing is flagged.

Reviewing your own branch before opening a PR is flow 1 with the base detected automatically.

Out of scope for v2: new analysis features, new model providers, changes to how units are split, and any web or GUI front end.

## Ground rules for the agent

- One branch per phase, named `v2/phase-N-<name>`, branched from the previous phase once it's merged.

- Tests first. Every key binding in the keymap table gets at least one test that sends the keys and asserts the resulting action or state. Every finish case gets a test.

- Keep teer shippable after every phase. Old screens can stay behind until Phase 8 removes them, but the default path must always work.

- No hard-coded keys outside the keymap module once Phase 1 lands. A key string anywhere else is a bug.

- Don't change the split, level or explain logic except where Phase 3 says so.

- Don't post to GitHub, commit, or call a model in tests. Fake the git, gh and model layers.

- When the plan is silent or wrong, stop and write the question into `docs/v2-map.md` under "Open questions". Don't guess on anything a user would see.

- Stop for review after Phases 0, 1, 4, 5 and 6.

## Phase 0: map the code (no source changes)

The output is `docs/v2-map.md`, which ties every part of this plan to real files and functions. **Stop for review.**

It must contain:

1. **Entry points.** Where `teer`, `--base`, `pr <url>`, `pr list`, `setup`, `notes` and every flag are parsed.

2. **Screens.** Each screen (list, swipe, history, flagged, all-accepted, notes, commit, approve, ask panel) with its file, its state struct and how it is drawn.

3. **Key handling.** Every place a key is matched today, as a table: key, screen, action, file:line. Mark each collision (same key, different meaning).

4. **Unit state.** How pending, accepted, flagged, needs-fix and notes are stored, persisted and restored between runs.

5. **Prepare pipeline.** Where split, level, explain and check run, and where the level and the `$ ⚠ ≠` marks are decided.

6. **Agent integration.** How notes are handed to Claude Code or Codex, how `--session` is used, and how re-runs and "only changed units come back" work.

7. **Config.** The `config.toml` schema, especially `[keys]`.

8. **Tests.** What exists, how to run it, and how the UI is tested (snapshot, key replay, none).

9. **Phase-by-phase notes.** For each later phase, the files it will touch and any risk or conflict with this plan.

10. **Open questions.**

## Phase 1: keymap and action layer

All input goes through one keymap module that turns key sequences into named actions; screens only handle actions. This phase builds the layer and binds it to the existing screens, so behavior barely changes yet. **Stop for review.**

**Parser.** It must support:

- Single keys, and sequences with a prefix: `g` (`gg`, `gd`, `gr`, `gf`), `]` and `[` (`]c`, `[c`), `z` (`za`), and the leader `<space>`. A pending prefix waits up to 1000 ms (configurable) and shows a small hint line listing what can follow.

- Counts before an action: `3a`, `5j`. A count with no action is dropped on `esc`.

- A command line on `:` that accepts `w`, `e`, `e!` and `q`. Unknown commands show an error in the bottom bar and change nothing.

- Dot repeat: `.` replays the last decision action (accept, flag, file accept, file flag) with its count. Notes are not replayed.

- Visual mode: `V` starts a line selection in the queue; `j`/`k` extend it; `a` or `f` applies to every unit in it; `esc` leaves it.

- Undo and redo of decisions: `u` and `ctrl-r`, as a stack per review, persisted with the review.

**Rules the keymap must enforce** (add a test that loads the default keymap and checks them):

1. A key has one action across all screens. A screen may leave a key unbound, never rebind it.

2. For decisions, lowercase acts on the unit and uppercase on the whole file (a/A, f/F). Other uppercase keys follow Neovim (G, N, K, V).

3. `g` sequences move you somewhere, `<space>` sequences open something, `:` commands finish or restart.

4. `i` only ever starts typing text (a note or a commit message).

**Default bindings.** Action ids are what `config.toml` rebinds.

| Key | Action id | Where | Does |
| --- | --- | --- | --- |
| `j` `k` | `move.down` `move.up` | queue, unit, sheets | move the cursor |
| `gg` `G` | `move.first` `move.last` | queue | first or last unit |
| `n` `N` | `move.next_pending` `move.prev_pending` | queue, unit | next or previous pending unit; in the unit view, stays in the unit view |
| `]c` `[c` | `move.next_hunk` `move.prev_hunk` | unit | next or previous hunk |
| `ctrl-d` `ctrl-u` | `scroll.half_down` `scroll.half_up` | queue preview, unit | scroll half a page |
| `/` | `find.unit` | everywhere | fuzzy find a unit by file or name |
| `a` | `decide.accept` | queue, unit | accept the unit, then advance |
| `f` | `decide.flag` | queue, unit | flag the unit and open the note line, then advance |
| `i` | `note.edit` | queue, unit, finish | edit the note on this unit; in finish, edit the commit message |
| `A` `F` | `decide.accept_file` `decide.flag_file` | queue, unit | accept or flag every pending unit in the file |
| `V` | `visual.start` | queue | start a selection |
| `u` `ctrl-r` | `history.undo` `history.redo` | queue, unit | undo or redo the last decision |
| `.` | `history.repeat` | queue, unit | repeat the last decision |
| `l` `enter` | `view.open` | queue | open the unit view |
| `h` `esc` | `view.back` | unit, panels | back one level |
| `K` | `ask.open` | queue, unit | ask about the unit under the cursor |
| `gd` `gr` | `goto.definition` `goto.callers` | queue, unit | jump to the definition, list callers |
| `gf` | `goto.editor` | queue, unit | open `$EDITOR` at the unit's first changed line |
| `za` | `toggle.auto_accepted` | queue | show or hide units accepted for you |
| `<space>t` | `tool.tests` | queue, unit | tests that cover this unit |
| `<space>r` | `tool.run_old_new` | queue, unit | run old and new side by side in the sandbox |
| `<space>v` | `tool.diagram` | queue, unit | diagram of the change |
| `<space>c` | `tool.history` | queue, unit | commits and PR description |
| `<space>n` | `tool.notes` | queue, unit | every note in one list |
| `<space>s` | `tool.settings` | queue, unit | settings |
| `:w` | `finish.open` | everywhere | open the finish sheet |
| `y` | `finish.yank` | finish | copy notes as markdown |
| `:e` | `review.rerun` | everywhere | re-run now, decisions kept |
| `:e!` | `review.reset` | everywhere | re-run and reset every decision (asks to confirm) |
| `q` | `view.close` | everywhere | close the current panel; in the queue, quit (state saved) |
| `?` | `help.open` | everywhere | every key for the current screen |

## Phase 2: one start command

`teer` works out what to review, so the user never chooses a mode. `teer <url>` and `teer pr` cover pull requests.

**Detection for plain `teer`, in this order:**

1. Working tree has staged, unstaged or untracked changes → review those (today's `teer`).

2. Clean tree, current branch is not the default branch → review the branch since its merge base with the base branch.

3. Clean tree on the default branch → print `Nothing to review: no changes, and you're on <branch>.` and exit with code 0.

**Base branch.** Use, in order: `--base <ref>` if given; the branch's upstream PR base if `gh` knows it; `origin/HEAD`; then `main` or `master`, whichever exists. Show the chosen base in the queue header ("since main, 6 commits").

**Pull requests.**

- `teer <url>` with a GitHub PR URL behaves like today's `teer pr <url>`. Also accept a bare PR number (`teer 482`) when the repo has a GitHub remote.

- `teer pr` opens today's PR picker. Keep the separate checkout so the user's tree is untouched.

**First run.** If no config exists, run today's `teer setup` inline before the review starts, then continue straight into the review. Keep `teer setup` as a hidden command for re-running it.

**Agent session.** Replace `--session <id>` with discovery: find the most recent Claude Code session whose working directory is this repo and that touched any of the changed files in the last 24 hours. Use it for `K` if found; otherwise use the configured model. Keep `--session` as a hidden flag for overrides.

**Flags after this phase:** `--base <ref>`, `--no-agent`, `--json`. Keep `--swipe` working until Phase 8, but hide it from help.

Tests: one per detection case, the base fallback chain, URL and number parsing, and first-run setup followed by a review.

## Phase 3: prepare pipeline output

The pipeline keeps its four steps. Two things change: units with no behavior change are accepted automatically, and the marks become written reasons.

**Levels.** Keep the four internal levels, but the UI shows three: ▲ look closely, ◆ worth a look, ● routine. ○ units (formatting, comments, renames) are set to `accepted` with `by: auto` and hidden from the queue by default.

- Auto-accept only when the level is ○ **and** the second model did not dispute it. A disputed ○ goes up to ● like today.

- Add a config key `auto_accept = "none" | "no_behavior_change"`, default `no_behavior_change`.

- Auto-accepted units count as accepted for finishing. `u` on one reverts it to pending.

**Reasons instead of marks.** Replace the `$ ⚠ ≠` glyphs with a `reasons` list on each unit, each a short sentence shown in the unit view under "Why this level":

| v1 mark | Reason text (example) |
| --- | --- |
| `$` | Expensive to undo: changes the `orders` table schema. |
| `⚠` | The second model disputes this: "the retry wasn't removed, it moved to `client.ts`". |
| `≠` | Not mentioned in the commits or PR description. |

Keep the ⚠ glyph in the queue row only, since a dispute is worth seeing before opening a unit. Remove `$` and `≠` from the queue.

**Queue order.** Sort by level (▲, ◆, ●), then callers before callees inside a file, then by file path. The first unit shown is the first pending ▲.

`--json` output gains `reasons` and `accepted_by`; keep the old fields for one release.

Tests: auto-accept on and off, a disputed ○, reason generation for each mark, and sort order.

## Phase 4: the queue

The queue replaces the list, swipe and history screens as the screen teer opens on. Build it next to the old list, make it the default at the end of this phase, and leave the old screens reachable only through `--swipe` until Phase 8. **Stop for review.**

**Layout.**

- Header: what is being reviewed ("your changes", "feature/refunds since main, 6 commits", "PR #482 by ana"), plus one line of the change summary. `<space>c` opens the full history.

- Rows grouped by level, then file. Each row: level glyph, file path, unit name, state (pending, accepted, flagged + the first 40 characters of the note), and ⚠ when disputed.

- One line at the bottom of the list when units were auto-accepted: "14 units accepted for you, za shows them".

- Preview pane (right, or below on narrow terminals): why this level, the summary, then the diff with each claim under its lines. Same content as today's preview.

- Bottom bar: `4 pending · 1 flagged · 37 accepted`, and the pending key prefix or mode (`-- VISUAL --`, `g…`, `:`).

**Behavior.**

- **Auto-advance.** After `a` or `f`, move to the next pending unit in queue order, not simply the next row. When none are left, stay put and show "Nothing pending. :w to finish" in the bottom bar.

- **Flag with note.** `f` sets the state to flagged and opens a one-line note input at the bottom, prefilled with the existing note if there is one. `enter` saves (an empty note is allowed); `esc` keeps the flag and discards the edit. The old `needs fix` state is gone: a flag with a note is what goes to the agent.

- **File actions.** `A` and `F` apply to pending units in the current file only, never to ones already decided. `F` opens one note for the whole file, copied to each unit.

- **Visual mode** as in Phase 1. A note typed after `f` in visual mode goes on every selected unit.

- **Counts.** `3a` accepts the current unit and the next two pending units.

- **Mouse.** Keep click to select and drag to copy; no new mouse features.

**Persistence.** Decisions, notes, the undo stack and the cursor position save on every change, so `q` and a crash both resume at the same place.

Tests: auto-advance through mixed states, `f` with empty and non-empty notes, `esc` in the note line, `A`/`F` skipping decided units, visual mode with notes, counts, undo and redo across file actions, and resume after quit.

## Phase 5: the unit view

The unit view replaces the flagged screen and works on any unit, not just flagged ones. It opens with the answers already there, so the user reads before choosing anything. **Stop for review.**

**Content, top to bottom:**

1. Title row: level, file, unit name, state.

2. **Why this level:** the level rule that fired plus the `reasons` from Phase 3.

3. **What changed:** the summary and diff, each claim under the lines it cites. `]c` and `[c` jump between hunks.

4. **What to do about it:** today's recommendation (`n` on the flagged screen), generated when the unit opens.

5. **Second model:** today's disagreement tile (`x`), shown only when the unit is disputed.

**Prefetching.** Start generating the recommendation for the first three pending ▲ and ◆ units as soon as the queue opens, and for the next pending unit whenever one is opened. Cache results per unit and content hash. With `--no-agent`, sections 4 and 5 are hidden.

**Ask (`K`).** Opens a panel under the diff with an input line, focused. Questions go to the session found in Phase 2, or the configured model. Keep today's streaming, step view and "related units" links; `tab` moves between links and `enter` jumps to one. Follow-up questions stay in the same panel. `h`/`esc` closes it, and the conversation is kept per unit.

**Go-to.** `gd` jumps to the unit that defines the symbol under the cursor if it's in the review; otherwise it shows the definition read-only. `gr` lists callers that are in the review. `gf` opens `$EDITOR` at the first changed line.

**Leader tools.** `<space>t` tests, `<space>r` run old vs new in the sandbox, `<space>v` diagram, and `gx` inside the diagram opens it in the browser (add it to the keymap as `goto.browser`, like Neovim's open-URL key). These are today's `c`, `t` and `v` tiles, moved behind the leader and shown as panels under the diff, one at a time. `<space>` alone shows a hint listing them.

**Deciding here.** `a`, `f`, `A`, `F`, `u` and `.` work as in the queue. After `a` or `f`, the view moves to the next pending unit and stays in the unit view. `n`/`N` do the same without deciding.

Tests: prefetch and cache hits, `--no-agent` hiding, the ask panel lifecycle, go-to inside and outside the review, leader panels, and deciding then advancing in the unit view.

## Phase 6: :w finish

`:w` opens one sheet that reads the decisions and offers exactly one default next step; `enter` does it. It replaces the all-accepted popup, the notes screen and the separate commit and approve screens. **Stop for review.**

**What the sheet offers:**

| Reviewing | State | Default step (`enter`) | Alternatives (`j`/`k` to pick) |
| --- | --- | --- | --- |
| your changes or branch | something flagged | send the notes to your agent | yank notes, commit anyway |
| your changes or branch | all accepted | commit, agent drafts the message | yank notes |
| a pull request | something flagged | request changes, notes inline + summary | comment only, yank notes |
| a pull request | all accepted | approve | approve with comment, yank notes |

If units are still pending, the sheet says so first ("4 units pending"), and `n` jumps to the first one. The default step then becomes "keep reviewing". The other steps remain available but ask to confirm.

**Inside the sheet:**

- The sheet lists the flagged units with their notes, so it doubles as the notes overview.

- `y` copies the notes as markdown, each with its diff (today's `N` `y`).

- `i` edits the commit message when committing, or the review summary when posting to a PR.

- `esc` or `q` closes the sheet without doing anything.

**Keep today's safety checks unchanged:** commit only the files reviewed, with the user's hooks and signing, and refuse if one changed since the review; check the PR head is still the one reviewed; post with `gh` only after `enter`; open the PR page if posting fails.

**After the step:** a commit or a successful post shows a one-line result and exits. Sending notes to the agent hands off to Phase 7.

Tests: each of the four rows, the pending warning, every alternative, `y` output format, `i` for both message types, and each safety check refusing.

## Phase 7: automatic fix loop

After notes go to the agent, teer waits, re-runs on its own when the agent is done, and puts only the changed units back in the queue. The user never presses `r`.

1. **Hand-off.** Send the notes to Claude Code or Codex the way `N` `c` does today, into the session found in Phase 2 if there is one.

2. **Waiting screen.** The queue stays visible and read-only, with a status line: "Claude Code is working on 3 notes… (q to stop waiting)". `q` returns to the normal queue, and the user can re-run later with `:e`.

3. **Knowing the agent is done.** Prefer the agent's own completion signal (process exit, or the session reporting the turn finished). If there is none, watch the working tree and re-run once files have been quiet for 5 seconds (configurable as `fix_loop.quiet_seconds`).

4. **Re-run.** Run prepare on the new tree. Units whose content hash changed become pending again, keep their old note marked "code changed since this note", and go to the top of their level. Unchanged units keep their decision.

5. **Back in the queue.** The cursor lands on the first changed unit, and the bottom bar shows "3 units changed by Claude Code".

**Notes on changed code.** A note on a changed unit stays until the user acts on it: `a` accepts and archives the note, `f` keeps the flag and lets them edit it, `i` edits it without changing the decision.

**:e and :e!** `:e` runs step 4 now. `:e!` asks "Reset all 42 decisions? y/n" and then re-runs with every unit pending, including auto-accepted ones re-evaluated.

Tests: completion via signal and via quiet period, `q` while waiting, changed versus unchanged units after re-run, note handling on changed units, and both `:e` variants.

## Phase 8: remove v1, migrate, document

This phase deletes what v2 replaced and makes sure existing users land safely.

**Remove:** the list, swipe, history, flagged, all-accepted and notes screens; the `needs fix` state; `--swipe`; the visible `teer --base main` and `teer pr list` forms (keep them as hidden aliases for one release that print a one-line hint about the new form); the `$` and `≠` glyphs.

**Migrate saved reviews.** On load, convert v1 review state: `needs fix` becomes flagged (note kept), flagged without a note stays flagged, and the swipe position becomes the queue cursor. Test with a saved v1 review fixture.

**Migrate config.** Read v1 `[keys]` entries. Map each old action to its v2 action id where one exists. For actions that no longer exist, print one warning per key on first start ("`[keys].swipe` is no longer used") and continue. Write nothing back without asking.

**Help.** `?` shows the keys for the current screen, grouped as Move, Decide, Look closer, More, Finish, generated from the keymap so rebinding is reflected. `teer --help` lists only `teer`, `teer <url>`, `teer pr`, `teer notes` and the three flags.

**Docs.** Update the README and man page to the two flows and the keymap. Add a short "Coming from v1" section with the old-to-new key table from the v2 flow page.

## Acceptance checklist

v2 is done when each scenario below passes as an automated end-to-end test, using recorded key input against a fixture repo with fake git, gh and model layers, and once by hand on a real repo.

- **Agent changes, nothing wrong.** Dirty tree → `teer` → only `a` presses (or `5a`) → `:w` → `enter` commits with the drafted message. No other keys needed.

- **Agent changes, one fix.** `teer` → `f` with a note on one ▲ → `:w` → `enter` → the agent edits the file → teer re-runs without input → the cursor is on that unit with the note marked as changed → `a` → `:w` → `enter` commits.

- **Branch before a PR.** Clean tree on a feature branch → `teer` reviews since the detected base, and the header names the base.

- **PR, approve.** `teer 482` → accept all → `:w` → `enter` approves after the head check.

- **PR, request changes.** `teer pr` → pick → `f` on two units with notes → `:w` → `enter` posts inline comments and a summary.

- **Look closer.** `l` on a ▲ shows the recommendation without any key press; `K` streams an answer; `gd` jumps to a definition in the review; `h` returns to the same row.

- **Bulk and undo.** `V` `j` `j` `f` flags three units with one note; `u` undoes all three; `ctrl-r` redoes them; `.` repeats on the next unit.

- **Pending warning.** `:w` with pending units shows the count, and `n` jumps to the first one.

- **Resume.** `q` in the middle of a review, then `teer` again, lands on the same unit with all decisions and notes.

- **Keymap rules.** The keymap rules test from Phase 1 passes, and no key literal exists outside the keymap module.

- **v1 user.** A v1 config and saved review load with warnings only for removed actions, and the saved review continues in the queue.

- **No agent.** `teer --no-agent` runs the agent-changes scenario with no model calls and no empty sections in the unit view.
