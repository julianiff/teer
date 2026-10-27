# teer

**Review a code change by what it does, not line by line.**

teer is a terminal app for reviewing git changes, built for the age of agent-written code. It splits a change into the functions, methods and classes it touches, sorts them by how much attention each one needs, and has [Claude Code](https://claude.com/claude-code) explain every one in a few sentences, each sentence backed by the lines that prove it. You go through the list with a few keys and keep notes as you go. Then you hand those notes back to your agent, or post them on the pull request.

```
 teer  shop · uncommitted vs a1b2c3d
╭─ Needs a close look · 1 file ────────────────────────────────────────────╮
│ checkout.py                                                          0/4 │
│▌  ▲ charge_order                                                         │
│   ◆ old_helper                                                           │
│   ● new_helper                                                           │
│   ○ format_me  formatting or comments                                    │
╰──────────────────────────────────────────────────────────────────────────╯
 charge_order  checkout.py:8-12
 ▲ 3 callers · agent: high risk
 ┃ agent (haiku)
 ┃ Changes how orders are charged.

 ╭ code change ─────────────────────────────────────────────────── +3 −2 ╮
 │   8   def charge_order(order):                                        │
 │     ╭────────────────────────────────────╮                            │
 │     │ No longer sends the receipt email. │                            │
 │     ╰────────────────────────────────────╯                            │
 │   9 +     if order.currency not in SUPPORTED:                         │
 │     ╭────────────────────────────────────────────────────────────╮    │
 │     │ Rejects unsupported currencies. 2 callers don't handle it. │    │
 │     ╰────────────────────────────────────────────────────────────╯    │
 ╰───────────────────────────────────────────────────────────────────────╯
 e open in editor   s swipe   ? help   i investigate   / ask   m note
```

*(The name means "arrow" in Hindi and Urdu, a nod to swiping through changes.)*

## Why teer

- **Meaning first.** The units are sorted into **needs a close look** `▲`, **worth a look** `◆` and **routine** `●`. Renames, reformatting and comment-only edits are recognized from the syntax tree and set aside `○`, so they don't use up your attention.
- **Claims come with evidence.** Each statement the agent makes cites lines of the unit. If a statement can't point at its lines, teer drops it. A second model checks every description, and teer shows a warning wherever the two disagree.
- **Facts decide what matters.** Things teer can see in the code decide the level: a removed function that is still called, a changed signature, a big change, something that's expensive to undo (a migration, deleting data, a payment). The agent's opinion can raise the level, never lower it.
- **Built for a loop with your agent.** Write notes on what needs to change. Then hand them to Claude Code, copy them, or post them on the PR, and re-run the review on what changed.

## Install

```sh
brew install julianiff/tap/teer      # once v0.1.0 is released
cargo install --path .               # from a clone, until then
```

macOS and Linux, arm64 and x86_64. You need:

- **git**
- **[Claude Code](https://claude.com/claude-code)** (`claude` on your `PATH`) for the explanations and questions. It's optional: `teer --no-agent` still sorts and shows the changes, without statements.
- Optional: **nvim** to open a unit in your editor with `e`, and the **GitHub CLI** (`gh`) for pull requests.

## Your first review

```sh
cd your/repo
teer                    # your uncommitted changes
teer --base main        # this branch, against main
teer pr <url>           # a GitHub pull request of this repository
teer pr list            # pick a pull request waiting for your review
```

1. **Read the list from the top.** The preview shows why a unit needs attention, the agent's statements written into its code, and which commits touched it.
2. **Decide.** `l` accepts a unit, `i` flags it to investigate, `u` undoes. Or press `s` to swipe through one card at a time.
3. **Write notes.** `m` writes a note on the unit you're looking at, on any screen. `N` shows all your notes.
4. **Dig into what you flagged.** `d` opens the Flagged screen, where you can ask for a recommendation, a picture of what talks to the unit, the tests that cover it, or an actual run of the code before and after the change.
5. **Finish.** Once everything is accepted, teer offers to commit the reviewed files (local changes) or to approve the pull request.

Your decisions are saved. If you reopen teer, or re-run with `r` after editing, you keep your place. Only units whose code changed start over, and their notes stay.

## Notes for your agent

Press `m` on any unit to write a note about its change ("zero amounts still hit the gateway", "why retry here?"). The note shows as a `✎ you` card under the code, and a `✎` marks the unit in the list. `N` lists every note, and from there:

| Key | What happens |
| --- | --- |
| `c` | **Hand to Claude Code.** teer opens Claude Code right in your terminal, with your notes as the first message. Each note comes with the code it's about. Claude works through them, then you quit Claude and press `r` to review what it changed. With `--session <id>`, it continues a fork of the session that wrote the code. |
| `y` | **Copy** all notes as Markdown, to paste into any agent, ticket or chat. |
| `p` | **Post on the pull request** as a review that requests changes or comments (or approves, once everything is reviewed). Notes become inline comments on the changed lines. |

A note on code that has changed since you wrote it says so. Press `m` and `enter` to confirm it still applies, or `d` in the notes view to delete it.

From another terminal, `teer notes` prints the notes of your last review as Markdown. So a Claude Code session in another tab can pick them up with `! teer notes`:

```sh
teer notes              # this repository's last review
teer notes <pr-url>     # a pull request's review
```

## Digging into a change

The **Flagged** screen (`d`) takes the units you flagged one at a time. Ask what you need, and each answer gets a tile of its own next to the code:

- `n` **Recommend**: what to do about the change.
- `v` **Visualize**: a diagram of what calls the unit and what the change adds or removes. It's drawn right in the terminal where the terminal can show images (Ghostty, Kitty, WezTerm, iTerm2). `b` opens it in your browser.
- `c` **Tests**: teer finds the tests that reach the unit at once, without the agent. `a` then has the agent say which ones really cover it, and which dangerous cases none of them do.
- `t` **Behavior** (macOS): the agent writes a small probe, and teer runs it in a sandbox on the code before and after the change. You get a table of what came out, and whether your own tests notice the difference.
- `x` **Disagreement**: the second model's verdict on each statement.
- `/` **Chat** about the unit, with follow-up questions.

Mark each flagged unit `o` ok or `f` needs fix.

Anywhere else, `/` opens **Ask**, for questions about the codebase itself: "who calls this?", "how are retries done?", "show me the Cart class". The answer points at code, which you can open with `enter`.

## Reviewing pull requests

```sh
teer pr https://github.com/owner/repo/pull/123
```

teer fetches the pull request with plain `git` into a checkout of its own. Your working tree, branches and index are never touched. With `gh` installed, the header shows the PR's title, author and size, and the agent reads the PR description as the author's stated intent, and points out a change the description doesn't mention.

A pull request's code is treated as untrusted. teer runs none of it, git hooks are off, the agent can only read inside the checkout and never searches the web, and the repository's own Claude settings and instructions aren't loaded. Nothing is posted to GitHub until you've seen exactly what will be posted and confirmed it. teer also refuses if the PR got new commits since you reviewed it.

## What is sent where

- **Claude Code** gets the code of each changed unit, the commit messages and PR description, and, for your questions, read-only access to the repository. By default the explanations run on a fast, cheap model (`haiku`) and the second opinion on `sonnet`. Questions you ask on the Flagged screen use Claude Code's own default model. All three are configurable. `c` in the notes view is different: it starts an ordinary Claude Code session, with the permissions you've given Claude Code.
- **GitHub** gets something only when you confirm a post (with `gh`, under your account).
- Everything else stays local, under `~/.local/share/teer/`: saved reviews, pictures and probe runs.

## Keys

`?` shows every key in the app. All keys can be rebound.

| Where | Keys |
| --- | --- |
| List | `j` `k` move · `l` accept · `i` flag · `u` undo · `A` accept the whole file · `ctrl+d` `ctrl+u` next/previous file · `]` `[` next/previous undecided · `C` commits and PR description · `r` re-run |
| Swipe (`s`) | `l` / `→` accept · `i` / `←` flag · `h` previous card · `esc` back |
| Flagged (`d`) | `n` `v` `c` `t` `x` ask · `/` chat · `tab` next tile · `z` zoom · `r` ask again · `o` ok · `f` needs fix |
| Everywhere | `m` note · `N` all notes · `/` ask · `e` open in editor · `J` `K` scroll · `?` help · `q` quit |

The mouse works too: click rows, buttons and the keys a pop-up names, and scroll with the wheel. Drag over text to copy it (code comes without its line numbers), or double-click a name, path or number to copy just that.

## Configuration

Optional, in `~/.config/teer/config.toml`. These are the defaults:

```toml
theme = "dark"              # dark | light | mono (NO_COLOR forces mono)
prepare_model = "haiku"     # model that explains units; "sonnet", a full name, or "default"
prepare_effort = "low"      # low | medium | high | xhigh | max | default
second_opinion = true       # a second model checks every explanation
second_model = "sonnet"
second_effort = "low"
concurrency = 4             # agent requests at a time
large_review = 150          # above this many units, explain files as you open them
order = "story"             # within a file: callers before what they call | "line"
images = true               # draw Visualize pictures in the terminal when it can
dive_web = true             # Recommend may search the web (asks first; never for PRs)
behavior_timeout = 120      # seconds a Behavior probe may run, per side

[keys]                      # rebind any action, e.g.
accept = "l"
investigate = "i"
```

The full list of settings and key names is in [FEATURES.md](FEATURES.md#configuration).

## Languages

**Rust, TypeScript (and TSX), JavaScript, Python and PHP** are split into functions, methods and classes with tree-sitter. Any other changed text file (Markdown, YAML, JSON, Go, …) is reviewed as one unit per file, so nothing that changed goes unseen. Lock files and test snapshots are marked routine without asking the agent.

## Learn more

[FEATURES.md](FEATURES.md) describes every screen, key, check and limit in detail.

## Development

```sh
cargo test            # unit, fixture-repo and TUI snapshot tests
cargo clippy --all-targets
cargo fmt
```

Seven tests that need a real Claude Code are ignored by default. Each says how to run it, for example:

```sh
TEER_LIVE_REPO=<repo> TEER_LIVE_SESSION=<session id> cargo test --test live_claude -- --ignored --nocapture
```

Every `claude` command line is in `src/agent/commands.rs`, and every prompt and JSON schema is in `src/review/prompts.rs`.

## License

Apache-2.0, see [LICENSE](LICENSE).
