# teer features

[how it works](#how-it-works) · [start](#start) · [agents](#agents) · [the queue](#the-queue) · [the unit view](#the-unit-view) · [security](#security) · [proposed changes](#proposed-changes) · [finish](#finish) · [the fix loop](#the-fix-loop) · [notes](#notes) · [keys](#keys) · [config](#config) · [safety](#safety) · [limits](#limits)

## how it works

1. **split** — teer parses the old and new version of every changed file and lists the units that changed: functions, methods, classes, top-level code. rust, python, typescript, javascript, php and go are split; any other text file is one whole-file unit. binary, huge and non-utf-8 files are listed as not shown.
2. **sort** — every unit gets a level from what the code shows. the agent can raise it, never lower it. a unit with no behavior change is accepted for you.
3. **explain** — a fast model writes a one-line summary and up to three statements per unit. a statement that doesn't cite lines of its unit is dropped. a second model checks every description.
4. **review** — down the queue, accept (`a`) or flag (`f`), and write a note (`i`) where you have something to say. `l` opens any unit to look closer.
5. **finish** — `:w` reads your decisions and offers one next step: send the notes to your agent, commit, request changes or approve.
6. **fix loop** — when the agent is done with your notes, teer reviews again on its own and puts only the changed units back.

your place is saved on every change: decisions, notes, undo history and the cursor. quit with `q` (or crash), run `teer` again, and you're on the same unit. a re-run keeps every decision, answer and note for units whose code didn't change.

### levels

| | level | when |
| --- | --- | --- |
| `▲` | needs a close look | removed but still called · the agent rates it high risk · a [security](#security) fact that needs it (an auth check gone, a secret, `pull_request_target`) · a high security finding that stands |
| `◆` | worth a look | signature changed and it has callers · 40+ changed lines · medium risk · the second model disputes it · expensive to undo · couldn't be described · a security fact (a new route, a sink reachable from one) · a medium security finding that stands |
| `●` | routine | everything else that changes behavior · removed with no callers · lock files, snapshots |
| `○` | no behavior change | formatting or comments · renamed locals · renamed with the same body |

`○` units are accepted for you and left out of the queue; `za` shows them, `u` on one makes it pending again. `auto_accept = "none"` turns this off, and they wait as routine.

### reasons

besides the rule that set its level, a unit can have reasons to look, each a sentence under "why this level":

- **expensive to undo** — deletes or rewrites data, changes a schema, sends, charges or publishes, changes a public api. files under `migrations/` always count.
- **the second model disputes** a statement: the statement, quoted, and what it says instead. this one also shows as `⚠` on the unit's row.
- **not mentioned in the commits or pr description** — a behavior change nobody wrote down.
- **security** — each security fact teer found in the change, or another tool reported (see [security](#security)), prefixed `security:`.

## start

| | |
| --- | --- |
| `teer` | your uncommitted changes, staged, unstaged and untracked. with none, this branch since its base. with none on the default branch, there's nothing to review |
| `teer <url>` | a github pull request of this repo |
| `teer 482` | pull request 482 of this repo |
| `teer pr` | pick a pr waiting for your review, assigned to you, or yours |
| `teer notes [url]` | print the notes of the last review as markdown |
| `teer notes --sarif [url]` | print the last review's security facts and findings as sarif |

| flag | |
| --- | --- |
| `--base <ref>` | review this branch since `<ref>`, instead of the base teer finds |
| `--no-agent` | no agent: sort the change and review it yourself |
| `--json` | prepare the review, print it as json, exit |
| `--sarif <file>` | pin another tool's sarif findings to the units they fall in (repeat it for more files) |

**the base** — `--base` if given, else the base of this branch's pull request if `gh` knows it, else `origin/HEAD`, else `main` or `master`. the queue's header names it: "feature since main, 6 commits".

**pull requests** are fetched with plain git into a checkout teer keeps for itself, so your working tree is never touched. `gh` adds the title, author and description; without it teer reads the commits. new commits on the pr keep your decisions for every unit that didn't change.

**the session that wrote it** — with claude code, teer looks for the session that made the change: the newest one in this repo that edited the changed files in the last 24 hours. `K` asks a fork of it, which knows why the code changed, and your notes go to a fork of it too. `--session <id>` names one instead.

**first start** — without a config, teer asks which agent and models to use, then goes straight on into the review. `teer setup`, or `<space>s` in a review, changes them later.

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

the advice, `K`, the tools and commit messages use the cli's own default model, or the second model with an api.

setup also checks what else teer needs: git, gh (and whether it's logged in), your editor and the macos sandbox. before saving, the fast model gives a test answer. api keys never go into the config, only the name of the variable that holds one.

## the queue

what teer opens on. one box per level, most critical first; in each, units by file, callers before callees (`↳`). the cursor starts on the first pending unit.

- **header** — what you review ("your changes", "feature since main, 6 commits", "PR #482 by ana") and a line about the whole change. `<space>c` shows the pr's title and description and every commit, with its message and the units it changed.
- **rows** — level, file, unit, `⚠` when the second model disputes it, and where it stands: pending, accepted, or flagged with the start of its note.
- **accepted for you** — a line under the boxes: "14 units accepted for you, za shows them".
- **preview** — beside the list from 110 columns, else below: why the unit got its level, the agent's summary, and the change with each statement written under the lines it cites. `<c-d>` `<c-u>` scroll it.
- **bottom bar** — `4 pending · 1 flagged · 37 accepted`, then a key sequence you started and what can follow it, `-- VISUAL --`, or the `:` line.

deciding:

- `a` accepts, `f` flags; neither asks for anything. both go to the next pending unit, not the next row. when nothing is pending, the bar says "Nothing pending. :w to finish".
- `A` and `F` do the same for the pending units of the file under the cursor (listed by level, the ones in its box), never for ones you decided.
- a count decides more: `3a` accepts this unit and the next two pending ones.
- `V` starts a selection, `j` `k` grow it, `a` or `f` decide every unit in it, `esc` leaves it.
- `u` undoes the last decision, `<c-r>` redoes it, `.` repeats it on the unit under the cursor, with its count. the undo history is saved with the review.
- `i` opens a note line at the bottom, with the note so far, without changing the decision: `enter` saves it (empty is no note), `esc` drops the edit.

moving: `n` `N` go to the next and previous pending unit, `]f` `[f` to the next and previous file, `/` finds a unit by file or name. `zf` lists by file instead: each file in the box of its most critical unit.

**mouse** — click to select, wheel to scroll, drag to copy, double-click to copy a word (`src/app.rs:120` and urls count as one).

## the unit view

`l` or `enter` opens any unit, flagged or not. it's written before you get there, so you read first:

1. **title** — level, file, unit, where it stands.
2. **why this level** — the rule that set it and the [reasons](#reasons).
3. **what changed** — the summary, then the diff in its box with each statement under the lines it cites. `]c` `[c` jump between hunks.
4. **second model** — its verdict on every statement, only when it disputes one.

beside the text, two tiles are open from the start:

- **advice** — what the agent says to do about the unit: concrete steps, most important first, each with its reason. nothing is asked until you say so: `<space>a` (here or from the queue), `enter` on the tile, or a click on its question. the answer is kept with the unit until its code changes; who wrote it is on the tile's bottom edge.
- **keys** — what you can do here, by group (decide, look closer, move, finish). click one to run it; `?` lists every key.

with `--no-agent`, the second model and the advice are left out, and so are the keys that need an agent.

each of these opens a tile, tmux style, next to the ones already open: the text keeps a tall column on the left (on top in a narrow terminal), the tiles share the rest in the order you opened them, the keys tile under them. the new tile gets the focus, and the scroll keys go to whatever has it. `tab` `shift+tab` (`gt` `gT`) move the focus, the text among the tiles; `zz` zooms what has it; `h` closes it. the same key on an open tile gives it the focus, and in focus closes it:

| | |
| --- | --- |
| `<space>a` | the advice, asked for now (see above). in focus with an answer, closes it |
| `K` | ask anything; "this" is the unit (from the queue, `K` opens it first). while the agent works you see each step it takes (the file it reads, what it searches), then the answer as it's written (claude code and model apis; codex shows the steps and the answer once done). the answer brings the code it talks about and the units it relates to: `tab` moves to them, `enter` opens one. follow-ups stay in the tile; the conversation is kept with the unit. `esc` stops typing and leaves the tile open; `K` or `i` types again |
| `<space>t` | the tests that reach the unit, found by teer at once without an agent. with one, the tile then asks whether the agent reads them: `enter` (or a click on the question) and it says what each checks, and the dangerous cases none covers. with none found, it offers to look for tests itself |
| `<space>r` | old and new side by side: the agent writes a small program, teer runs it before and after the change in a sandbox, then checks the statements against the runs and whether your tests notice the change. macos only |
| `<space>v` | what talks to the unit and what the change adds or removes, as a diagram in the terminal (kitty, ghostty, iterm2, sixel). `gx` opens it in the browser |
| `<space>x` | security: how the change can be abused, each way with its path, checked by a second model; `<space>p` then runs each attacker's case old against new. see [security](#security) |
| `<space>e` | the agent proposes a change, shown as hunks under the code change; `p` puts one in, `P` all. see [proposed changes](#proposed-changes) |
| `gd` | what the unit calls: in the review, a list to jump to; outside it, the definition, read-only |
| `gr` | what calls the unit, to jump to |
| `gf` | your editor, at the unit's first changed line |

`a` `f` `A` `F` `u` `.` work as in the queue; after `a` or `f` the next pending unit opens here. `n` `N` move without deciding, and close the tiles but the advice. `h` on the text goes back to the queue, on the unit you were looking at.

**flagged units** — `<space>f`, here or in the queue, lists every unit you flagged one after another, each as the queue's preview shows it: why it has its level, the agent's summary, the whole change in its box with the statements written in, your note under it. one is selected, with the `▌` bar and a background behind it. `j` `k` (or `n` `N`) select the next and previous one and scroll it to the top, a click selects one, `]c` `[c` jump from hunk to hunk, `<c-d>` `<c-u>` scroll. `l` or `enter` looks closer at the selected one (`h` comes back), `a` accepts it and it leaves the list, `i` writes its note, `h` or `<space>f` goes back.

like statements, every claim in the advice and the tools must cite real lines. what doesn't is dropped and logged to `teer.log`.

## security

a lens over the review, not a scanner of the whole repo: teer finds security facts in the change itself, the agent looks for ways to abuse it only when you ask, every finding has to show its path, and you decide. no finding is never the same as safe: what was checked and what wasn't is always on screen.

**facts, without an agent.** at prep, teer reads every change for security facts and puts each under "why this level", prefixed `security:`. in php, typescript/javascript and go:

- an authorization check gone (`denyAccessUnlessGranted`, `#[IsGranted]`, `@UseGuards`, `requireAuth` middleware…): `▲` when the code is reachable
- a new action or route without the check all its siblings have: `◆`
- new routes, handlers, commands and server actions: `◆`
- raw sql, shell, eval, file paths, outgoing requests or unescaped html given something that isn't a literal: `◆` when reachable from an entry point (up to four calls up), else a note

tests, fixtures and mocks are no entry points and reach no sinks: only real keys count there.

in every file:

- hard-coded secrets: `▲`, the value never shown
- passwords or tokens logged
- packages added or bumped, install scripts (`▲`), a lock changed without its manifest
- `pull_request_target` and `${{ github.event… }}` in `run:` (`▲`), wider permissions, actions not pinned to a commit
- open ingress and public buckets in terraform
- cors `*`, cookie flags off, tls verification off, debug on
- weak crypto: a note

**the security order.** `zs` puts the units with security facts or standing findings in a security box above the level boxes, most exposed first, under an attack-surface card: the entry points, sinks, removed or missing checks, secrets, packages, ci and config the change touches, most severe first, and on its last line what teer checked and what nobody did. with nothing found the card is one muted line that still says what was checked. on a short terminal it is one line; a click on an item selects its unit. `zs` again lists by level, without the card: there each fact is a reason under its unit.

**the security tile (`<space>x`).** asks the agent how the change can be abused. teer gives it the call chains into the unit from its call graph (up to four calls to the entry points), the checks its catalog finds along them, what the unit calls, and the facts teer found, as hints, not conclusions. each finding must show a path: where untrusted input enters, each call on the way, and where it does harm, every step citing real lines.

- a finding whose lines don't exist, or whose path doesn't pass through the unit, is dropped and counted
- a path whose calls teer's graph can't confirm shows "path not verified" and never raises the level
- a second model checks each finding against the cited code, which teer reads itself; a disputed finding shows `⚠` and doesn't count
- a finding that stands raises its unit: high `▲`, medium `◆`

the tile also lists what holds (controls checked and found in place) and what was checked and what wasn't. denial of service, rate limits, resource exhaustion, input validation without impact and open redirects without a chain are left out unless `security.toml`'s `include` names them. until the agent answers, the tile is only as tall as teer's facts; without an agent it shows them alone.

**prove it (`<space>p`).** on a unit whose security tile has findings, the agent writes an attacker's case for each finding not run yet (another tenant's id, `../` in a path, a crafted payload), and teer runs them old against new in the sandbox with the behavior probe runner. each finding then reads **proven** (the case came out as the agent predicted before anything ran), **not shown** (it ran and didn't), or **unproven** with why (no macos sandbox, no probe that works, or it needs a database or a framework kernel). the probe and both runs are kept under `~/.local/share/teer/proofs/`. the agent stays read-only; the probe never touches your checkout.

**other tools' findings.** `teer --sarif codeql.sarif` (repeat it for more files) pins every result of a sarif 2.1.0 file, from codeql, semgrep, trivy or claude security, to the unit it falls in, as a security reason: a `security-severity` of 7 or an `error` needs a close look, 4 or a `warning` is worth a look. results outside the change are left out and counted in `teer.log`; a re-run reads the files again. reviewing a pull request, teer also asks github (`gh`) for the code-scanning alerts open on its head and pins them the same way; without code scanning, permission or `gh`, the review goes on without them. imports only add reasons, they never lower a level.

**finishing.** in the flagged list, a flagged unit's most severe finding or fact sits under its name with its severity. the notes you copy (`y`, `teer notes`) or hand to your agent carry each flagged unit's findings, with the path from where the input enters to where it does harm, what's missing and how it's exploited. a pull request review gets a security section, and each finding goes under its flagged unit's inline note. `teer notes --sarif` prints teer's own facts and the standing findings as sarif 2.1.0 (one rule per class, each finding's path as a code flow), for github code scanning or another tool.

**your own names** go in `security.toml`, next to the config (`~/.config/teer/security.toml`), per language. teer never reads it from the repository under review.

```toml
include = ["open_redirect"]       # redirects count like other sinks
[php]
checks = ["MyVoter::vote"]        # your voters and guards
entry = ["AsWebhook"]             # attributes or decorators that make an entry point
sinks.sql = ["Db::raw"]           # also: shell, eval, file, http, template, redirect, log, llm
```

`enabled = false` turns the lens off: no facts, no card, no security tile.

## proposed changes

when something important has to change in a unit, the agent proposes the code, and you put it in from the unit's text.

**what it answers.** the most severe security finding that stands; else your note; else the advice. `<space>e` asks; with nothing to fix it says how to get something. once a high security finding stands (its path verified, the second model agreeing), teer asks on its own; `propose_fixes = "never"` turns that off.

**what teer checks.** the agent only reads: it answers with exact text to replace, at most 8 hunks, each with one line on why. teer keeps a hunk only when:

- its text is in the file exactly once, as reviewed
- the file is inside the repository, never under `.git`
- it has no control characters and adds no bidi overrides
- it doesn't overlap another hunk

what it drops is counted and logged to `teer.log`. a hunk whose file has a new syntax error with it says "doesn't parse", for the languages teer splits. a second model then says whether the change fixes what it should and changes nothing else.

**in the unit's text.** under the code change, **proposed change**: what it answers and who wrote it, the summary, the second model's word, then each hunk in a box like the code change: its file and lines, its state (proposed, put in, suggested), why, and the lines it removes and adds. `]c` `[c` stop at each after the code change's hunks. the one you jumped to, else the one nearest the top, has the accent border and `p put · P put all` on its edge. the queue's preview says `✎ a change is proposed: 2 hunks · ␣e shows it`.

**putting it in.** `p` puts the current hunk into your working tree, `P` every open one, after a y/n question. each hunk's text is found again on disk first; if a file changed since, nothing is written. files are written whole and atomically, keeping their permissions, and symlinks are never followed. then teer reviews again and says what the change did, e.g. "1 unit left the review by the proposed change: as at the base again".

**on a pull request** nothing is written. `p` and `P` make the hunks suggestions and flag the unit (`u` takes that back). on `:w` a suggested hunk is posted as a github suggestion on its lines when they're in the pr diff, so the author can commit it with one click; it replaces whole lines even when the change starts mid-line, and joins your note when that sits on the same lines. a hunk outside the diff, in another file, or past the 50 inline comments goes into the summary's "proposed changes" as a diff, and so does every suggestion when github refuses inline comments. hunks you never suggested stay out of the post.

**in the notes.** the notes you copy (`y`, `teer notes`) or hand to your agent carry each proposed change not put in yet: what it answers, what it does, the second model's word, and each hunk as a diff. your agent is asked to put them in, or say why not. the flagged list says `✎ change proposed: 2 hunks` under a unit that has some.

## finish

`:w` opens one sheet that reads your decisions and offers one next step. `enter` does it; `j` `k` pick another.

| reviewing | state | `enter` | or |
| --- | --- | --- | --- |
| your changes | something flagged | send the notes to your agent | yank the notes · commit anyway |
| your changes | all accepted | commit, with a message the agent drafts | yank the notes |
| a branch | something flagged | send the notes to your agent | yank the notes |
| a branch | all accepted | done | yank the notes |
| a pull request | something flagged | request changes: notes inline, plus a summary | comment only · yank the notes |
| a pull request | all accepted | approve | approve with a comment · yank the notes |

- with units still pending, the sheet says how many first, the step is "keep reviewing", and `n` goes to the first pending unit. the other steps ask before they run.
- the sheet lists the flagged units with their notes.
- `y` copies the notes as markdown, each with its unit's diff.
- `i` edits the commit message or the review summary in your editor.
- `esc` or `q` closes it and does nothing.
- a commit or a post ends teer with one line saying what it did.

**commit** — teer commits exactly the files you reviewed, with your hooks and signing, and refuses when one changed since.

**post** — notes become inline comments where their line is in the pr diff; the rest go in the summary. teer checks the pr still has the head you reviewed, posts with `gh` only after `enter`, and opens the pr page if posting fails.

## the fix loop

flag what's wrong, say in the note what should change, then `:w` `enter`:

1. teer opens claude code or codex in your terminal with the notes as its first message: with claude code, in a fork of the session that wrote the change when teer found one.
2. when you leave the agent, teer reviews again at once. no key to press.
3. units whose code changed are pending again, on top of their level, with your note kept and marked "code changed since this note". the rest keep your decision.
4. the cursor lands on the first changed unit, and the bar says how many changed: "3 units changed by Claude Code".

on a note marked changed, `a` accepts and archives the note, `f` keeps the flag and edits it, `i` edits it without deciding. on a branch the agent doesn't have to commit: a branch review covers the branch's commits and whatever isn't committed yet.

**notes teer doesn't hand over** — a model api has no session to take them, so `:w` `enter` copies the notes, with what to do, for you to paste into your agent; `y` copies them too. teer then waits, with the queue on screen and read-only (no deciding, no `:w`), and reviews again once your files have been quiet for 5 seconds (`quiet_seconds` under `[fix_loop]`). `q` stops waiting.

**`:e`** reviews again now and keeps your decisions; on a pull request it fetches the head first, so a pushed fix shows up. **`:e!`** asks "Reset all 42 decisions? y/n", then reviews again with every unit pending, and decides afresh which ones are accepted for you.

pull requests have no fix loop: the notes go on the pr.

## notes

`f` writes a note as it flags; `i` edits one any time. `<space>n` lists them all: `enter` goes to the unit, `i` edits, `d` deletes, `y` copies them as markdown, `:w` finishes.

`teer notes` prints the same markdown for any agent: each note with its file, lines and diff. a note stays when its code changes, marked as such until you act on it.

## keys

keys follow neovim, and a key does the same on every screen. a prefix (`g`, `]`, `[`, `z`, `<space>`) waits `timeoutlen` milliseconds (default 1000) for the next key and lists what can follow. digits before a key are a count (`3a`, `5j`); `esc` drops one. `:` opens a command line for `:w`, `:e`, `:e!` and `:q`.

| keys | action id | does |
| --- | --- | --- |
| `j` `<down>` | `move.down` | move down |
| `k` `<up>` | `move.up` | move up |
| `gg` `<home>` | `move.first` | first unit |
| `G` `<end>` | `move.last` | last unit |
| `n` | `move.next_pending` | next pending unit |
| `N` | `move.prev_pending` | previous pending unit |
| `]c` | `move.next_hunk` | next hunk |
| `[c` | `move.prev_hunk` | previous hunk |
| `]f` | `move.next_file` | next file |
| `[f` | `move.prev_file` | previous file |
| `<tab>` `gt` | `move.next_tile` | next tile |
| `<s-tab>` `gT` | `move.prev_tile` | previous tile |
| `<c-d>` | `scroll.half_down` | scroll half a page down |
| `<c-u>` | `scroll.half_up` | scroll half a page up |
| `<pagedown>` | `scroll.page_down` | scroll a page down |
| `<pageup>` | `scroll.page_up` | scroll a page up |
| `/` | `find.unit` | find a unit by file or name |
| `a` | `decide.accept` | accept the unit |
| `f` | `decide.flag` | flag the unit |
| `i` | `note.edit` | edit the note |
| `A` | `decide.accept_file` | accept the file's pending units |
| `F` | `decide.flag_file` | flag the file's pending units |
| `V` | `visual.start` | select lines |
| `u` | `history.undo` | undo the last decision |
| `<c-r>` | `history.redo` | redo |
| `.` | `history.repeat` | repeat the last decision |
| `l` `<cr>` | `view.open` | open |
| `h` `<esc>` | `view.back` | back |
| `K` | `ask.open` | ask about the unit |
| `gd` | `goto.definition` | what this unit calls |
| `gr` | `goto.callers` | what calls this unit |
| `gf` | `goto.editor` | open in the editor |
| `gx` | `goto.browser` | open the picture in the browser |
| `za` | `toggle.auto_accepted` | show units accepted for you |
| `zf` | `toggle.by_file` | list by file or by level |
| `zz` | `toggle.zoom` | zoom the tile in focus |
| `zs` | `toggle.security` | list by level or by exposure |
| `<space>a` | `tool.advice` | advice: what to do about the unit |
| `<space>t` | `tool.tests` | tests that cover this unit |
| `<space>r` | `tool.run_old_new` | run old and new |
| `<space>v` | `tool.diagram` | diagram of the change |
| `<space>x` | `tool.security` | security: how the change can be abused |
| `<space>e` | `tool.fix` | propose a change: the agent writes it, teer checks it |
| `p` | `fix.apply` | put the proposed hunk into the code |
| `P` | `fix.apply_all` | put every proposed hunk into the code |
| `<space>p` | `tool.prove` | prove the security findings, old against new |
| `<space>c` | `tool.history` | commits and PR description |
| `<space>n` | `tool.notes` | every note |
| `<space>f` | `tool.flagged` | flagged units, one after another |
| `<space>s` | `tool.settings` | settings |
| `:app` | `finish.app` | open the review in Teer.app |
| `:w` | `finish.open` | finish |
| `y` | `finish.yank` | copy the notes |
| `:e` | `review.rerun` | re-run, decisions kept |
| `:e!` | `review.reset` | re-run and reset every decision |
| `q` `:q` | `view.close` | close; in the list, quit |
| `?` | `help.open` | keys |
| `d` | `note.delete` | delete the note |

most keys work in the queue and the unit view alike. `]c` `[c` work in the unit view and the flagged list; `gx`, the tile keys and `zz` are the unit view's, `V` `za` `zf` the queue's; `y` works on the `:w` sheet and in the notes, `d` in the notes. `?` shows the keys of the screen you're on, as you bound them.

typing a note, a question or a message, `enter` saves, `esc` cancels and `<c-u>` clears the line. a yes/no question takes `y` or `enter` for yes, any other key for no. `<c-c>` quits from anywhere, saved.

**rebinding** — `[keys]` in the config takes an action id and a key, or a list of keys, in neovim notation: `<cr>` enter, `<esc>`, `<tab>`, `<s-tab>`, `<space>`, `<bs>`, `<up>` `<down>` `<left>` `<right>`, `<home>` `<end>` `<pageup>` `<pagedown>`, `<c-x>` ctrl, `<a-x>` alt, `<lt>` for `<`, and `:cmd` for a command. keys you bind replace the action's own; a key another action had by default is taken from it, with a warning. an unknown action id is warned about and skipped. teer refuses a key bound twice, or one that starts another (`g` can't be bound while `gg` is).

```toml
timeoutlen = 500

[keys]
"goto.editor" = ["gf", "e"]       # a list keeps gf
"history.redo" = "U"              # <c-r> no longer redoes
```

## config

`~/.config/teer/config.toml`. a bad value stops teer at start and says why.

```toml
theme = "light"
editor = "code --wait --goto {file}:{line}"
second_opinion = false

[keys]
"goto.editor" = ["gf", "e"]
```

| key | default | |
| --- | --- | --- |
| `provider` | set by setup | `claude`, `codex` or `api` |
| `prepare_model` | per agent | the fast model; `default` is the cli's own |
| `second_model` | per agent | the second model |
| `prepare_effort` · `second_effort` | `low` | how hard each model thinks |
| `second_opinion` | `true` | check every description with the second model |
| `propose_fixes` | `high` | the agent proposes a change on its own when a high security finding stands, or `never` (`<space>e` always asks) |
| `[api]` | | `format` (`anthropic` or `openai`), `base_url`, `key_env` (the variable holding the key) |
| `editor` | `nvim +{line} {file}` | what `gf` runs, and `i` on the `:w` sheet; `{file}` and `{line}` are filled in |
| `theme` | `dark` | `dark`, `light` or `mono` (`NO_COLOR` means mono) |
| `order` | `story` | callers and callees together, or `line` |
| `auto_accept` | `no_behavior_change` | accept units with no behavior change for you, or `none` |
| `languages` | all six | which ones are split into functions |
| `concurrency` | `4` | agent requests at once |
| `large_review` | `150` | above this many units, a file is described when you reach it |
| `dive_web` | `true` | the advice and `K` may search the web, after asking once; never for a pr |
| `images` | `true` | draw diagrams in the terminal |
| `behavior_timeout` | `120` | seconds per old or new run of `<space>r` |
| `timeoutlen` | `1000` | milliseconds a prefix waits for the next key |
| `[fix_loop]` | | `quiet_seconds` (`5`): after you copied the notes, how long the files must be quiet before teer reviews again |
| `[keys]` | | `"action.id" = "key"` or a list, see [keys](#keys) |

**editors** — a gui editor needs its wait flag, or commit messages and review text come back unedited; `gf` then waits until you close the tab:

```toml
editor = "code --wait --goto {file}:{line}"   # vs code
editor = "idea --wait --line {line} {file}"   # intellij
editor = "zed --wait {file}:{line}"           # zed
```

**elsewhere** — `TEER_CONFIG` moves the config, `TEER_DATA_DIR` the data (default `~/.local/share/teer`), `TEER_GH` names another `gh`.

## safety

- **the agent only reads.** claude code runs without shell or write tools, codex in its read-only sandbox, a model api with teer's own list, read and grep, kept inside the repo. only the advice and `K` may search the web, and only after you allow it.
- **teer writes only what you put in.** a proposed change is text the agent answered with. teer writes it to your working tree only on `p` or `P` and your `y`, only files inside the repository (never `.git`, never through a symlink), all of them or none, and never for a pull request.
- **the fix loop is your agent.** `:w` opens claude code or codex in your terminal as if you had started it, with its usual permissions. never for a pull request.
- **pull requests are untrusted.** their own checkout, no web, and the repo's agent instructions (`CLAUDE.md`, `AGENTS.md`, skills, settings) aren't loaded.
- **code runs only in `<space>r`**, on a separate checkout, in a macos sandbox: no network, no writes outside the checkout, no reads in your home folder beyond the checkout and language toolchains, no keys or tokens in its environment.
- **git runs nothing of the repo's.** hooks, fsmonitor and lfs filters are off for every git command teer runs, except the commit you confirm.
- **security rules are yours.** `security.toml` is read from teer's config folder, never from the repository under review, so a pull request can't switch off its own checks. facts and imported findings only ever raise a level.
- **nothing is posted without you.** github sees a review only after `enter` on the `:w` sheet.
- **what leaves your machine** is the changed code, plus what the agent reads in the repo to answer you. reviews, `teer.log`, diagrams and probe runs stay in `~/.local/share/teer`.

## limits

- `<space>r` runs on macos only, with the dependencies your checkout has installed.
- codex's read-only sandbox can read the whole disk, not only the repo. for untrusted prs, prefer claude code or an api.
- the model apis are tested against a fake server, not yet live.
- the small default model can miss a change; `prepare_model = "sonnet"` trades a little speed for depth.
- inline pr comments are placed with teer's own diff. when github rejects a line, teer posts the summary only.
- web search is told to send generic queries only; teer doesn't filter them.
- security facts come from a catalog of names per language (php, typescript/javascript, go; rust and python get secrets, packages, ci and config only). a check done somewhere else (middleware, a voter elsewhere) can show as gone; the security tile is where that gets sorted out.
- prove it runs on macos only, like `<space>r`, and only for units that run on their own.
- code-scanning alerts: the first 100 per pull request. in a pr review, sarif paths must be relative to the repo.
