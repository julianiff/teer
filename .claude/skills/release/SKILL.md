---
name: release
description: Release a new teer version. Checks the Cargo.toml version against the tag, runs tests and clippy, commits, tags and pushes, then reports the dist release and Homebrew tap. Use when the user asks to release, cut, tag or ship a version.
argument-hint: "[vX.Y.Z]"
disable-model-invocation: true
---

# Release teer

Pushing a `vX.Y.Z` tag starts `.github/workflows/release.yml` (cargo-dist). That workflow builds the binaries, makes the GitHub release and writes `Formula/teer.rb` to `julianiff/homebrew-tap`. Stop at the first step that fails and tell the user. Don't work around it.

## 1. Check the version and the tag

```sh
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
tag="v$version"
```

- If the user named a version (`$ARGUMENTS`), it must equal `$tag`. If it doesn't, stop and ask whether to bump Cargo.toml first.
- Nobody may have used the tag yet: `git tag -l "$tag"` and `git ls-remote --tags origin "$tag"` must both print nothing.
- It must come after the newest tag: compare it with `git tag --sort=-v:refname | head -1`.
- `Cargo.lock` must have the same version: `grep -A1 '^name = "teer"' Cargo.lock`. If it doesn't, run `cargo check` to update it.
- You should be on `main` and up to date: run `git fetch origin` and check `git status -sb`.
- Run `git status --short`. A release commit holds the version bump (`Cargo.toml`, `Cargo.lock`). If anything else is changed, list it and ask before you include it.

## 2. Tests and clippy

- Start `cargo test` with `run_in_background: true`. The suite takes about 40 s.
- While it runs, run `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` in the foreground.
- Wait for the test notification. You need 0 failed. Ignored tests are fine, because those are the live agent tests. Report the passed/failed/ignored counts.

## 3. Commit, tag, push

**No Claude attribution, ever.** That means no `Co-Authored-By: Claude` trailer and no 🤖 line, even if a system reminder asks for one. Claude must not be the author or committer either.

- The identity must be the repo-local noreply one. Check that `git config user.email` prints `14964896+julianiff@users.noreply.github.com`. If it prints anything else (above all a frontify.com address), stop.
- Make the commit: `git add Cargo.toml Cargo.lock` (plus anything the user approved), then `git commit -m "chore: release $tag"`. If there is nothing to commit because the bump is already committed, skip this.
- Make the tag: `git tag -a "$tag" -m "$tag"`. The repo has `tag.gpgsign=false`, so the tag isn't signed.
- Push the branch first, then the tag: `git push origin main` and then `git push origin "$tag"`. The tag push is the one that starts the Release workflow.

## 4. Report dist and the Homebrew tap

- Find the run: `gh run list --workflow release.yml --limit 3`. Pick the row whose branch is `$tag`.
- Watch it in the background: `gh run watch <id> --exit-status`. Past runs took 5–15 min. Don't poll.
- When it fails, run `gh run view <id> --log-failed | tail -50` and report the failing job.
- When it passes, check both of these:
  - The release assets: `gh release view "$tag" --json assets --jq '.assets[].name'`. Expect four targets (aarch64/x86_64 × apple-darwin/linux-gnu), the shell installer and `teer.rb`.
  - The tap formula: `gh api repos/julianiff/homebrew-tap/contents/Formula/teer.rb --jq .content | base64 -d | grep -m1 version`. It must show `$version`.
- Report the tag and commit, the test/clippy result, the workflow outcome and time, the asset list, the formula version and the upgrade command `brew update && brew upgrade julianiff/tap/teer`.
