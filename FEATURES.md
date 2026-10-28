# teer features

[how it works](#how-it-works) · [commands](#commands) · [agents](#agents) · [the list](#the-list) · [swipe](#swipe) · [the flagged screen](#the-flagged-screen) · [ask](#ask) · [notes](#notes) · [commit and approve](#commit-and-approve) · [keys](#keys) · [config](#config) · [safety](#safety) · [limits](#limits)

## how it works

1. **split** — teer parses the old and new version of every changed file and lists the units that changed: functions, methods, classes, top-level code. rust, python, typescript, javascript and php are split; any other text file is one whole-file unit. binary, huge and non-utf-8 files are listed as not shown.
2. **sort** — every unit gets a level from what the code shows. the agent can raise it, never lower it.
3. **explain** — a fast model writes a one-line summary and up to three statements per unit. a statement that doesn't cite lines of its unit is dropped. a second model checks every description.
4. **triage** — accept or flag, in the list or one card at a time.
5. **dig in** — flagged units get a screen of their own, with questions on demand.
6. **hand back** — notes go to your agent, the clipboard or the pull request. accepted changes can be committed, a pr approved.

your place is saved as you go. a re-run (`r`) keeps every decision, answer and note for units whose code didn't change.

### levels

| | level | when |
| --- | --- | --- |
| `▲` | needs a close look | removed but still called · the agent rates it high risk |
| `◆` | worth a look | signature changed and it has callers · 40+ changed lines · medium risk · the second model disagrees · expensive to undo · couldn't be described |
| `●` | routine | everything else that changes behavior · removed with no callers · lock files, snapshots |
| `○` | no behavior change | formatting or comments · renamed locals · renamed with the same body |

### markers

| | |
| --- | --- |
| `$` | expensive to undo: deletes or rewrites data, changes a schema, sends, charges or publishes, changes a public api. files under `migrations/` always count |
| `⚠` | the second model disagrees: a disputed statement, a missed change, or a different risk |
| `≠` | a behavior change the commit messages and pr description don't mention |

## commands

| | |
| --- | --- |
| `teer` | your uncommitted changes, untracked files included |
| `teer --base main` | this branch against where it left `main` |
| `teer pr <url>` | a github pull request of this repo |
| `teer pr list` | pick a pr waiting for your review, assigned to you, or yours |
| `teer notes [url]` | print the notes of the last review as markdown |
| `teer setup` | pick the agent and its models, check what teer needs |

| flag | |
| --- | --- |
| `--swipe` | open in swipe mode |
| `--no-agent` | no agent: sort the change and review it yourself |
| `--json` | prepare the review, print it as json, exit |
| `--session <id>` | claude code only: questions go to a fork of that session, which knows why the code changed |
| `-C <dir>` | run as if started in `<dir>` |

**pull requests** are fetched with plain git into a checkout teer keeps for itself, so your working tree is never touched. `gh` adds the title, author and description; without it teer reads the commits. new commits on the pr keep your decisions for every unit that didn't change.

## agents

`teer setup` (also on the first start) picks what explains the changes:

| agent | runs as |
| --- | --- |
| claude code | `claude -p`, headless |
| codex | `codex exec`, in its read-only sandbox |
| anthropic · openai · openrouter · ollama · any openai-compatible server | a model api; teer runs the agent with its own read-only tools |

then two models:

- **fast model** — describes every unit. quick and cheap: `haiku` by default.
- **second model** — checks each description: `sonnet` by default. `none` turns the check off.

the flagged questions, ask and commit messages use the cli's own default model, or the second model with an api.

setup also checks what else teer needs: git, gh (and whether it's logged in), your editor and the macos sandbox. before saving, the fast model gives a test answer. api keys never go into the config, only the name of the variable that holds one.

## the list

one box per level, most critical first. each file sits in the box of its most critical unit, with its units under it. units that call each other stay together, caller first (`↳`).

- `f` lists units by their own level instead of by file
- `ctrl+d` `ctrl+u` jump between files, `] [` between pending units
- `A` accepts every pending unit of a file
- accepted units dim in place; flagged ones turn red

the preview (beside the list from 110 columns, else below) shows why the unit got its level, the agent's summary, the `$ ⚠ ≠` lines, and the code change with each statement written under the lines it cites. `J` `K` scroll it, `e` opens the unit in your editor.

**history** — `C` shows a summary of the whole change, the pr's title and description, and every commit with its type and tickets. `enter` on a commit lists the units it changed.

**mouse** — click to select, wheel to scroll, drag to copy, double-click to copy a word (`src/app.rs:120` and urls count as one).

## swipe

`s` deals the pending and flagged units as cards, most critical first.

| | |
| --- | --- |
| `l` `→` swipe right | accept |
| `i` `←` swipe left | flag |
| `h` | previous card |
| `u` | undo |
| `esc` | back to the list |

once everything is accepted, teer offers the next step: commit (`c`) or approve the pr (`a`).

## the flagged screen

`d` shows your flagged units one at a time: the code in a tall tile, then a tile for every question you ask.

| key | question | |
| --- | --- | --- |
| `n` | recommend | what to do about it. may search the web, after asking once; never for a pr |
| `v` | visualize | what talks to the unit and what the change adds or removes, as a diagram in the terminal (kitty, ghostty, iterm2, sixel). `b` opens it in the browser |
| `c` | tests | the tests that reach the unit, found by teer at once without an agent. `a` has the agent read them: what each checks, and the dangerous cases none covers |
| `t` | behavior | the agent writes a small program, teer runs it before and after the change in a sandbox, then checks the statements against the runs and whether your tests notice the change. macos only |
| `x` | disagreement | the second model's verdict on every statement |
| `/` | chat | a conversation about this unit |

`tab` `←` `→` move between tiles, `z` zooms, `r` asks again, `j` `k` go to the next flagged unit. resolve with `o` (fine) or `f` (needs a fix).

like statements, every claim in a tile must cite real lines. what doesn't is dropped and logged to `teer.log`.

## ask

`/` asks anything about the code; "this" is the unit on screen. the answer brings the code it talks about (read by teer, changed lines marked) and the units it relates to: `tab`, then `enter` jumps to a unit or opens the code in your editor. answers stay on the unit they're about and are kept with the review.

## notes

`m` writes a note on the unit on screen, anywhere. `N` lists them all:

| | |
| --- | --- |
| `y` | copy as markdown, each with its unit's diff |
| `c` | hand them to claude code or codex to work through (not for a pr) |
| `p` | post them on the pull request |

`teer notes` prints the same markdown for any agent. a note stays when its code changes, marked as such until you confirm it with `m`.

## commit and approve

**commit** — when every unit of your uncommitted changes is accepted, `c` commits them. the agent drafts the message, `e` edits it. teer commits exactly the files you reviewed, with your hooks and signing, and refuses when one changed since.

**approve** — when every unit of a pr is accepted, `a` approves it; `p` in the notes requests changes or comments (`v` switches). notes become inline comments where their line is in the pr diff, the rest go in the summary. teer checks the pr still has the head you reviewed, posts with `gh` only after you confirm, and opens the pr page if posting fails.

## keys

| everywhere | |
| --- | --- |
| `?` | every key |
| `/` | ask |
| `m` · `N` | note · all notes |
| `e` | open in your editor |
| `J` `K` | scroll the preview, card or tile (pageup / pagedown: a page) |
| `r` | re-run the review |
| `R` | reset every decision |
| `q` · `ctrl+c` | quit, saved |

| list | |
| --- | --- |
| `j` `k` `g` `G` | move |
| `l` · `i` | accept · flag |
| `u` | undo |
| `] [` | next · previous pending |
| `ctrl+d` `ctrl+u` | next · previous file |
| `A` | accept the file |
| `f` | by file · by unit |
| `s` · `d` · `C` | swipe · flagged · commits |

swipe and flagged keys are in their sections. every key can be rebound (see [config](#config)).

## config

`~/.config/teer/config.toml`. a bad value stops teer at start and says why.

```toml
theme = "light"
editor = "code --wait --goto {file}:{line}"
second_opinion = false

[keys]
accept = "y"
```

| key | default | |
| --- | --- | --- |
| `provider` | set by setup | `claude`, `codex` or `api` |
| `prepare_model` | per agent | the fast model; `default` is the cli's own |
| `second_model` | per agent | the second model |
| `prepare_effort` · `second_effort` | `low` | how hard each model thinks |
| `second_opinion` | `true` | check every description with the second model |
| `[api]` | | `format` (`anthropic` or `openai`), `base_url`, `key_env` (the variable holding the key) |
| `editor` | `nvim +{line} {file}` | what `e` runs; `{file}` and `{line}` are filled in |
| `theme` | `dark` | `dark`, `light` or `mono` (`NO_COLOR` means mono) |
| `order` | `story` | callers and callees together, or `line` |
| `languages` | all five | which ones are split into functions |
| `concurrency` | `4` | agent requests at once |
| `large_review` | `150` | above this many units, a file is described when you reach it |
| `dive_web` | `true` | recommend may search the web |
| `images` | `true` | draw diagrams in the terminal |
| `behavior_timeout` | `120` | seconds per behavior run |
| `[keys]` | | `action = "key"` |

**editors** — a gui editor needs its wait flag, or commit messages and review text come back unedited; `e` then waits until you close the tab:

```toml
editor = "code --wait --goto {file}:{line}"   # vs code
editor = "idea --wait --line {line} {file}"   # intellij
editor = "zed --wait {file}:{line}"           # zed
```

<details>
<summary>key actions</summary>

`quit` q · `down` j · `up` k · `top` g · `bottom` G · `preview_down` J · `preview_up` K · `open` enter · `edit` e · `swipe` s · `rerun` r · `reset` R · `accept` l · `investigate` i · `flagged` d · `undo` u · `back` esc · `help` ? · `next_pending` ] · `prev_pending` [ · `prev_card` h · `next_group` ctrl+d · `prev_group` ctrl+u · `grouping` f · `accept_file` A · `commits` C · `ask` / · `edit_note` m · `notes` N · `dive_recommend` n · `dive_visualize` v · `dive_tests` c · `interpret_tests` a · `dive_behavior` t · `dive_disagree` x · `open_visual` b · `dive_rerun` r · `resolve_ok` o · `resolve_fix` f · `prev_tile` left · `next_tile` right · `zoom` z · `approve_pr` a · `commit` c

keys: a character, `enter tab esc space left right up down backspace`, or `ctrl+x` `alt+x` `shift+x`.

</details>

**elsewhere** — `TEER_CONFIG` moves the config, `TEER_DATA_DIR` the data (default `~/.local/share/teer`), `TEER_GH` names another `gh`.

## safety

- **the agent only reads.** claude code runs without shell or write tools, codex in its read-only sandbox, a model api with teer's own list, read and grep, kept inside the repo. only recommend may search the web, and only after you allow it.
- **pull requests are untrusted.** their own checkout, no web, and the repo's agent instructions (`CLAUDE.md`, `AGENTS.md`, skills, settings) aren't loaded.
- **code runs only in behavior**, on a separate checkout, in a macos sandbox: no network, no writes outside the checkout, no reads in your home folder beyond the checkout and language toolchains, no keys or tokens in its environment.
- **git runs nothing of the repo's.** hooks, fsmonitor and lfs filters are off for every git command teer runs, except the commit you confirm.
- **nothing is posted without you.** github sees a review only after you confirm it.
- **what leaves your machine** is the changed code, plus what the agent reads in the repo to answer you. reviews, `teer.log`, diagrams and probe runs stay in `~/.local/share/teer`.

## limits

- behavior runs on macos only, with the dependencies your checkout has installed.
- codex's read-only sandbox can read the whole disk, not only the repo. for untrusted prs, prefer claude code or an api.
- the model apis are tested against a fake server, not yet live.
- the small default model can miss a change; `prepare_model = "sonnet"` trades a little speed for depth.
- inline pr comments are placed with teer's own diff. when github rejects a line, teer posts the summary only.
- web search is told to send generic queries only; teer doesn't filter them.
