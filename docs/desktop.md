# Teer.app

teer's review in a window, for macOS (it builds for Linux too). The review
is teer's own: the same `session` code the terminal runs, the same keys
(`[keys]` rebinding applies), config (`~/.config/teer/config.toml`) and
saved reviews (`~/.local/share/teer`). A review started in one front end
continues in the other.

## How it fits together

```
src/                  the teer library: units, graph, review, agent
├── session/          App, Effect, AppEvent, Runtime: what keys and actions do
│   └── view.rs       snapshot(&App) -> View, the view model (serde)
├── start.rs          what to review, the agent, the session: both front ends start here
├── app_link.rs       teer:// links
└── tui/              the terminal front end (feature "tui", on by default)
desktop/              Teer.app
├── src-tauri/        the Tauri 2 shell, on the library with "tui" off
│   ├── host.rs       one review per window: its session on a task, the view sent on change
│   ├── front.rs      the terminal handoffs, the window's way
│   ├── pty.rs        the terminal pane (portable-pty)
│   ├── settings.rs   `teer setup`'s checks, models and save, for the settings screen
│   ├── links.rs      reviews opened by `teer app`, teer:// links, Dock drops; Open in Terminal
│   └── env.rs        the login shell's PATH for an app started from Finder
└── src/              Svelte + TypeScript: draws the View, sends keys, actions and clicks
```

The window never decides anything. It sends keys in Neovim notation (teer's
keymap resolves them), action ids and clicks (`host::Click`), and draws
the `View` teer sends after each change (at most one per frame while
answers stream). What scrolls natively (the panes) teer tells it with a
`scroll` event naming the action a key ran.

The five effects that take over the terminal are the window's
(`front.rs`): the agent CLI the notes go to runs in a terminal pane and
quitting it re-runs the review; `gf` opens GUI editors directly and
terminal editors in the pane; the commit message and the review summary
are written in the window; the commit streams git's output into a panel
(`commit::Io::Stream`); the settings are a screen of their own.

## Shared state

Both front ends save to the same place, so a review is open in one place at
a time: `start::begin` takes a lock beside the saved reviews of the
repository (or the pull request) and the second opener is told where it is
open (`store::lock`). A lock whose process is gone is taken over.

- `teer app [<pr>] [--base <ref>]` opens the repository's review in the app
  through a `teer://open?repo=…` link.
- `:app` in the terminal saves, lets go of the review and opens it in the app.
- File › Open in Terminal in the app lets go of the review and runs `teer`
  in the user's terminal (a `.command` file, so whichever terminal opens
  those).
- Folders dropped on the Dock icon, and `teer://` links from anywhere, open
  in a free window; a window that shows the review already comes forward.

## Develop

```bash
cd desktop
npm install
npx tauri dev                 # the app, reloading the frontend on change
npm test && npm run check     # vitest, svelte-check
cargo test -p teer-desktop    # the shell's own tests
```

`npx tauri build --debug --no-bundle` builds `target/debug/teer-desktop`,
which takes `[--no-agent] <repo> [<pr>]` or a `teer://` link. With
`TEER_CONFIG` and `TEER_DATA_DIR` it uses a config and data of its own, as
the CLI does.

## Release

`.github/workflows/desktop-release.yml` runs on the same `vX.Y.Z` tag as the
CLI's release: a universal `.dmg` attached to the release, and the
`teer-app` cask in `julianiff/homebrew-tap`. The app's version lives in
`desktop/src-tauri/tauri.conf.json`, `desktop/src-tauri/Cargo.toml` and
`desktop/package.json`; the `/release` skill checks them.

What needs your accounts before the first public release:

- **Developer ID signing and notarization.** teer spawns git, gh, claude,
  codex and sandbox-exec and reads `~/.claude/projects`, so it can't live in
  the App Sandbox or the Mac App Store. It ships with a Developer ID, the
  hardened runtime (on in `tauri.conf.json`) and notarization. Join the
  Apple Developer Program, export the Developer ID Application certificate
  as a .p12, make an app-specific password, and set the secrets the
  workflow lists. Without them the `.dmg` is built unsigned and no cask is
  written.
- **In-app updates** (`tauri-plugin-updater`) need a signing key pair:
  `npx tauri signer generate -w ~/.tauri/teer.key`, the public key as
  `plugins.updater.pubkey` in `tauri.conf.json` with the release's
  `latest.json` as the endpoint, `bundle.createUpdaterArtifacts: true`, the
  private key as the `TAURI_SIGNING_PRIVATE_KEY` secret, and the plugin
  registered in `main.rs`. It isn't wired in until that key exists: the
  plugin won't start without one.

## Not yet

- Frontend end-to-end tests: Tauri's WebDriver doesn't drive WKWebView, so
  they would run on Linux CI. The view model is snapshot tested in Rust
  (`tests/it/view.rs`), the key and scroll logic with vitest.
- Pictures are SVG inline; the terminal's Kitty graphics aren't needed.
