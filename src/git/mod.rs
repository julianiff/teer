//! Diff extraction and file contents at the base and head revisions.
//! Shells out to `git` so we behave exactly like the user's own git.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const WORKTREE: &str = "WORKTREE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: PathBuf,
    /// Set when the file was renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<PathBuf>,
    pub status: FileStatus,
    pub old_content: Option<String>,
    pub new_content: Option<String>,
}

/// A changed file teer can't show: binary, not UTF-8, too large, or not a
/// plain file. Kept so the review can say it was left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedFile {
    pub path: PathBuf,
    pub status: FileStatus,
    /// Why, in a word or two: "binary", "too large".
    pub why: String,
}

/// What is being compared.
#[derive(Debug, Clone)]
pub struct DiffSpec {
    /// Resolved commit of the old side.
    pub base: String,
    /// Commit of the new side, or [`WORKTREE`].
    pub head: String,
    /// Commit HEAD points at.
    pub head_commit: String,
}

/// `-c` settings every git run by teer gets. The code under review (a PR's
/// files, `.gitattributes`, `.lfsconfig`, even a committed `.husky/_` hooks
/// directory) is attacker-controlled, while the user's own git config is not:
/// so nothing configured there may run because of it. No hooks (also catches a
/// relative `core.hooksPath` that resolves inside the checkout), no fsmonitor
/// daemon, no Git LFS filters (they would run a user-configured command on
/// PR content and may hit the network). None of these is needed to read a
/// diff, a blob or a file list.
pub const HARDEN: [&str; 14] = [
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "filter.lfs.process=",
    "-c",
    "filter.lfs.smudge=",
    "-c",
    "filter.lfs.clean=",
    "-c",
    "filter.lfs.required=false",
    "-c",
    "log.showSignature=false",
];

/// A git command for `repo` with [`HARDEN`] applied, Git LFS downloads off, and no optional locks.
pub fn command(repo: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(repo)
        .args(HARDEN)
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_EXTERNAL_DIFF");
    c
}

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = command(repo)
        .args(args)
        .output()
        .context("failed to run git; is it installed?")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

fn git_str(repo: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git(repo, args)?)
        .trim()
        .to_string())
}

pub fn repo_root(dir: &Path) -> Result<PathBuf> {
    let root = git_str(dir, &["rev-parse", "--show-toplevel"])
        .with_context(|| format!("{} is not inside a git repository", dir.display()))?;
    Ok(PathBuf::from(root))
}

/// Git's empty tree, used as the base when the repository has no commits.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

pub fn head_commit(repo: &Path) -> Option<String> {
    git_str(repo, &["rev-parse", "--verify", "-q", "HEAD"])
        .ok()
        .filter(|s| !s.is_empty())
}

/// Resolve what to compare. Without a base, the working tree (including
/// untracked files) is compared against HEAD. With a base, HEAD is compared
/// against the merge base of `base` and HEAD, like a pull request would be.
pub fn resolve(repo: &Path, base: Option<&str>) -> Result<DiffSpec> {
    let head_commit = head_commit(repo);
    match base {
        None => Ok(DiffSpec {
            base: head_commit.clone().unwrap_or_else(|| EMPTY_TREE.into()),
            head: WORKTREE.into(),
            head_commit: head_commit.unwrap_or_else(|| EMPTY_TREE.into()),
        }),
        Some(b) => {
            let head = head_commit.context("--base needs at least one commit on HEAD")?;
            let base_commit = git_str(repo, &["rev-parse", "--verify", &format!("{b}^{{commit}}")])
                .with_context(|| format!("unknown revision '{b}'"))?;
            let merge_base =
                git_str(repo, &["merge-base", &base_commit, &head]).unwrap_or(base_commit);
            Ok(DiffSpec {
                base: merge_base,
                head: head.clone(),
                head_commit: head,
            })
        }
    }
}

fn show(repo: &Path, rev: &str, path: &Path) -> Result<String, &'static str> {
    if rev == EMPTY_TREE {
        return Err("unreadable");
    }
    let spec = format!("{rev}:{}", path.to_string_lossy());
    let bytes = git(repo, &["show", &spec]).map_err(|_| "unreadable")?;
    text_of(bytes)
}

