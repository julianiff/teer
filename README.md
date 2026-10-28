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

teer splits a git change into the functions it touches, puts the risky ones on top, and has an agent explain each one, every sentence pinned to the lines that prove it. you accept or flag, take notes, and hand them back to your agent.

- **risky first** — every function lands in `▲` close look, `◆` worth a look, `●` routine or `○` no behavior change. facts from the code set the level; the agent can only raise it.
- **no claim without its lines** — a fast model explains each function right in the code, a second model checks it. a sentence that can't cite its lines is dropped.
- **dig into what you flag** — a recommendation, a diagram of what calls it, the tests that cover it, or a sandboxed run before and after the change. `/` asks anything.
- **notes back to your agent** — `m` notes any function. hand them to claude code or codex, copy them as markdown, or post them on the pr.
- **pull requests at arm's length** — `teer pr <url>` reviews in a checkout of its own. the agent only reads, the repo's agent instructions stay out, nothing is posted until you confirm.
- **any agent, or none** — [claude code](https://claude.com/claude-code), [codex](https://developers.openai.com/codex), or a model api with your key (anthropic, openai, openrouter, ollama). `--no-agent` still sorts the change.

## install

```bash
brew install julianiff/tap/teer
```

or `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/julianiff/teer/releases/latest/download/teer-installer.sh | sh` · `cargo install --locked teer` · [binaries](https://github.com/julianiff/teer/releases)

macos and linux. needs `git` and an agent; `gh` adds pull requests. `teer setup` checks what's there.

## start

```bash
teer                # your uncommitted changes
teer --base main    # this branch against main
teer pr <url>       # a github pull request
teer pr list        # pick one waiting for you
```

the first start asks which agent and models to use; `teer setup` changes it.

| | |
| --- | --- |
| `l` · `i` | accept · flag |
| `s` | swipe, one card at a time |
| `d` | dig into what you flagged |
| `/` | ask about the code |
| `m` · `N` | note · all notes |
| `e` | open in your editor |
| `r` | re-run after an edit |
| `?` | every key |

your place is saved. once everything is accepted, teer offers to commit it, or to approve the pr.

## config

`~/.config/teer/config.toml`:

```toml
theme = "light"                                # dark, light, mono
editor = "code --wait --goto {file}:{line}"    # default: nvim +{line} {file}

[keys]
accept = "y"                                   # every key can be rebound
```

## docs

[FEATURES.md](FEATURES.md): [how it works](FEATURES.md#how-it-works) · [commands](FEATURES.md#commands) · [agents](FEATURES.md#agents) · [the list](FEATURES.md#the-list) · [swipe](FEATURES.md#swipe) · [the flagged screen](FEATURES.md#the-flagged-screen) · [ask](FEATURES.md#ask) · [notes](FEATURES.md#notes) · [commit and approve](FEATURES.md#commit-and-approve) · [keys](FEATURES.md#keys) · [config](FEATURES.md#config) · [safety](FEATURES.md#safety) · [limits](FEATURES.md#limits)

## development

```bash
git clone https://github.com/julianiff/teer
cd teer
cargo build --release

cargo test                   # unit, fixture-repo and tui snapshot tests
cargo clippy --all-targets
```

tests that need a real agent are ignored by default; each one says how to run it. the `claude` and `codex` command lines are in [`src/agent/commands.rs`](src/agent/commands.rs), the model api client in [`src/agent/api.rs`](src/agent/api.rs), the prompts in [`src/review/prompts.rs`](src/review/prompts.rs).

## license

[Apache 2.0](LICENSE)
