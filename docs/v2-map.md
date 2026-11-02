# teer v2: code map (Phase 0)

Mapped on 2026-11-02 against `main` at `e394565`. No source file was changed. Read it next to `docs/v2-plan.md`: section numbers 1–10 follow Phase 0's list, section 11 is the test matrix you asked for, to be written once Phase 8 is done.

All paths are under `src/` unless they start with `tests/` or `docs/`. Line numbers are from `e394565`.

## 1. Entry points

`main.rs` parses `cli::Args` (clap derive) and calls `cli::run` (`cli.rs:313`).

| Form | Parsed at | Runs | Notes |
| --- | --- | --- | --- |
| `teer` | `cli.rs:31` (no subcommand) | `cli.rs:367` → `review::build_review` → `cli::restore` (`cli.rs:259`) → `run_review` (`cli.rs:518`) | Always the working tree vs HEAD, untracked files included (`git/mod.rs:129-150`, `:193-202`). A clean tree prints `teer: no uncommitted changes to review in <repo>` (`cli.rs:564-578`). There is no branch detection. |
| `--base <REV>` | `cli.rs:36` | `git::resolve` → merge base of REV and HEAD | No default. Refused with `pr` and `pr list` (`cli.rs:380`, `:393`). |
| `pr <url>` | `Command::Pr`, `cli.rs:79` | `review_pr` (`cli.rs:428`) → `pr::prepare` (`pr.rs:1058`) | `pr::parse_url` (`pr.rs:79-157`) rejects bare numbers (tests at `pr.rs:1233-1234`). |
| `pr list` / `pr ls` | `PrAction::List`, `cli.rs:111` | `pr_list::run` (`cli.rs:383`) → picker `tui/pr_pick.rs:305` | Piped or with `--json`, it prints instead. |
| `setup` | `Command::Setup`, `cli.rs:107` | `tui::setup::run` (`cli.rs:325-335`) | Also runs on first start when `provider` is unset and stdin is a terminal (`cli.rs:338-353`). |
| `notes [URL]` | `Command::Notes`, `cli.rs:95` | `print_notes` (`cli.rs:405`) → `notes::markdown` | Reads the latest saved review of the repo or the PR. |
| `--session <ID>` | `cli.rs:39-43` (global) | `session_by_id` (`cli.rs:245`) | Claude Code only (`cli.rs:354`). For a PR, cwd is forced to the checkout (`tui/mod.rs:97-100`). |
| `--swipe` | `cli.rs:46` | `tui::Launch.swipe` → `App::start` (`tui/app.rs:710`) | |
| `--json` | `cli.rs:50` | `cli.rs:537-561` | Prepares every unit (no second opinion), saves, prints the serde `Review`. |
| `--no-agent` | `cli.rs:54` | `cli.rs:527` | The agent becomes `Err(NO_AGENT)`. |
| `-C <DIR>`, `-V` | `cli.rs:58`, `:62` | | |

The help text with examples is `after_help` at `cli.rs:28`. Signals: `watch_signals` (`cli.rs:471`) saves the review and removes a throwaway PR checkout.

## 2. Screens

Dispatch: `ui::draw` (`tui/ui.rs:21`) draws the header (`:170`), the screen for `app.screen`, the footer (`:293`, buttons from `buttons_for` `:214`), then the one open overlay on top. The loop is `tui::run` (`tui/mod.rs:77`, loop `:138-219`). Effects are data: `App` returns `Vec<Effect>` (`tui/app.rs:246`) and `Runtime::exec` (`tui/mod.rs:277`) runs them, except `Edit`, `OpenAgent`, `EditComment` and `Commit`, which need the terminal and run inline in the loop (`:155-214`).

| Screen | Kind | State | Drawn by | Opened by |
| --- | --- | --- | --- | --- |
| List | `Screen::Slim` (`app.rs:32`) | fields on `App`: `order`, `sel`, `grouping` (`app.rs:41`), `levels`, `depth` | `ui::draw_slim` `ui.rs:348`, rows `slim::build_with` `slim.rs:791`, preview `preview::draw_preview` `preview.rs:586` (beside at ≥110 cols, else below: `layout_of` `:129`) | default screen |
| Swipe | `Screen::Swipe` | `SwipeState` `app.rs:361` | `ui::draw_swipe` `ui.rs:442`, summary `:665` | `s`, `--swipe` |
| Flagged | `Screen::Flagged` | `FlaggedState{sel,pane,zoom,typing}` `app.rs:374`, tiles `Pane` `app.rs:383` | `flagged::draw_flagged` `flagged.rs:1084` | `d` → `start_flagged` `app.rs:1205` |
| All reviewed | `Overlay::AllDone` `app.rs:82` | — | `ui::draw_all_done` `ui.rs:750` | `check_all_done` `app.rs:920` |
| Notes | `Overlay::Notes{sel}` `app.rs:65` | — | `notes::draw_view` `notes.rs:196` | `N` → `open_notes` `app.rs:2025` |
| Note editor | `Overlay::Note{id,text,back}` `app.rs:58` | — | `notes::draw_editor` `notes.rs:105` | `m` → `edit_note` `app.rs:2015` |
| Commit | `Overlay::Commit{..}` `app.rs:100` | `DraftState` `app.rs:197` | `ui::draw_commit` `ui.rs:1171` | `c` in AllDone → `ask_commit` `app.rs:1684` |
| Post review | `Overlay::PostReview{..}` `app.rs:85` | — | `ui::draw_post_review` `ui.rs:1003` | `a` in AllDone, `p` in Notes → `ask_post` `app.rs:1540` |
| History | `Overlay::Commits{sel,unit}` `app.rs:115` | — | `commits::draw_commits` `commits.rs:210` | `C` → `open_commits` `app.rs:2205` |
| Ask | `Overlay::Ask` `app.rs:121` | `AskState` `app.rs:129` | `ask::draw_ask` `tui/ask.rs:34` | `/` → `open_ask` `app.rs:2283` |
| Help | `Overlay::Help` | — | `ui::draw_help` `ui.rs:831` | `?` |
| Confirms | `ConfirmReset`, `ConfirmWeb`, `ConfirmAcceptFile`, `ConfirmDeleteNote` | — | `ui.rs:712`, `ui.rs:55`, `ui.rs:61-73`, `ui.rs:50-54` (`draw_confirm` `ui.rs:988`) | `R`, first web question, `A`, `d` in Notes |
| PR picker | own loop | `Picker` `pr_pick.rs:36` | `pr_pick::draw` `:254` | `teer pr list` |
| Setup | own loop | `Setup` `tui/setup.rs:82`, `Step` `:33` | `setup::draw` `:429` | `teer setup`, first start |

