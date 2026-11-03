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

teer splits a git change into the functions it touches, puts the risky ones on top, and has an agent explain each one, every sentence pinned to the lines that prove it. you accept each one, or flag it with a note. then `:w` hands your notes to your agent and reviews what it changed, commits, or posts your review on the pull request.

- **risky first** — every function lands in `▲` close look, `◆` worth a look or `●` routine. facts from the code set the level; the agent can only raise it. what doesn't change behavior is accepted for you.
- **no claim without its lines** — a fast model explains each function right in the code, a second model checks it. a sentence that can't cite its lines is dropped.
- **answers before you ask** — open a function and it says why it got its level, what changed and what to do about it. `K` asks anything; its tests, old and new run side by side in a sandbox, or a diagram are a key away.
- **a fix loop** — `f` flags with a note, `:w` sends the notes to claude code or codex, and once the agent is done teer reviews again, with only the changed functions back in the queue.
- **pull requests at arm's length** — `teer <url>` reviews in a checkout of its own. the agent only reads, the repo's agent instructions stay out, nothing is posted until you confirm.
- **neovim keys** — a key means the same on every screen. counts, `.`, `V`, undo and redo; every key can be rebound.
- **any agent, or none** — [claude code](https://claude.com/claude-code), [codex](https://developers.openai.com/codex), or a model api with your key (anthropic, openai, openrouter, ollama). `--no-agent` still sorts the change.

## install

```bash
brew install julianiff/tap/teer
```

or `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/julianiff/teer/releases/latest/download/teer-installer.sh | sh` · `cargo install --locked teer` · [binaries](https://github.com/julianiff/teer/releases)

macos and linux. needs `git` and an agent; `gh` adds pull requests. `teer setup` checks what's there.

## start

```bash
teer                # your changes, or this branch since its base
teer <url>          # a github pull request
teer 482            # pull request 482 of this repo
teer pr             # pick one waiting for you
```

the first start asks which agent and models to use; `teer setup` changes it.

**review what your agent wrote** — `teer` → `a` or `f` down the queue → `l` on a `▲` → `K` to ask → `f` with a note → `:w` `enter` sends the notes to your agent → teer reviews again when it's done → `a` the changed units → `:w` `enter` commits.

**review a teammate's pull request** — `teer pr` → `a` or `f` down the queue → `l`, `K` to ask → `f` to note the problem → `:w` `enter` requests changes, or approves when nothing is flagged.

| | |
| --- | --- |
| `a` · `f` | accept · flag with a note, then on to the next pending unit |
| `A` · `F` | the same for the file's pending units |
| `l` · `h` | open the unit · back |
| `K` | ask about it |
| `i` | edit the note |
| `u` · `<c-r>` | undo · redo |
| `/` | find a unit |
| `:w` | finish: send the notes, commit, approve |
| `?` | every key |

your place is saved on every change.

## config

`~/.config/teer/config.toml`:

```toml
theme = "light"                                # dark, light, mono
editor = "code --wait --goto {file}:{line}"    # default: nvim +{line} {file}

[keys]
"goto.editor" = ["gf", "e"]                    # action id = keys, in neovim notation
```

## coming from v1

one queue replaces the list, swipe and the flagged screen, `:w` replaces the finish screens, and keys follow neovim. your config and saved reviews carry over: old `[keys]` names still work, teer warns once about each one that no longer exists, and writes nothing back. units you flagged or resolved on the flagged screen are flagged, with their notes.

| v1 | v2 |
| --- | --- |
| `l` · `i` accept · flag | `a` · `f`, which opens the note |
| `s` swipe | the queue: `a` and `f` move on to the next pending unit |
| `d` the flagged screen | `l` opens any unit |
| `n` recommend · `x` disagreement | shown in the unit view |
| `c` tests · `a` the agent reads them | `<space>t`, the agent reads them on its own |
| `t` behavior · `v` visualize · `b` browser | `<space>r` · `<space>v` · `gx` |
| `o` · `f` resolve | gone: a flag and its note say it |
| `tab` `z` `r` tiles, zoom, ask again | `tab` `zz` in the unit view; asking again is `<c-r>` while typing |
| `/` ask and chat | `K`; `/` finds a unit |
| `m` note · `N` all notes | `i` · `<space>n` |
| `c` `a` `p` hand over, commit, approve, post | `:w` |
| `e` editor | `gf` |
| `r` · `R` re-run · reset | `:e` · `:e!` |
| `J` `K` scroll | `<c-d>` `<c-u>` |
| `g` · `]` `[` top · pending | `gg` · `n` `N` |
| `ctrl+d` `ctrl+u` next file | `]f` `[f` |
| `f` by file | `zf` |
| `C` commits | `<space>c` |
| `teer --base main` | `teer` finds the base; `--base` still picks one |
| `teer pr list` · `teer pr <url>` | `teer pr` · `teer <url>` |

## docs

[FEATURES.md](FEATURES.md): [how it works](FEATURES.md#how-it-works) · [start](FEATURES.md#start) · [agents](FEATURES.md#agents) · [the queue](FEATURES.md#the-queue) · [the unit view](FEATURES.md#the-unit-view) · [finish](FEATURES.md#finish) · [the fix loop](FEATURES.md#the-fix-loop) · [notes](FEATURES.md#notes) · [keys](FEATURES.md#keys) · [config](FEATURES.md#config) · [safety](FEATURES.md#safety) · [limits](FEATURES.md#limits)

## development

```bash
git clone https://github.com/julianiff/teer
cd teer
cargo build --release

cargo test                   # unit, fixture-repo and tui snapshot tests
cargo nextest run            # the same, each test in its own process, with timings
cargo clippy --all-targets
```

the integration tests are one binary: each file in `tests/it/` is a module of `tests/it/main.rs`. run a part of them by module path, e.g. `cargo test --test it pr::`.

tests that need a real agent are ignored by default; each one says how to run it. the `claude` and `codex` command lines are in [`src/agent/commands.rs`](src/agent/commands.rs), the model api client in [`src/agent/api.rs`](src/agent/api.rs), the prompts in [`src/review/prompts.rs`](src/review/prompts.rs).

## license

[Apache 2.0](LICENSE)