/// Bytes as text, or why they aren't: "binary" (a NUL byte) or "not UTF-8".
fn text_of(bytes: Vec<u8>) -> Result<String, &'static str> {
    String::from_utf8(bytes).map_err(|e| {
        if e.as_bytes().contains(&0) {
            "binary"
        } else {
            "not UTF-8"
        }
    })
}

/// The changed files with their full old and new contents, and the files
/// left out with why (binary, not UTF-8, too large, not a plain file).
pub fn changes(repo: &Path, spec: &DiffSpec) -> Result<(Vec<ChangedFile>, Vec<SkippedFile>)> {
    let mut args = vec![
        "diff",
        "--name-status",
        "-z",
        "-M",
        "--no-color",
        "--no-ext-diff",
    ];
    let range;
    if spec.head == WORKTREE {
        args.push(&spec.base);
    } else {
        range = format!("{}..{}", spec.base, spec.head);
        args.push(&range);
    }
    let raw = git(repo, &args)?;
    let mut entries = parse_name_status(&raw);

    if spec.head == WORKTREE {
        let untracked = git(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        for p in untracked.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            entries.push((
                FileStatus::Added,
                None,
                PathBuf::from(String::from_utf8_lossy(p).as_ref()),
            ));
        }
    }

    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for (status, old_path, path) in entries {
        let old_src = old_path.as_ref().unwrap_or(&path);
        let old_content = match status {
            FileStatus::Added => None,
            _ => Some(show(repo, &spec.base, old_src)),
        };
        let new_content = match status {
            FileStatus::Deleted => None,
            _ if spec.head == WORKTREE => {
                Some(read_text_file_why(repo, &path, MAX_WORKTREE_FILE_BYTES))
            }
            _ => Some(show(repo, &spec.head, &path)),
        };
        // Skip binary or unreadable files: both sides must be text if present.
        let why = [&new_content, &old_content]
            .into_iter()
            .find_map(|c| match c {
                Some(Err(why)) => Some(*why),
                Some(Ok(text)) if text.contains('\0') => Some("binary"),
                _ => None,
            });
        if let Some(why) = why {
            skipped.push(SkippedFile {
                path,
                status,
                why: why.to_string(),
            });
            continue;
        }
        files.push(ChangedFile {
            path,
            old_path,
            status,
            old_content: old_content.and_then(Result::ok),
            new_content: new_content.and_then(Result::ok),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    skipped.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((files, skipped))
}

/// Largest untracked file read from the working tree.
pub const MAX_WORKTREE_FILE_BYTES: usize = 16 * 1024 * 1024;

/// Read a working-tree file as text, but only if it is a plain regular file
/// that really lives inside `repo`. A symlink (to `~/.aws/credentials`, or
/// `/dev/zero`), a FIFO, a device, a directory, or a path that goes through a
/// symlinked directory is never followed or opened, and no more than `limit`
/// bytes are ever read, whatever size the file claims to have. `None` for all
/// of those, for files over `limit`, and for non-UTF-8 content.
pub fn read_text_file(repo: &Path, rel: &Path, limit: usize) -> Option<String> {
    read_text_file_why(repo, rel, limit).ok()
}

/// [`read_text_file`], saying why there is no text: "symlink", "not a
/// regular file", "too large", "binary", "not UTF-8", "outside the
/// repository" or "unreadable".
pub fn read_text_file_why(repo: &Path, rel: &Path, limit: usize) -> Result<String, &'static str> {
    use std::io::Read;
    const OUTSIDE: &str = "outside the repository";
    const UNREADABLE: &str = "unreadable";
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(OUTSIDE);
    }
    let full = repo.join(rel);
    let meta = std::fs::symlink_metadata(&full).map_err(|_| UNREADABLE)?;
    if meta.file_type().is_symlink() {
        return Err("symlink");
    }
    if !meta.file_type().is_file() {
        return Err("not a regular file");
    }
    if meta.len() > limit as u64 {
        return Err("too large");
    }
    let real = full.canonicalize().map_err(|_| UNREADABLE)?;
    if !real.starts_with(repo.canonicalize().map_err(|_| UNREADABLE)?) {
        return Err(OUTSIDE);
    }
    let file = std::fs::File::open(&real).map_err(|_| UNREADABLE)?;
    // The opened file must be the regular file we looked at.
    if !file
        .metadata()
        .map_err(|_| UNREADABLE)?
        .file_type()
        .is_file()
    {
        return Err("not a regular file");
    }
    let mut buf = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|_| UNREADABLE)?;
    if buf.len() > limit {
        return Err("too large");
    }
    text_of(buf)
}

