# teer

<p align="center">
  <a href="#install">install</a> · <a href="#start">start</a> · <a href="FEATURES.md">docs</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-666666?labelColor=333333" alt="Apache 2.0 license" /></a>
  <a href="https://github.com/julianiff/teer/releases/latest">

    

https://github.com/user-attachments/assets/e1bdbb5d-a7ab-415f-9aba-e80b056169f1


  </a>
</p>

---


**review a code change by what it does, not line by line.**

teer splits a git change into units: the functions, methods and classes that changed. it puts the risky units on top. an agent explains each unit, and each sentence points to the lines that prove it. you accept a unit, or you flag it and write a note. then `:w` sends your notes to your agent, commits the change, or posts your review on the pull request.

- **risky units first.** each unit gets a level: `▲` close look, `◆` worth a look, or `●` routine. facts from the code set the level. the agent can raise the level, but it cannot lower it. a unit that does not change behavior is accepted for you.
- **no claim without its lines.** a fast model explains each unit in its code. a second model checks each explanation. a sentence that does not point to its lines is removed.
- **answers before you ask.** open a unit, and it shows why it got its level and what changed. one tile holds the agent: `K` asks a question, and the tile offers what to do about the unit first. `␣t` shows its tests.
- **a fix loop.** `f` flags a unit, `i` writes a note, and `:w` sends the notes to claude code or codex. when the agent is done, teer reviews the change again, with only the changed units in the queue.
- **pull requests in their own checkout.** `teer <url>` reviews a pull request in a checkout that teer keeps for itself. the agent only reads. the agent instructions of the repository are not used. nothing is posted before you confirm.
- **three modes, one way to work.** `review` shows what a change does and if it is correct. `security` reads the change for abuse only. `improve` lets the agent write what to add. each mode has its own tools. `␣m` changes the mode.
- **neovim keys.** a key does the same on each screen. counts, `.`, `V`, undo and redo work. you can rebind each key.
- **any agent, or none.** use [claude code](https://claude.com/claude-code), [codex](https://developers.openai.com/codex), or a model api with your key (anthropic, openai, openrouter, ollama). `--no-agent` sorts the change without an agent.

## install

```bash
brew install julianiff/tap/teer
```

or `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/julianiff/teer/releases/latest/download/teer-installer.sh | sh` · `cargo install --locked teer` · [binaries](https://github.com/julianiff/teer/releases)

teer runs on macos and linux. it needs `git` and an agent. `gh` adds pull requests. `teer setup` checks what is installed.

## start

```bash
teer                # your changes, or this branch since its base
teer <url>          # a github pull request
teer 482            # pull request 482 of this repo
teer pr             # pick one waiting for you
teer security       # the security check alone; teer review is the default
teer improve        # what to add so the change is better
teer app            # the same review in teer.app
```

at the first start, teer asks which agent and models to use. `teer setup` changes them later.

**modes.** a review opens in `review`. it writes what each unit does into its code. one tile asks the agent (`K`), and offers what to do about the unit. the mode also has tests, proposed changes, commits, and a key to send a result to your agent. `teer security` reads the same change for abuse only. it writes the risks into the code. in the same tile, it offers the checks that follow each risk, as deep as you want. `teer improve` lets the agent write an insert for each unit: what to add to make the change better. the queue lists the inserts in place of the units. `a` marks an insert to add, and `x` leaves it out. `:w` puts the marked inserts in at once, then asks if teer must check the change again. on a pull request, the inserts go as suggestions with your review. `␣m` changes the mode in a review. see [FEATURES.md](FEATURES.md#modes).

**check what your agent wrote**

1. run `teer`.
2. press `a` or `f` on each unit in the queue.
3. press `l` on a `▲` unit, then `K` to ask.
4. press `f` and `i` to write a note.
5. press `:w` `enter` to send the notes to your agent. teer reviews the change again when the agent is done.
6. press `a` on the changed units.
7. press `:w` `enter` to commit.

**review a pull request of a teammate**

1. run `teer pr`.
2. press `a` or `f` on each unit in the queue.
3. press `l`, then `K` to ask.
4. press `f` and `i` to write a note about the problem.
5. press `:w` `enter`. teer requests changes, or approves when no unit is flagged.

**make the change better**

1. run `teer improve`.
2. press `a` on each insert to add, and `x` on the rest.
3. press `:w` `enter` to put the inserts in.
4. press `y` to check the change again.

| | |
| --- | --- |
| `a` · `f` | accept · flag, then go to the next pending unit |
| `A` · `F` | the same for all pending units of the file |
| `l` · `h` | open the unit · go back |
| `K` | ask about the unit |
| `i` | edit the note |
| `u` · `<c-r>` | undo · redo |
| `/` | find a unit |
| `␣m` | the mode: review, security, improve |
| `:w` | finish: send the notes, commit, approve |
| `?` | the keys that work now |

teer saves your place at each change. a review is open in one place at a time. if you start `teer` on the same review in a second terminal or in the app, it tells you where the review is open.

## the desktop app

teer.app shows the same review in a window. it uses the same code, the same keys (also your `[keys]`), the same config and the same saved reviews. a review that you start in one place continues in the other.

- **your repositories first.** the start screen lists your repositories. for each one, it shows the pull requests that wait for your review or are assigned to you. one click opens one. `+ add repository` adds a folder. you can also drop the folder on the window. paste the link of a pull request, and it opens in your clone of its repository. in a review, the subject at the top left (`⇧⌘O`) lists the other repositories.
- **buttons where they act.** accept and flag are on the unit. the closer looks are over the unit view. zoom and close are on a tile. each button shows its key, so you learn the keymap when you click. `⇧⌘B` hides all buttons. `⌘K` finds an action.
- **the handoffs in the window.** your agent runs in a terminal pane. you write the commit message and the review summary in the window. the output of the commit goes to a panel.
- **to the terminal and back.** `teer app` opens the review of this repository in the app. `:app` sends the review that is open in the terminal to the app. file › open in terminal sends it back.

teer.app runs on macos. it also builds on linux. there is no signed release yet. build it from a clone:

```bash
cd desktop && npm install && npx tauri build --debug --bundles app
open ../target/debug/bundle/macos/Teer.app
```

when a signed release is out: `brew install --cask julianiff/tap/teer-app`. see [docs/desktop.md](docs/desktop.md).

## config

`~/.config/teer/config.toml`:

```toml
theme = "light"                                # dark, light, mono
editor = "code --wait --goto {file}:{line}"    # default: nvim +{line} {file}

[keys]
"goto.editor" = ["gf", "e"]                    # action id = keys, in neovim notation
```

## from v1

v2 has one queue in place of the list, the swipe and the flagged screen. `:w` is in place of the finish screens. the keys follow neovim. your config and your saved reviews stay valid. old `[keys]` names still work. teer warns one time about each name that does not exist now, and writes nothing back. units that you flagged or resolved on the flagged screen are flagged, with their notes.

| v1 | v2 |
| --- | --- |
| `l` · `i` accept · flag | `a` · `f`; `i` writes a note |
| `s` swipe | the queue: `a` and `f` go to the next pending unit |
| `d` the flagged screen | `l` opens any unit |
| `n` recommend · `x` disagreement | `K`, then `enter`: the advice, the first question the ask tile offers · the disagreement shown there |
| `c` tests · `a` the agent reads them | `<space>t`, then `enter` lets the agent read them |
| `t` behavior · `v` visualize · `b` browser | gone |
| `o` · `f` resolve | gone: a flag and its note say it |
| `tab` `z` `r` tiles, zoom, ask again | `tab` `zz` in the unit view; to ask again, press `<c-r>` while you type |
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
| `teer --base main` | `teer` finds the base; `--base` still selects one |
| `teer pr list` · `teer pr <url>` | `teer pr` · `teer <url>` |

## docs

[FEATURES.md](FEATURES.md): [how it works](FEATURES.md#how-it-works) · [start](FEATURES.md#start) · [modes](FEATURES.md#modes) · [agents](FEATURES.md#agents) · [the queue](FEATURES.md#the-queue) · [the unit view](FEATURES.md#the-unit-view) · [security](FEATURES.md#security) · [proposed changes](FEATURES.md#proposed-changes) · [improve](FEATURES.md#improve) · [send to your agent](FEATURES.md#send-to-your-agent) · [finish](FEATURES.md#finish) · [the fix loop](FEATURES.md#the-fix-loop) · [notes](FEATURES.md#notes) · [keys](FEATURES.md#keys) · [config](FEATURES.md#config) · [safety](FEATURES.md#safety) · [limits](FEATURES.md#limits)

## development

```bash
git clone https://github.com/julianiff/teer
cd teer
cargo build --release

cargo test                   # unit, fixture-repo and tui snapshot tests
cargo nextest run            # the same, each test in its own process, with timings
cargo clippy --all-targets
```

teer.app is in `desktop/`, a workspace member. run `cd desktop && npm install && npx tauri dev`. its frontend tests are `npm test` and `npm run check`. see [docs/desktop.md](docs/desktop.md).

the integration tests are one binary. each file in `tests/it/` is a module of `tests/it/main.rs`. run a part of them by module path, for example `cargo test --test it pr::`.

tests that need a real agent are ignored by default. each one says how to run it. the `claude` and `codex` command lines are in [`src/agent/commands.rs`](src/agent/commands.rs). the model api client is in [`src/agent/api.rs`](src/agent/api.rs). the prompts are in [`src/review/prompts.rs`](src/review/prompts.rs).

## license

[Apache 2.0](LICENSE)
