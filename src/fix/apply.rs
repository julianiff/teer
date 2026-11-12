//! Applying proposed hunks to the working tree: each hunk's old text is
//! looked up again in the file as it is on disk now (it must still be
//! there exactly once), then the file is written atomically.
//!
//! All or nothing: every file's new text is worked out before anything is
//! written, each is written to a temporary file beside it first, and only
//! when all of those are written are they renamed over the files. Should a
//! rename still fail, the files renamed already get their old text back.

use std::collections::BTreeMap;
use std::io::Write;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use super::model::{HunkState, Proposal, ProposedHunk};

/// Largest file a hunk is applied to.
const MAX_FILE_BYTES: usize = crate::git::MAX_WORKTREE_FILE_BYTES;

/// Write hunks `hunks` (indexes into `proposal.hunks`) of `proposal` to the
/// files under `root`, skipping those not in state `Proposed`. Each file
/// must be a plain file inside the repository (no symlink, nothing under
/// `.git`) in which every chosen hunk's old text still occurs exactly once;
/// otherwise nothing is written. The files written, relative to `root`.
pub fn apply(root: &Path, proposal: &Proposal, hunks: &[usize]) -> Result<Vec<PathBuf>, String> {
    let mut chosen = hunks.to_vec();
    chosen.sort_unstable();
    chosen.dedup();
    if let Some(&i) = chosen.iter().find(|&&i| i >= proposal.hunks.len()) {
        return Err(format!("there is no proposed hunk {}", i + 1));
    }
    let mut by_file: BTreeMap<&Path, Vec<&ProposedHunk>> = BTreeMap::new();
    for &i in &chosen {
        let h = &proposal.hunks[i];
        if h.state == HunkState::Proposed {
            by_file.entry(h.file.as_path()).or_default().push(h);
        }
    }
    if by_file.is_empty() {
        return Err("nothing left to apply".into());
    }
    // Every new text first: nothing is written unless all of them apply.
    let mut planned: Vec<(PathBuf, String, String)> = Vec::new();
    for (file, hs) in by_file {
        let name = file.display();
        if !in_repo(file) {
            return Err(format!("{name} is outside the repository"));
        }
        let old = crate::git::read_text_file_why(root, file, MAX_FILE_BYTES)
            .map_err(|why| format!("{name} can't be read: {why}"))?;
        let mut edits: Vec<(Range<usize>, &str)> = Vec::new();
        for h in hs {
            let [at] = occurrences(&old, &h.old, 2)[..] else {
                return Err(format!("{name} changed since the change was proposed"));
            };
            edits.push((at..at + h.old.len(), h.new.as_str()));
        }
        edits.sort_by_key(|(r, _)| r.start);
        if edits.windows(2).any(|w| w[0].0.end > w[1].0.start) {
            return Err(format!("the hunks for {name} overlap"));
        }
        let new = splice(&old, &edits);
        planned.push((file.to_path_buf(), old, new));
    }
    write_all(root, &planned)?;
    Ok(planned.into_iter().map(|(file, ..)| file).collect())
}

/// A path relative to the repository that stays inside it, and out of
/// `.git` (its config and hooks run code).
pub(crate) fn in_repo(file: &Path) -> bool {
    !file.as_os_str().is_empty()
        && file.components().all(|c| match c {
            Component::Normal(part) => !part.eq_ignore_ascii_case(".git"),
            _ => false,
        })
}

/// Where `needle` starts in `text` (byte offsets, overlapping matches
/// included), at most `max` of them. None for an empty `needle`.
pub(crate) fn occurrences(text: &str, needle: &str, max: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let Some(first) = needle.chars().next() else {
        return out;
    };
    let mut from = 0;
    while out.len() < max {
        let Some(i) = text[from..].find(needle) else {
            break;
        };
        out.push(from + i);
        from += i + first.len_utf8();
    }
    out
}

/// `text` with each byte range of `edits` (in order, none overlapping)
/// replaced by its text.
pub(crate) fn splice(text: &str, edits: &[(Range<usize>, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (r, new) in edits {
        out.push_str(&text[at..r.start]);
        out.push_str(new);
        at = r.end;
    }
    out.push_str(&text[at..]);
    out
}

/// Write each `(file, old, new)` of `planned`: every new text to a
/// temporary file beside its file, then each renamed over it. A failed
/// rename puts the old text back in the files renamed already.
fn write_all(root: &Path, planned: &[(PathBuf, String, String)]) -> Result<(), String> {
    let mut staged = Vec::new();
    for (file, _, new) in planned {
        // An error drops (and so deletes) the ones staged already.
        staged.push(stage(root, file, new)?);
    }
    let mut renamed: Vec<usize> = Vec::new();
    for (k, tmp) in staged.into_iter().enumerate() {
        let file = &planned[k].0;
        if let Err(e) = tmp.persist(root.join(file)) {
            for &j in &renamed {
                let (file, old, _) = &planned[j];
                let back = stage(root, file, old).and_then(|t| {
                    t.persist(root.join(file))
                        .map(|_| ())
                        .map_err(|e| e.error.to_string())
                });
                if let Err(e) = back {
                    crate::log::write(&format!(
                        "couldn't put back {} after a failed apply: {e}",
                        file.display()
                    ));
                }
            }
            return Err(format!("couldn't write {}: {}", file.display(), e.error));
        }
        renamed.push(k);
    }
    Ok(())
}

/// `text` in a temporary file in the directory of `root/file`, with that
/// file's permissions: to be renamed over it.
fn stage(root: &Path, file: &Path, text: &str) -> Result<tempfile::NamedTempFile, String> {
    let name = file.display();
    let full = root.join(file);
    let meta = std::fs::symlink_metadata(&full).map_err(|e| format!("{name}: {e}"))?;
    if !meta.file_type().is_file() {
        return Err(format!("{name} isn't a plain file"));
    }
    let dir = full
        .parent()
        .ok_or_else(|| format!("{name} has no directory"))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".teer-")
        .suffix(".tmp")
        .tempfile_in(dir)
        .map_err(|e| format!("couldn't write beside {name}: {e}"))?;
    tmp.write_all(text.as_bytes())
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|e| format!("couldn't write {name}: {e}"))?;
    std::fs::set_permissions(tmp.path(), meta.permissions())
        .map_err(|e| format!("couldn't keep the permissions of {name}: {e}"))?;
    Ok(tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occurrences_overlap_and_stop_at_max() {
        assert_eq!(occurrences("aaa", "aa", 5), [0, 1]);
        assert_eq!(occurrences("abcabc", "bc", 1), [1]);
        assert_eq!(occurrences("éxé", "é", 5), [0, 3]);
        assert!(occurrences("abc", "", 5).is_empty());
        assert!(occurrences("abc", "d", 5).is_empty());
    }

    #[test]
    fn splice_replaces_each_range_in_one_pass() {
        let t = "one\ntwo\nthree\n";
        assert_eq!(
            splice(t, &[(0..3, "1"), (8..13, "")]),
            "1\ntwo\n\n".to_string()
        );
        assert_eq!(splice(t, &[]), t);
    }

    #[test]
    fn only_paths_inside_the_repository_and_out_of_git() {
        assert!(in_repo(Path::new("src/a.php")));
        assert!(!in_repo(Path::new("../a.php")));
        assert!(!in_repo(Path::new("/etc/hosts")));
        assert!(!in_repo(Path::new(".git/config")));
        assert!(!in_repo(Path::new("sub/.GIT/hooks/pre-commit")));
        assert!(!in_repo(Path::new("")));
    }
}