Flagged tiles: Code, then answers in the order asked (`tile_kinds` `app.rs:229`), then Chat, then "more light" (`flagged_panes` `app.rs:1236`; menu `flagged.rs:74`). Answers: Recommend `n`, Visualize `v` (`b` opens it in the browser), Tests `c` (teer's search) and `a` (agent interprets), Behavior `t`, Disagreement `x`, Chat `/`. `o` and `f` set the resolution (ok, needs fix).

Things to know before moving screens around:
- Only one overlay is open at a time. `ConfirmDeleteNote` redraws Notes under itself, and the note editor returns to Notes through `back`.
- `return_to` is a stack of screens, but `enter_swipe` clears it (`app.rs:2843-2846`).
- The Ask overlay and the Flagged Chat tile share `talk()` (`app.rs:2296`) and `ask_links()` (`:2318`).
- Mouse targets are registered while drawing (`Target`, `app.rs:403`). `Target::Key` turns a clicked hint label back into a key with `ui::key_of` (`ui.rs:135`).

## 3. Key handling

**Mechanism.** `config.rs` holds `enum Action` (`:389`), the table `Action::ALL` of (action, config name, default key) (`:457-521`), `parse_key` (`:524`) and `KeyMap` (`:555`), which holds exactly one key per action. In `app.rs`, keys go through:

1. `on_key` (`:936`): `ctrl+c` quits first. Then each overlay's own handler runs (`:944-1014`), then chat typing (`:1015`), then `PageUp`/`PageDown` (`:1019-1022`).
2. `action_for` (`:1035`), or `flagged_action_for` (`:1112`) on the Flagged screen. These hard-code arrows and `Home`/`End` (`:1041-1046`), `Tab`/`BackTab` (`:1116-1117`) and `h` (`:1104`).
3. `on_action` (`:1155`) handles the actions that work everywhere, then hands the rest to `slim_action` (`:2175`), `swipe_action` (`:2905`) or `flagged_action` (`:1299`).

Overlays, text inputs, the setup screen and the PR picker match `KeyCode` literals directly: about 98 matches in `app.rs`, 20 in `tui/setup.rs`, 8 in `pr_pick.rs` and 7 in `ui.rs`.

**Every key today.** "Configurable" means it's in `Action::ALL`; "literal" means it's hard-coded at that line. The last column is what v2's table gives the key.

| Key | Screen | Does today | Where | v2 |
| --- | --- | --- | --- | --- |
| `q` | all screens; AllDone; Notes; History | quit (save); close | `quit` config; `app.rs:988`, `:2099`, `:2251` | `view.close` |
| `j` `k` / `↓` `↑` | list, Flagged, Notes, History | move | `down`/`up`; arrows literal `app.rs:1041-1042` | same |
| `j` `k` | Ask (link mode) | move between links | literal `app.rs:2382`, `:2385` | `move.*` |
| `J` `K` | all screens | scroll the detail area | `preview_down`/`preview_up` | **`K` = `ask.open`** |
| `g` `G` / `Home` `End` | list, Flagged, Notes, History | top, bottom | `top`/`bottom`; literal `app.rs:1045-1046` | `gg` `G` |
| `enter` | — | `Action::Open` exists but is unused on the list | `open` | `view.open` |
| `enter` | confirms, Notes, Commit, Post, Ask, Chat, History | confirm, go to unit, send | literal (see the overlay handlers) | sheet- and panel-local |
| `l` | list, swipe | accept | `accept` | **`view.open`** |
| `→` | swipe; Flagged; History | accept; next tile; open commit | literal `app.rs:1043`; `next_tile`; `:2249` | none |
| `i` | list, swipe | flag ("investigate") | `investigate` | **`note.edit`** |
| `i` | Post review | toggle inline comments | literal `app.rs:1648` | collides with v2 |
| `h` / `←` | list | flag | literal `app.rs:1104`, `:1044` | **`view.back`** |
| `h` | swipe | previous card | `prev_card` | `view.back` |
| `←` | Flagged; History | previous tile; back | `prev_tile`; `app.rs:2266` | none |
| `d` | list, swipe | open Flagged | `flagged` | none |
| `d` | Notes | delete the note | literal `app.rs:2072` | none |
| `n` | Flagged | Recommend | `dive_recommend` | **`move.next_pending`** |
| `v` | Flagged | Visualize | `dive_visualize` | `<space>v` |
| `v` | Post review | cycle the verdict | literal `app.rs:1649` | none |
| `t` | Flagged | Behavior (run old and new) | `dive_behavior` | `<space>r` |
| `x` | Flagged | Disagreement | `dive_disagree` | section in the unit view |
| `c` | Flagged | Tests (teer's search) | `dive_tests` | `<space>t` |
| `c` | AllDone | commit | `commit` | `:w` |
| `c` | Notes | hand to Claude Code or Codex | literal `app.rs:2088` | `:w` |
| `a` | Flagged | agent interprets the tests | `interpret_tests` | **`decide.accept`** |
| `a` | AllDone | approve the PR | `approve_pr` | `:w` |
| `b` | Flagged | open the picture in the browser | `open_visual` | `gx` |
| `o` | Flagged | resolution ok | `resolve_ok` | gone |
| `f` | Flagged | resolution needs fix | `resolve_fix` | **`decide.flag`** |
| `f` | list | by file / by unit | `grouping` | **`decide.flag`** |
| `m` | all screens; Notes | write a note | `edit_note` | `i` |
| `N` | all screens | Notes view | `notes` | **`move.prev_pending`**; notes under `<space>n` |
| `r` | list, swipe | re-run the review | `rerun` | `:e` |
| `r` | Flagged | ask the focused tile again | `dive_rerun` | none |
| `R` | all screens; AllDone | reset decisions | `reset` | `:e!` |
| `e` | all screens | open the editor | `edit` | `gf` |
| `e` | Commit; Post review | edit the message in `$EDITOR` | literal `app.rs:1788`, `:1643` | `i` |
| `s` | list; swipe | enter swipe; leave it | `swipe` | gone |
| `u` | list, swipe | undo the last decision | `undo` | `history.undo` |
| `]` `[` | list | next, previous pending | `next_pending`/`prev_pending` | prefixes for `]c` `[c` |
| `ctrl+d` `ctrl+u` | list; PR picker | next, previous file | `next_group`/`prev_group` | **`scroll.half_*`** |
| `ctrl+u` | every text input | clear the line | literal `app.rs:179` | collides with v2 |
| `A` | list | accept the file (asks first) | `accept_file` | `decide.accept_file` |
| `C` | list; History | open history; close it | `commits` | `<space>c` |
| `/` | list, swipe; Flagged | Ask; Chat | `ask` | **`find.unit`** |
| `/` | Ask (link mode) | leave link mode | literal `app.rs:2387` | — |
| `z` | Flagged | zoom the tile | `zoom` | prefix for `za` |
| `tab` `shift+tab` | Flagged; Chat; Ask; History | tile focus; link mode; open | literal `app.rs:1116-1117`, `:2579-2580`, `:2387`, `:2396`, `:2249` | link focus in `K` |
| `esc` | everywhere | back, close, cancel | `back`; literal in every overlay | `view.back` |
| `?` | everywhere | help | `help` | `help.open` |
| `y` | confirms; Commit; Post | yes | literal `app.rs:950`, `:970`, `:977`, `:1623`, `:1787`, `ui.rs:994` | `finish.yank` |
| `y` | Notes | copy as Markdown | literal `app.rs:2078` | `finish.yank` |
| `p` | Notes | post on the PR | literal `app.rs:2091` | `:w` |
| `ctrl+r` | Ask; Chat | ask again | literal `app.rs:2404`, `:2586` | **`history.redo`** |
| `PageDown` `PageUp` | all screens and overlays | scroll a page | literal `app.rs:1019-1022`, `:1662-1665`, `:1800-1804`, `:2375`, `:2584` | not in the plan |
| `ctrl+c` | everywhere, setup, picker | quit | literal `app.rs:937` | keep |
| letters, `backspace` | note line, Ask, Chat, Commit and Post edit lines, setup fields | typing | `typed` `app.rs:173`, plus each handler | insert mode |

**Collisions today**, meaning one key has different meanings on different screens or in different modes: `a` (interpret tests / approve), `c` (tests / commit / hand to agent), `f` (needs fix / grouping), `r` (re-run / re-ask), `h` (flag / previous card), `←` `→` (decide / tiles / history), `i` (flag / toggle inline), `e` (editor / edit message), `v` (visualize / verdict), `d` (open Flagged / delete note), `y` (yes / copy), `/` (Ask / Chat / leave link mode), `ctrl+u` (previous file / clear the line).

**Keys v2 takes from v1 with a different meaning:** `l`, `h`, `i`, `f`, `K`, `n`, `N`, `a`, `/`, `ctrl+d`, `ctrl+u`, `ctrl+r`, `g`.

## 4. Unit state

| Thing | Where | How |
| --- | --- | --- |
| Decision | `Decision` `review/model.rs:307-314` | `Pending`, `Accepted`, `Investigate` (= flagged). Serialized under those names, with no alias. |
| Needs fix | `Dive.resolution: Option<Resolution>` `model.rs:745`, `Resolution{Ok, NeedsFix}` `:335-339` | Not a decision. Set on Flagged with `o`/`f` (`App::resolve` `app.rs:1501`); pressing the same key again clears it. |
| Note | `Dive.note`, `Dive.note_on` `model.rs:739-753` | `note_on` is the fingerprint at writing (`app.rs:1995`). `Unit::note_is_stale` (`model.rs:820-828`). |
| Questions | `Review.questions: Vec<Question>` `model.rs:88-107` | `Question.unit` is the unit on show when asked. `error` is not saved. |
| Who accepted | — | Nothing is recorded. The only approval record is `PrInfo.approved_head` (`model.rs:258-270`). |
| ○ units | `UnitStatus::NoBehaviorChange` | `prep = Skipped`, never sent to a model. Left out of the order (`app.rs:565-567`), swipe (`:2812`), the counts (`:856`) and `all_reviewed` (`:868`). Shown in a muted box (`slim.rs:848-859`). |
| Undo | `Undo{entries}` `app.rs:353-358` in `SwipeState.history` | In memory only, decisions only. `accept_file` is one step. Cleared by `enter_swipe` and `reset`. No redo. |
| Cursor | `App.sel`, `swipe.cursor` | Not saved. Every start opens on the list at row 0. A re-run keeps the selection by id (`app.rs:3146-3161`). |

**Storage** (`review/store.rs`):
- Data dir: `$TEER_DATA_DIR`, else `$XDG_DATA_HOME/teer`, else `~/.local/share/teer` (`:20-43`).
- Repo reviews go in `reviews/<fnv(repo_root)>`; PR reviews in `reviews/pr-<store_key>` (`:56-72`).
- File name: `<head_commit[..12]>-<fnv(base,head)>.json` (`:76-86`). A `latest` pointer file sits next to them (`:107-110`).
- Format: pretty JSON of the whole `Review`, written atomically (`save` `:97`).
- **There is no schema version, and `load`/`load_latest` (`:116-131`) silently ignore a file that doesn't parse.** `Dive` has no container-level `#[serde(default)]`.
- Saved after each decision, after prepare settles, on quit (`tui/mod.rs:224`) and in `--json` mode.

**Identity and carry-over.**
- Unit id: `"{file}::{symbol_path}"` (`units/mod.rs:55-57`). Fingerprint: `hash_hex(old own_text, new own_text)` (`units/mod.rs:77`); for whole files, `whole.rs:159`.
- `merge_previous` (`store.rs:135-185`) matches by id. With the same fingerprint it keeps the decision, the dive and the prepared description. With a changed fingerprint it keeps only the note and `note_on`; the decision goes back to `Pending`. A renamed unit has a new id and starts fresh.
- It runs at start (`cli::restore`) and on every re-run (`app.rs:3142`).

**Saved shape** (the same as `--json`):
- `Review{id, repo_root, base, head, head_commit, agent_session, created_at, files[], units[], skipped[], graph, pr?, intent?, questions?}`
- `Unit{id, file, symbol, kind, status, old_range, new_range, renamed_from?, fingerprint, changes[], prep, callers[], callees[], decision, summary?, nbc_reason?, dive{results, asked?, resolution, note, note_on?}, checks?{cost?, unmentioned?, second?, second_failed?}}`

## 5. Prepare pipeline

| Step | Where | When |
| --- | --- | --- |
| Split | `build_review` `review/mod.rs:36-70` → `units::analyze` `units/mod.rs:33-53`, then `graph::build` `graph/mod.rs:61` (callers, callees) | Sync, before the TUI (`cli.rs:372`, `pr.rs:1079`). On re-run, in `spawn_blocking` (`tui/mod.rs:298-312`). |
| Level | `attention::assess` `review/attention.rs:125` | Pure and not stored. Recomputed in `App::rebuild_order` (`app.rs:555`) on every `Prep` event and re-run. |
| Explain | `prepare::prepare` `review/prepare.rs:212-237` → `run_batch` `:87-162` (`ask_prepare`, `units_prompt` `prompts.rs:287`) | In the background from `App::start` → `prepare_start` (`app.rs:755-771`). Large reviews (>150 units) go lazily per file. |
| Check | `second_opinions` `prepare.rs:166-206` (`ask_second`, `validate_second` `validate.rs:268`) | Chained inside each batch when `second_opinion = true`. On demand with `x`. Not with `--json`. |

**Level** (`attention.rs:17-45`) has four values: `Same` ○, `Routine` ●, `Look` ◆, `Attention` ▲. `assess` works like this:
- `NoBehaviorChange` returns `Same` at once, before any agent signal.
- Everything else starts at `Routine`.
- Removed but still called → ▲.
- Signature changed with callers, ≥40 changed lines, a whole file too large to describe, or no description for a changed unit → at least ◆.
- Risk high → ▲; medium → at least ◆.
- `$` or `⚠` → at least ◆.
- `Assessment.reasons` already holds plain-text reasons ("removed, 3 still call it") but is not saved.

**Marks:**
- `$`: `checks.cost`, from the agent's `undo` answer through `validate_cost` (`validate.rs:198-218`) plus a migration-folder rule (`attention.rs:62-69`).
- `⚠`: `SecondOpinion::disagreements` (`model.rs:672-695`): a disputed statement, a missed change of medium or higher, or a risk mismatch.
- `≠`: `checks.unmentioned` through `validate_unmentioned` (`validate.rs:231-251`), asked only when there are commits or a PR to compare against.
- All three are drawn by `preview::check_lines` (`preview.rs:243-294`) in the list preview, the swipe card and the Flagged head. **List rows show none of them today**; a row has the level icon and `✎`.

**Order** (`App::rebuild_order` `app.rs:555-617`): the file's box (the highest level among its units, or the unit's own level when listed by unit), then path, then the unit's level, then story rank (`story::place` `story.rs:33-124`: callers just before their callees; `order = "line"` uses the line instead). Swipe uses the list order, sorted again by level. Flagged goes by file, then line.

## 6. Agent integration

- **Adapters.** `AgentAdapter` (`agent/mod.rs:54-101`) has `list_sessions`, `ask_structured`, `ask_dive`, `ask_live`, `ask_prepare`, `ask_probe` and `ask_second`. The implementations are Claude (`agent/claude.rs`), Codex (`agent/codex.rs`), the API (`agent/api.rs`) and `MockAdapter` (`agent/mock.rs`). For PRs, everything is wrapped in `UntrustedCode` (`pr.rs:1104-1152`), which adds the untrusted notice and turns web off. **`UntrustedCode` doesn't forward `ask_live`, so asks in PR reviews don't stream today.**
- **Hand-off.** Notes `c` → `notes_to_agent` (`app.rs:2116-2143`). It is refused for PRs and for the API provider. The prompt is `notes::agent_prompt`. Claude Code gets `--resume <id> --fork-session` when a session is set; Codex starts fresh. `open_agent` (`tui/mod.rs:659-684`) runs the CLI on the terminal (`with_terminal_released` `:584`) and waits for it to exit. Afterwards the status line says "Back from … r re-runs the review". **There is no automatic re-run.**
- **Sessions.** `agent/sessions.rs::list_sessions` (`:190`) reads `~/.claude/projects/<cwd>/<id>.jsonl`, only the first 512 KB of each (`HEAD_BYTES` `:25`), and marks sessions open by recency (10 min) and by `ps`. `discover_agent` (`cli.rs:144`, `:181-183`) stores the list in `App.sessions`, **which nothing reads**. Without `--session`, teer uses a fresh session (`tui/mod.rs:93-104`). A set session is forked (`--resume <id> --fork-session --no-session-persistence`) for Flagged questions, Ask, probes and the commit draft, never for prepare or check.
- **Re-run.** `r` → `Effect::Rerun` → `build_review` on the same base (`tui/mod.rs:298-312`; for a PR, the same checkout with no fetch) → `AppEvent::Rerun` (`app.rs:3140-3168`) → `merge_previous` → prepares only units whose `prep` is `NotRequested` or `Pending`.
- **Ask.** `Effect::Ask` → `review::ask::answer` (`review/ask.rs:49-87`) → `ask_live`, relaying `Live::Step` and `Live::Partial` (`agent/live.rs`) as `AppEvent::Asking`, then `Answer`.
- **Commit** (`commit.rs`):
  - Commits only the reviewed paths (`:157-173`): `git add -A -- <paths>`, then `git commit -F <tmp> --only -- <paths>`, with the user's hooks and signing.
  - `check()` (`:421-476`) refuses when units are not reviewed, a path is bad, a merge or rebase is in progress, HEAD moved, a file changed since the review, or there is nothing to commit.
  - The draft comes from `draft_message` (`:670-687`) through `ask_dive`, with a 30 s timeout and a template fallback (`:84-116`).
  - **`can_commit` (`app.rs:1676-1680`) allows only working-tree reviews, so a branch review can't commit.**
- **Post** (`approve.rs`): `Verdict{Approve, RequestChanges, Comment}` (`:73-77`). `post()` (`:543-615`):
  - checks `git ls-remote` for the PR head
  - posts with `gh api … /reviews`, at most 50 inline comments, the rest in the summary
  - retries once without inline comments on a 422
  - opens the PR page on any other failure (`:526-532`)
- **`teer notes`** (`notes.rs:104-133`): a header, then per unit `## file:a-b · symbol`, a ≤40-row diff, a stale warning when the note is stale, and the note. Notes `y` copies exactly this.
- **PR checkout:** `<data>/worktrees/<repo hash>/checkout`, kept and locked. A throwaway checkout is used when the lock is busy (`pr.rs:556-575`, `:803-902`).

## 7. Config

`Config` (`config.rs:165`) is read from `$TEER_CONFIG`, else `~/.config/teer/config.toml`. Missing means defaults. It has `#[serde(default)]`, so unknown top-level keys are ignored.
- **Keys:** theme, concurrency, languages, large_review, dive_web, images, provider, `[api]` (format, base_url, key_env), prepare_model/effort, order (story | line), second_opinion, second_model/effort, behavior_timeout, editor, and `[keys]`.
- **`[keys]`** is a `HashMap<String,String>` from v1 action names (the middle column of `Action::ALL`, `config.rs:457-521`) to one key each. **An unknown action name in `[keys]` is a hard error today (`config.rs:570`)**, so Phase 8's "warn and continue" changes that.
- The doc comment at `config.rs:1-27` is out of date (`accept = "l"`, `investigate = "i"`).
- `teer setup` writes the config with `toml_edit`, keeping the user's comments (`setup.rs`).

## 8. Tests

- **Running:** `cargo test` (the full run takes over 10 minutes, so run it in the background), `cargo clippy --all-targets`, `cargo fmt --check`. Seven live tests (`tests/live_*.rs`) are `#[ignore]` and need real agents. **CI runs no tests**: `.github/workflows/release.yml` only builds releases.
- **Layout:** 40 files in `tests/`, plus unit tests inline in `src/` under `#[cfg(test)]`. The helpers are in `tests/common/mod.rs`:
  - `Repo` makes a real git repo in a temp dir.
  - `isolate_env` points `TEER_DATA_DIR`, `TEER_CONFIG` and `TEER_GH` at a temp dir.
  - `teer_bin()` is the built binary.
  - There are language fixtures.
- **How the UI is tested, in three ways:**
  1. **Key replay on `App`:** build an `App`, call `on_key(KeyEvent)`, and assert the state and the returned `Effect`s. Effects are data, so nothing runs (`tests/tui*.rs`, `app.rs` tests from `:3326`).
  2. **Render snapshots:** ratatui `TestBackend` with `insta`; 13 snapshots in `tests/snapshots/`.
  3. **Binary end to end:** `teer_bin()` with fake `gh` and `claude` scripts (`tests/pr.rs`, `commit_flow.rs`, `approve_pr.rs`).

  `MockAdapter` scripts every model answer and records the prompts. There is no pty-level harness; earlier live checks used a hand-run Python pty driver.

## 9. Phase-by-phase notes

**Phase 1, keymap and actions.**
- Touches: new `src/keymap.rs`, which takes `Action`, `KeyMap` and `parse_key` out of `config.rs:389-612`. Then `app.rs` (`on_key`, `action_for`, `flagged_action_for`, every overlay handler, `typed`), `ui.rs` (`key_of`, `draw_confirm`, `draw_help`), `flagged.rs::key_of`, `pr_pick.rs` and `tui/setup.rs`.
- `KeyMap` must allow several keys per action (`l`/`enter`, `h`/`esc`, arrows), key sequences, counts, a pending-prefix timer (the tick is 100 ms, `tui/mod.rs:140-153`) and modes (normal, visual, command line, insert, confirm).
- Undo and redo move from `SwipeState` onto the review and are saved, which is the first change to the saved format. Add a `version` field then.
- Risk: the v2 default table takes 13 v1 keys. See Q1.

**Phase 1 as built (2026-11-02).** Q1 went with (a): v2 keys ship in Phase 1. The old screens keep their extra functions on temporary `v1.*` actions, which Phase 8 deletes:

| Key | Action id | Was (v1) |
| --- | --- | --- |
| `s` | `v1.swipe` | `s` |
| `gt` `gT` `tab` `⇧tab` `→` `←` | `v1.next_tile` `v1.prev_tile` | `→` `←` `tab` |
| `zz` | `v1.zoom` | `z` |
| `<space>w` | `v1.recommend` | `n` |
| `<space>m` | `v1.disagree` | `x` |
| `<space>a` | `v1.interpret_tests` | `a` (Tests tile) |
| `o` `x` | `v1.resolve_ok` `v1.resolve_fix` | `o` `f` |
| `r` | `v1.dive_rerun` | `r` (Flagged) |
| `c` `p` (notes view) | `v1.notes_to_agent` `v1.notes_post` | `c` `p` |
| `d` (notes view) | `note.delete` | `d` |

Also added: `zf` (`toggle.by_file`, permanent: the queue by level or by file, see Q3), `]f` `[f` (`move.next_file`/`prev_file`, Q4), `gx` (`goto.browser`, Q17), `<pagedown>` `<pageup>` (`scroll.page_*`, Q19), `timeoutlen` in `config.toml` (Neovim's name for the prefix wait). `[keys]` takes action ids and Neovim notation, a string or a list. Unknown ids warn and are skipped, and an override takes its key away from the default that had it. Saved reviews get `version` and `history` (undo and redo).

**Phase 2, one start command.**
- Touches: `cli.rs` (a positional URL or number next to the subcommands, `pr` with no URL opening the picker, hidden `setup`, `--session` and `--swipe`), `git/mod.rs::resolve` (detection, the base chain), `pr.rs::parse_url` (bare numbers), `agent/sessions.rs` (the "touched the changed files" filter), `ui.rs::draw_header`.
- Risks:
  - Clap must not read `teer notes` as a PR.
  - Asking `gh` for the PR base adds a network call at start, so it needs a timeout and a fallback.
  - The session filter needs the transcript's edit tool calls, but only the first 512 KB is read today. See Q9.

**Phase 3, prepare output.**
- Touches: `attention.rs` (reasons), `model.rs` (`Unit.reasons`, `accepted_by`), `validate.rs` (cost and unmentioned become reason text), `preview.rs::check_lines`, `slim.rs` (muted box, row `⚠`), `app.rs` (`rebuild_order`, counts, `all_reviewed`), `story.rs`, `config.rs` (`auto_accept`), `cli.rs` (`--json`).
- Risks:
  - "Disputed ○" can't happen today; see Q2.
  - Today a dispute raises a unit to ◆, not ●.
  - The plan's sort order differs from today's file boxes; see Q3.
  - `--json` is the serde of `Review`, so the new fields get there for free, but the level and reasons must be stored to appear.

**Phase 4, the queue.**
- Touches: new `src/tui/queue.rs`, `ui.rs` dispatch, `slim.rs` and `preview.rs` (reused), `app.rs` (`decide`, `next_pending_pos` `:2632`, note line, `A`/`F`, visual mode), `store.rs` (cursor, undo stack).
- Risks:
  - `f` used to switch by file / by unit, so that switch loses its key (Q3).
  - `ctrl+d` and `ctrl+u` stop jumping files (Q4).
  - The bottom bar replaces today's footer buttons (`buttons_for`), which are mouse targets too.

**Phase 5, the unit view.**
- Touches: new `src/tui/unit.rs` replacing `flagged.rs` (1,134 lines), `review/dive.rs` (Recommend, Disagree), `tui/ask.rs` (merging the Ask overlay and the Chat tile into one `K` panel), `diff.rs` (hunk jumps), `graph/mod.rs` (`gd`/`gr`), `visual.rs` (`gx`), prefetch in `app.rs`, `pr.rs::UntrustedCode` (forward `ask_live` so PR asks stream).
- Risks:
  - Recommend runs on the deep model (the CLI's default, Opus), so prefetching 3 units per review start costs real money.
  - The plan has no key for the agent's test interpretation (Q6), for "more light" (Q7) or for an in-review settings screen (Q8).
  - `gd` needs a cursor that doesn't exist (Q5).

**Phase 6, `:w` finish.**
- Touches: new `src/tui/finish.rs`, replacing `AllDone`, `Notes`, `Commit` and `PostReview`. It reuses `commit.rs`, `approve.rs` and `notes.rs`.
- Risks:
  - A branch review has nothing to commit (Q10).
  - The API provider has no agent to send notes to (Q11).
  - Today `e` opens `$EDITOR` for the message; v2's `i` must pick inline or editor (Q12).
  - After success teer now exits, where today it shows "Committed <sha>".

**Phase 7, fix loop.**
- Touches: `tui/mod.rs::open_agent` (`:659`), a tree watcher (`notify` crate or polling; a new dependency), `store::merge_previous` (move changed units to the top of their level, archive the note on accept: needs a new field), `AppEvent::Rerun`.
- Risks:
  - The hand-off gives the terminal to an interactive agent, so a "waiting screen" can't be visible during the work (Q13).
  - `:e` on a PR doesn't fetch (Q14).

**Phase 8, remove and migrate.**
- Deletes: `swipe.rs`, `flagged.rs`, the AllDone, Notes and Commit overlays, `Resolution`, `--swipe`, and the `$`/`≠` drawing.
- Migration:
  - `Investigate` → flagged. `NeedsFix` → flagged, note kept. `Ok` → flagged.
  - There is no swipe position to migrate, because none is saved.
  - Old `[keys]` names map through a table and unknown ones warn instead of failing.
  - Reviews that don't parse are dropped silently today; test that v1 files load.
- teer has no man page. "Update the man page" means `--help`, README and FEATURES.md, unless you want one.

## 10. Open questions

**Answers (2026-11-02):** Q1 (a). Q2 (a): every ○ unit is accepted for you, no model check. Q3: the plan's layout (each unit under its own level, then file) is the default, and a switch to the file boxes stays, on `zf` (`toggle.by_file`, next to `za`). Q10 (a): done, exit. Q11 (a): copy, then watch the tree. Q13 (a): keep the interactive hand-off. Q4–Q9, Q12, Q14–Q19: the defaults below. Commit each phase on its own branch, stacked; don't push. Run the phases through without the review stops.

The plan says to stop and ask when it conflicts with the code. Each question has two options; **bold** is what I'd do.

**Need your call:**

- **Q1. When v2 keys land.** The rules test in Phase 1 can't pass with v1 keys (13 collisions), but v2 keys on the v1 screens change what `l`, `h`, `i`, `f`, `n`, `a` do before the queue exists.
  - **(a) Phase 1 ships v2 defaults. The old screens' extra actions (tiles, zoom, resolution, swipe) sit on temporary leader keys until Phase 8.**
  - (b) Phase 1 ships the layer with a v1 profile, and v2 defaults switch on in Phase 4, with the rules test enforced from then.
- **Q2. Disputed ○.** No model ever sees a ○ unit (`prep = Skipped`), so "the second model did not dispute it" is always true. Also, a dispute raises a unit to ◆ today, not ●.
  - **(a) Auto-accept every ○ with no model check, as today (they're already left out of the counts).**
  - (b) Send ○ units to the second model in batches and auto-accept only the confirmed ones. That adds calls and time before the queue settles.
- **Q3. Queue grouping.** The plan says "grouped by level, then file" and "sort by level, then callers before callees, then path", so one file can appear under ▲ and again under ●. That's today's "by unit" mode; the default is "by file" (each file in the box of its most critical unit). `f` was the switch.
  - **(a) Level, then file, with the unit's own level, as the plan says. Drop the switch.**
  - (b) Keep the file boxes as the only layout, with no switch.
- **Q10. Branch review, all accepted.** There's nothing to commit (`can_commit` needs a working-tree review).
  - **(a) The default step is "done", which exits, with yank as the alternative.**
  - (b) The default step is "open a pull request" (`gh pr create`). That's a new outward action, and the plan puts new features out of scope.
- **Q11. Notes with the API provider.** There's no CLI agent to send them to.
  - **(a) The default step becomes "yank the notes and wait": teer watches the tree (Phase 7's quiet period) and re-runs when it settles.**
  - (b) Offer only yank, with no wait.
- **Q13. The fix loop's waiting screen.** Today's hand-off runs `claude`/`codex` interactively on teer's terminal, so teer can't show a queue while the agent works.
  - **(a) Keep the interactive hand-off. The agent exiting is the completion signal, and teer re-runs at once when it gets the terminal back. The waiting screen and quiet-period watcher are used only when the notes went somewhere teer doesn't run (yank, API).**
  - (b) Run the agent headless in the background with write access (`claude -p --resume <id> --permission-mode acceptEdits`, `codex exec --sandbox workspace-write`) and show the waiting screen with streamed steps. teer has never run an agent that writes, so this changes its safety story.

**Defaults I'll take unless you say otherwise:**

- **Q4. Jumping between files.** `ctrl+d`/`ctrl+u` become half-page scroll. **(a) Bind `]f` `[f` (`move.next_file`, `move.prev_file`).** (b) Drop file jumps.
- **Q5. `gd`/`gr` with no cursor.** The unit view scrolls; it has no line cursor (notes went whole-hunk on 2026-10-19). **(a) `gd` lists the symbols this unit calls that are in the review (graph callees), to pick one; `gr` lists the callers.** (b) Add a line cursor to the unit view.
- **Q6. The agent reads the tests** (today `a` on the Tests tile). **(a) It runs on its own after `<space>t` when an agent is configured.** (b) Give it its own leader key, `<space>T`.
- **Q7. "More light" and web search.** **(a) The "more light" questions become the empty-state suggestions of the `K` panel, and the one-time web-search confirm moves to the first `K` that can use the web.** (b) Drop both.
- **Q8. `<space>s` settings.** Setup is a separate program loop today. **(a) Suspend the queue, run the setup screen, and reload the config on return.** (b) Leave `<space>s` out of v2 and keep `teer setup`.
- **Q9. Which session wrote the change.** **(a) Read the tail of recent transcripts (capped) for Edit/Write/MultiEdit `file_path`s that match the changed files.** (b) Use the newest session in the repo from the last 24 h, without the file check.
- **Q12. `i` on a commit message.** **(a) `i` opens the configured editor, as `e` does today, and falls back to the one-line field.** (b) A multi-line editor inside the sheet.
- **Q14. `:e` on a PR.** **(a) `:e` fetches the PR head again before re-running, so a pushed fix shows up.** (b) Re-run on the same checkout, as `r` does today.
- **Q15. Confirms and the one-meaning rule.** `:e!` asks "y/n", but `y` is `finish.yank` and `n` is `move.next_pending`. **(a) Confirm prompts and text inputs are modes outside the rule: the keymap module owns them and the rules test skips them.** (b) Confirms take only `enter`/`esc`.
- **Q16. "No key literal outside the keymap module."** **(a) The rule covers the setup screen and the PR picker too, as their own keymap contexts, and text inputs get their editing keys (`enter`, `esc`, `backspace`, `ctrl+u`) from the keymap. A test greps `src/` for `KeyCode::` outside `src/keymap.rs`.** (b) Exempt setup and the picker.
- **Q17. `gx` is missing from Phase 1's table.** It goes in as `goto.browser`.
- **Q18. "Fake git".** The repo's tests use real git in temp repos, which is hermetic already. **(a) Keep real git in temp repos; fake only `gh` (`TEER_GH`), the agent CLIs (scripts on `PATH`) and the model (`MockAdapter`).** (b) Add a git fake layer.
- **Q19. `PageUp`/`PageDown` and arrows.** They aren't in the plan. **(a) Keep them as extra default keys of `scroll.*` and `move.*`.** (b) Drop them.

## 11. Requirements and their integration tests (Phase 9)

Added at your request: once Phase 8 is done, every requirement below gets an integration test. Each phase still writes its own tests first, as the plan says. Phase 9 adds the cross-cutting suite and makes sure each row is covered.

**Harness** (built at the start of Phase 9, in `tests/common/v2.rs`):
- `Driver` builds a review from a fixture repo (real git in a temp dir) and runs `App` with `MockAdapter`.
- `.keys("3a:w<cr>")` takes Neovim notation, turns it into `KeyEvent`s and feeds them in.
- It executes the returned `Effect`s against the fakes:
  - git for real in the temp repo
  - `gh` as a recording script through `TEER_GH`
  - the agent CLI as a script on `PATH` that edits files and exits
  - the model through `MockAdapter`
- It exposes the state, the effects, the status and bottom-bar text, and an `insta` render.
- Start-up cases (detection, flags, help) run `teer_bin()` in an isolated env.

| ID | Requirement | Test |
| --- | --- | --- |
| K1 | All input goes through the keymap; screens only see actions | `v2_keymap::screens_receive_actions_not_keys` |
| K2 | Prefixes `g` `]` `[` `z` `<space>`, 1000 ms timeout (configurable), hint line lists what follows | `v2_keymap::prefix_waits_shows_hint_and_times_out` |
| K3 | Counts before actions; `esc` drops a dangling count | `v2_keymap::counts_apply_and_esc_drops_them` |
| K4 | `:` takes `w` `e` `e!` `q`; unknown commands show an error and change nothing | `v2_keymap::command_line_accepts_four_commands` |
| K5 | `.` repeats the last decision with its count, never a note | `v2_keymap::dot_repeats_decisions_not_notes` |
| K6 | `V` selects, `j`/`k` extend, `a`/`f` apply to all, `esc` leaves | `v2_keymap::visual_mode_applies_to_selection` |
| K7 | `u`/`ctrl-r` undo and redo stack, saved with the review | `v2_keymap::undo_redo_survives_restart` |
| K8–K11 | The four keymap rules hold for the default keymap | `v2_keymap::default_keymap_follows_the_rules` |
| K12 | Every row of the default bindings table does its action (one case per row) | `v2_keymap::every_default_binding` |
| K13 | No key literal outside the keymap module | `v2_keymap::no_key_literals_outside_keymap` |
| K14 | `[keys]` rebinds by action id, and help shows the new key | `v2_keymap::rebinding_changes_dispatch_and_help` |
| S1 | A dirty tree (staged, unstaged, untracked) reviews the tree | `v2_start::dirty_tree_reviews_the_changes` |
| S2 | A clean tree on a feature branch reviews since the merge base | `v2_start::clean_branch_reviews_since_base` |
| S3 | A clean tree on the default branch prints the message and exits 0 | `v2_start::clean_default_branch_exits_zero` |
| S4 | Base chain: `--base`, the PR base from `gh`, `origin/HEAD`, `main`/`master` | `v2_start::base_fallback_chain` |
| S5 | The header names the base and the commit count | `v2_start::header_names_base_and_commits` |
| S6 | `teer <url>` and `teer 482` review the PR | `v2_start::url_and_number_open_the_pr` |
| S7 | `teer pr` opens the picker; the user's tree is untouched | `v2_start::pr_picker_keeps_tree_untouched` |
| S8 | First run: setup, then straight into the review; `teer setup` hidden but works | `v2_start::first_run_setup_then_review` |
| S9 | The session that touched the changed files in the last 24 h is used for `K`; `--session` overrides | `v2_start::session_discovery_and_override` |
| S10 | Help shows `--base`, `--no-agent`, `--json`; `--swipe` still works, hidden | `v2_start::visible_and_hidden_flags` |
| P1 | Three levels shown; ○ auto-accepted (`by: auto`) and hidden | `v2_prepare::no_behavior_change_is_accepted_for_you` |
| P2 | Auto-accept rule for disputed ○ (per Q2) | `v2_prepare::disputed_no_change_is_not_auto_accepted` |
| P3 | `auto_accept = "none"` turns it off | `v2_prepare::auto_accept_none_keeps_units_pending` |
| P4 | Auto-accepted units count for finishing; `u` makes one pending | `v2_prepare::auto_accepted_count_and_undo` |
| P5 | `$` `⚠` `≠` become sentences under "Why this level" | `v2_prepare::marks_become_reasons` |
| P6 | Queue rows show `⚠` only | `v2_prepare::queue_rows_show_only_dispute_glyph` |
| P7 | Order: level, callers before callees, path; the first unit shown is the first pending ▲ | `v2_prepare::queue_order_and_first_unit` |
| P8 | `--json` has `reasons` and `accepted_by` and keeps the old fields | `v2_prepare::json_has_reasons_and_old_fields` |
| Q1 | The queue is the default screen; old screens only through `--swipe` | `v2_queue::queue_is_the_default_screen` |
| Q2 | The header says what is reviewed plus one summary line | `v2_queue::header_says_what_is_reviewed` |
| Q3 | Rows: glyph, path, name, state, the first 40 characters of the note, `⚠` | `v2_queue::row_contents` |
| Q4 | "N units accepted for you, za shows them"; `za` toggles | `v2_queue::za_shows_auto_accepted` |
| Q5 | The preview sits right or below by width, with today's content | `v2_queue::preview_side_or_below` |
| Q6 | The bottom bar shows the counts and the pending prefix or mode | `v2_queue::bottom_bar_counts_and_mode` |
| Q7 | Auto-advance to the next pending unit; at the end, "Nothing pending. :w to finish" | `v2_queue::auto_advance_through_mixed_states` |
| Q8 | `f` opens the note line, prefilled; `enter` saves (empty is fine); `esc` keeps the flag | `v2_queue::flag_note_line` |
| Q9 | "Needs fix" no longer exists | `v2_queue::no_needs_fix_state` |
| Q10 | `A`/`F` act only on pending units in the file; `F` copies one note to each | `v2_queue::file_actions_skip_decided_units` |
| Q11 | `f` in visual mode puts the note on every selected unit | `v2_queue::visual_flag_notes_every_unit` |
| Q12 | `3a` accepts this unit and the next two pending | `v2_queue::count_accepts_pending_units` |
| Q13 | Click selects, drag copies | `v2_queue::mouse_select_and_copy` |
| Q14 | Decisions, notes, undo and the cursor are saved on every change; `q` and a crash resume | `v2_queue::resume_after_quit_and_crash` |
| U1 | `l`/`enter` open any unit with title, why, what changed, what to do, second model; `]c` `[c` jump hunks | `v2_unit::sections_and_hunk_jumps` |
| U2 | The recommendation runs on open; prefetch of 3 ▲/◆ plus the next; cached per unit and hash | `v2_unit::prefetch_and_cache` |
| U3 | `--no-agent` hides sections 4 and 5 and leaves no empty section | `v2_unit::no_agent_hides_agent_sections` |
| U4 | `K` panel: focused input, streaming, steps, links (`tab`, `enter`), follow-ups, closes on `h`/`esc`, kept per unit | `v2_unit::ask_panel_lifecycle` |
| U5 | `gd` in and outside the review, `gr`, `gf` (per Q5) | `v2_unit::goto_definition_callers_editor` |
| U6 | `<space>t` `r` `v` panels one at a time; `<space>` hint; `gx` opens the diagram | `v2_unit::leader_panels_and_gx` |
| U7 | `a f A F u .` work here and advance inside the unit view; `n`/`N` move without deciding | `v2_unit::decide_and_advance_in_unit_view` |
| F1 | `:w` opens one sheet; `enter` does its default step | `v2_finish::enter_runs_the_default_step` |
| F2 | The four rows and every alternative (`j`/`k`) | `v2_finish::default_and_alternatives_per_case` |
| F3 | Pending units: the count shows first, `n` jumps to the first, the default is "keep reviewing", other steps confirm | `v2_finish::pending_warning` |
| F4 | The sheet lists flagged units with their notes | `v2_finish::sheet_lists_notes` |
| F5 | `y` copies the Markdown with diffs | `v2_finish::yank_format` |
| F6 | `i` edits the commit message or the review summary | `v2_finish::i_edits_message_or_summary` |
| F7 | `esc`/`q` close without doing anything | `v2_finish::close_does_nothing` |
| F8 | Safety: only reviewed files, hooks and signing, refuses a changed file, checks the PR head, `gh` only after `enter`, opens the page on failure | `v2_finish::each_safety_check_refuses` |
| F9 | A commit or post prints one line and exits | `v2_finish::success_exits_with_one_line` |
| L1 | Notes go to Claude Code or Codex, into the found session | `v2_loop::hand_off_uses_found_session` |
| L2 | Waiting status line; `q` stops waiting; `:e` later (per Q13) | `v2_loop::waiting_and_q` |
| L3 | Done by signal, or by `fix_loop.quiet_seconds` of quiet | `v2_loop::completion_by_signal_and_by_quiet` |
| L4 | Changed units turn pending, keep the note marked changed, top of their level; others keep their decision | `v2_loop::rerun_changed_vs_unchanged` |
| L5 | The cursor lands on the first changed unit; "3 units changed by Claude Code" | `v2_loop::cursor_and_status_after_rerun` |
| L6 | On a changed note: `a` archives it, `f` edits with the flag kept, `i` edits without a decision | `v2_loop::notes_on_changed_code` |
| L7 | `:e` re-runs now; `:e!` asks "Reset all N decisions? y/n", then re-evaluates everything | `v2_loop::rerun_and_reset_commands` |
| M1 | v1 screens, needs fix, `--swipe`, `$`, `≠` are gone | `v2_migrate::v1_surface_removed` |
| M2 | `--base main` and `pr list` still work for one release and print a hint | `v2_migrate::old_forms_hint` |
| M3 | A v1 saved review loads: needs fix → flagged with its note, flagged stays flagged | `v2_migrate::v1_review_fixture_loads` |
| M4 | v1 `[keys]` map over; removed ones warn once each; nothing is written back | `v2_migrate::v1_keys_config` |
| M5 | `?` groups keys (Move, Decide, Look closer, More, Finish) from the keymap, with rebinding | `v2_migrate::help_is_generated_from_keymap` |
| M6 | `teer --help` lists only the four forms and three flags | `v2_migrate::help_lists_v2_surface` |
| A1–A12 | The plan's acceptance checklist, each scenario end to end | `v2_acceptance::{agent_changes_nothing_wrong, agent_changes_one_fix, branch_before_pr, pr_approve, pr_request_changes, look_closer, bulk_and_undo, pending_warning, resume, keymap_rules, v1_user, no_agent}` |

M7 (README, "Coming from v1") is docs and gets no test. The by-hand run on a real repo stays a manual step after Phase 9.
