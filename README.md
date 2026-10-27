# teer

<p align="center">
  <a href="#install">install</a> · <a href="#start">start</a> · <a href="FEATURES.md">docs</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-666666?labelColor=333333" alt="Apache 2.0 license" /></a>
  <a href="https://github.com/julianiff/teer/releases/latest"><img src="https://img.shields.io/github/v/release/julianiff/teer?label=release&labelColor=333333&color=666666" alt="latest release" /></a>
</p>

---


https://github.com/user-attachments/assets/4f66d3b0-057e-49b9-8873-3f277a254ea7



**review a code change by what it does, not line by line.**

teer splits a git change into the functions it touches, puts the risky ones on top, and has [claude code](https://claude.com/claude-code) explain each one, every sentence pinned to the lines that prove it. you accept or flag, write notes, and hand them back to your agent.

- **meaning first** — every unit is sorted into needs a close look `▲`, worth a look `◆` or routine `●`. renames, reformatting and comment-only edits are spotted from the syntax tree and set aside `○`.
- **claims come with their lines** — a fast model (haiku) explains each unit, right in the code under the lines it's about. a sentence that can't cite its lines is dropped, and a second model (sonnet) flags every place it disagrees.
- **facts decide what needs you** — a removed function that's still called, a changed signature, a migration, a payment. what teer sees in the code sets the level; the agent can raise it, never lower it.
- **dig into what you flagged** — get a recommendation, a diagram of what talks to the unit, the tests that cover it, or a sandboxed run of the code before and after the change (macos). `/` asks anything about the code. [flagged screen →](FEATURES.md#flagged-screen)
- **notes back to your agent** — `m` writes a note on any unit. send them all to claude code, copy them as markdown, or post them as inline comments on the pull request. then `r` reviews what changed. [notes →](FEATURES.md#notes-srcnotesrs-srctuinotesrs-srcclipboardrs)
- **pull requests at arm's length** — `teer pr <url>` reviews a pr in a checkout of its own, so your working tree is never touched. the agent can only read, the repo's claude settings aren't loaded, its code only ever runs in a sandbox, and nothing is posted until you've seen it and confirmed. [pull requests →](FEATURES.md#getting-a-review)
- **you know what leaves your machine** — claude code gets the changed code, and read-only access to the repo for your questions. github gets something only when you confirm a post. everything else stays in `~/.local/share/teer/`.
- **one rust binary** — rust, typescript, javascript, python and php are split by function; any other text file is reviewed whole. keyboard first, mouse too. no claude code? `teer --no-agent` still sorts the change.

---

## install

```bash
brew install julianiff/tap/teer
```

or `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/julianiff/teer/releases/latest/download/teer-installer.sh | sh` · `cargo install --locked teer` · [binaries](https://github.com/julianiff/teer/releases)

macos and linux. needs `git`, and [claude code](https://claude.com/claude-code) for the explanations. `gh` adds pull request titles and posting, `nvim` opens a unit with `e`.

## start

in any repository:

```bash
teer                # your uncommitted changes
teer --base main    # this branch, against main
teer pr <url>       # a github pull request
teer pr list        # pick one waiting for your review
```

`l` accepts, `i` flags, `s` swipes one card at a time, `d` digs into what you flagged, `?` shows every key. your place is saved, and after an edit `r` re-runs: only the code that changed starts over. once everything is accepted, teer offers to commit it, or to approve the pr.

## docs

everything is in [FEATURES.md](FEATURES.md): [getting a review](FEATURES.md#getting-a-review) · [the list](FEATURES.md#the-list-view) · [swipe](FEATURES.md#swipe--triage) · [flagged screen](FEATURES.md#flagged-screen) · [ask](FEATURES.md#ask-srctuiaskrs-srcreviewaskrs) · [notes](FEATURES.md#notes-srcnotesrs-srctuinotesrs-srcclipboardrs) · [keys](FEATURES.md#keys) · [configuration](FEATURES.md#configuration) · [persistence and safety](FEATURES.md#persistence-and-safety) · [known limitations](FEATURES.md#known-limitations)

settings live in `~/.config/teer/config.toml`, and every key can be rebound.

## development

```bash
git clone https://github.com/julianiff/teer
cd teer
cargo build --release

cargo test                   # unit, fixture-repo and tui snapshot tests
cargo clippy --all-targets
```

tests that need a real claude code are ignored by default; each one says how to run it. every `claude` command line is in [`src/agent/commands.rs`](src/agent/commands.rs), every prompt in [`src/review/prompts.rs`](src/review/prompts.rs).

## license

teer is licensed under the [Apache License 2.0](LICENSE).