/// The newest `max` commits of `base..head`, oldest first, each as
/// `\0sha\0short\0parents\0author\0date\0subject\0body\0` followed by its
/// patch without context lines (`--unified=0`, renames followed). Parsed by
/// `review::intent::parse_log`. Merges carry no patch.
pub fn log_patches(repo: &Path, base: &str, head: &str, max: usize) -> Result<Vec<u8>> {
    let range = format!("{base}..{head}");
    let max = format!("-n{max}");
    git(
        repo,
        &[
            "log",
            &max,
            "--reverse",
            "--topo-order",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--date=short",
            "--format=%x00%H%x00%h%x00%P%x00%an%x00%ad%x00%s%x00%b%x00",
            "-p",
            "--unified=0",
            "-M",
            // Whatever the user's `diff.noprefix` or `diff.mnemonicPrefix` say.
            "--src-prefix=a/",
            "--dst-prefix=b/",
            &range,
            "--",
        ],
    )
}

/// Lines added, lines removed and files changed between two commits, as
/// `git diff --numstat` counts them (a binary file counts as a file only).
pub fn diff_size(repo: &Path, base: &str, head: &str) -> Option<(u32, u32, u32)> {
    let out = git(
        repo,
        &[
            "diff",
            "--numstat",
            "-M",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            base,
            head,
            "--",
        ],
    )
    .ok()?;
    let (mut add, mut del, mut files) = (0u32, 0u32, 0u32);
    for line in String::from_utf8_lossy(&out).lines() {
        let mut parts = line.split('\t');
        let (Some(a), Some(d)) = (parts.next(), parts.next()) else {
            continue;
        };
        files += 1;
        add = add.saturating_add(a.parse().unwrap_or(0));
        del = del.saturating_add(d.parse().unwrap_or(0));
    }
    Some((add, del, files))
}

fn parse_name_status(raw: &[u8]) -> Vec<(FileStatus, Option<PathBuf>, PathBuf)> {
    let fields: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < fields.len() {
        let code = fields[i].as_str();
        let kind = code.chars().next().unwrap_or('M');
        match kind {
            'R' | 'C' if i + 2 < fields.len() => {
                let old = PathBuf::from(&fields[i + 1]);
                let new = PathBuf::from(&fields[i + 2]);
                let status = if kind == 'R' {
                    FileStatus::Renamed
                } else {
                    FileStatus::Added
                };
                let old_path = (kind == 'R').then_some(old);
                out.push((status, old_path, new));
                i += 3;
            }
            _ => {
                let status = match kind {
                    'A' => FileStatus::Added,
                    'D' => FileStatus::Deleted,
                    _ => FileStatus::Modified,
                };
                if let Some(p) = fields.get(i + 1) {
                    out.push((status, None, PathBuf::from(p)));
                }
                i += 2;
            }
        }
    }
    out
}

/// All tracked files at the new side (used for finding callers outside the
/// changed files).
pub fn tracked_files(repo: &Path) -> Result<Vec<PathBuf>> {
    let raw = git(
        repo,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    Ok(raw
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(String::from_utf8_lossy(p).as_ref()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_status_with_renames() {
        let raw = b"M\0a.py\0R087\0old.rs\0new.rs\0A\0b.ts\0D\0gone.py\0";
        let parsed = parse_name_status(raw);
        assert_eq!(parsed.len(), 4);
        assert_eq!(
            parsed[0],
            (FileStatus::Modified, None, PathBuf::from("a.py"))
        );
        assert_eq!(
            parsed[1],
            (
                FileStatus::Renamed,
                Some(PathBuf::from("old.rs")),
                PathBuf::from("new.rs")
            )
        );
        assert_eq!(parsed[2].0, FileStatus::Added);
        assert_eq!(parsed[3].0, FileStatus::Deleted);
    }
}
