//! The queue's file tree: the changed files as an editor's explorer lists
//! them, directories before files, each by name. A chain of directories
//! with one child each is one row (`src/tui`). Every directory starts
//! open; `l` folds one and opens it again ([`App::folded`], by path, so a
//! re-run keeps it). The cursor is on a row: a file ([`App::sel`] is then
//! one of its units: its lead unit, or the unit a jump went to) or a
//! directory ([`App::dir`]). Files teer can't show are rows the cursor
//! skips.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::app::{App, Effect, Screen};
use crate::review::attention::Level;
use crate::review::model::{Decision, Prep};

/// What a row of the tree is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RowKind {
    Dir,
    File,
    /// A changed file teer can't show: the cursor skips it.
    Skipped,
}

/// One row of the tree, as it is listed now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub kind: RowKind,
    /// How many directory rows it is under.
    pub depth: usize,
    /// What the row says: a file's name, a directory's joined chain
    /// (`src/tui`).
    pub name: String,
    /// From the repository's root.
    pub path: PathBuf,
    /// A directory whose rows are hidden.
    pub folded: bool,
}

impl TreeRow {
    /// What names the row across re-runs and folds: its path, a
    /// directory's with a `/` at its end.
    pub fn id(&self) -> String {
        let path = self.path.to_string_lossy();
        match self.kind {
            RowKind::Dir => format!("{path}/"),
            _ => path.into_owned(),
        }
    }
}

/// What a file's row says about its listed units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FileFacts {
    /// The most attention one of them needs.
    pub level: Level,
    /// Every one is accepted: nothing is left to do on it.
    pub done: bool,
    /// One is flagged.
    pub flagged: bool,
    /// One is being described.
    pub preparing: bool,
    /// A second model disputes one.
    pub disputed: bool,
    /// The last re-run found one changed or new.
    pub changed: bool,
}

/// Is `file` under directory `dir`? A file is never under its own path: a
/// file and a directory of one name (one removed, one added) are two rows.
pub fn inside(file: &Path, dir: &Path) -> bool {
    file.parent().is_some_and(|p| p.starts_with(dir))
}

/// Tree order: in a directory its directories first, then its files, each
/// by name.
pub fn path_order(a: &Path, b: &Path) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.components().peekable(), b.components().peekable());
    loop {
        match (x.next(), y.next()) {
            (Some(p), Some(q)) => {
                // A component with more after it is a directory.
                let (dir_p, dir_q) = (x.peek().is_some(), y.peek().is_some());
                let by = dir_q.cmp(&dir_p).then_with(|| p.cmp(&q));
                if by.is_ne() {
                    return by;
                }
            }
            (p, q) => return p.is_some().cmp(&q.is_some()),
        }
    }
}

#[derive(Default)]
struct Node {
    dirs: BTreeMap<String, Node>,
    files: BTreeMap<String, RowKind>,
}

/// The rows of the tree of `files` (each a file or a skipped one), the
/// rows under a directory in `folded` left out.
pub fn rows<'a>(
    files: impl IntoIterator<Item = (&'a Path, RowKind)>,
    folded: &BTreeSet<PathBuf>,
) -> Vec<TreeRow> {
    let mut root = Node::default();
    for (path, kind) in files {
        let mut parts: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let Some(name) = parts.pop() else {
            continue;
        };
        let mut at = &mut root;
        for p in parts {
            at = at.dirs.entry(p).or_default();
        }
        // A file with units is never also one teer can't show.
        let k = at.files.entry(name).or_insert(kind);
        if kind == RowKind::File {
            *k = kind;
        }
    }
    let mut out = Vec::new();
    list(&root, Path::new(""), 0, folded, &mut out);
    out
}

fn list(
    node: &Node,
    under: &Path,
    depth: usize,
    folded: &BTreeSet<PathBuf>,
    out: &mut Vec<TreeRow>,
) {
    for (name, child) in &node.dirs {
        // A directory that holds one directory and nothing else is on
        // that one's row.
        let (mut name, mut child) = (name.clone(), child);
        let mut path = under.join(&name);
        while child.files.is_empty() && child.dirs.len() == 1 {
            let (next, only) = child.dirs.iter().next().expect("one directory");
            name = format!("{name}/{next}");
            path.push(next);
            child = only;
        }
        let is_folded = folded.contains(&path);
        out.push(TreeRow {
            kind: RowKind::Dir,
            depth,
            name,
            path: path.clone(),
            folded: is_folded,
        });
        if !is_folded {
            list(child, &path, depth + 1, folded, out);
        }
    }
    for (name, kind) in &node.files {
        out.push(TreeRow {
            kind: *kind,
            depth,
            name: name.clone(),
            path: under.join(name),
            folded: false,
        });
    }
}

impl App {
    // ---- Rows ---------------------------------------------------------------

    /// The tree's rows as they are listed now: the files of the listed
    /// units and the files teer can't show, under their directories.
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut files: Vec<(&Path, RowKind)> = Vec::new();
        for pos in (0..self.order.len()).filter(|&p| self.starts_file(p)) {
            files.push((self.file_at(pos).as_path(), RowKind::File));
        }
        for f in &self.review.skipped {
            files.push((f.path.as_path(), RowKind::Skipped));
        }
        rows(files, &self.folded)
    }

    /// The row the cursor is on, by index into [`App::rows`]: its
    /// directory's, else its file's, or the folded directory that file is
    /// in. `None` when no row can be selected.
    pub fn cursor_row(&self) -> Option<usize> {
        let rows = self.rows();
        if let Some(dir) = &self.dir
            && let Some(i) = rows
                .iter()
                .position(|r| r.kind == RowKind::Dir && &r.path == dir)
        {
            return Some(i);
        }
        let file = self.order.get(self.sel).map(|_| self.file_at(self.sel))?;
        rows.iter().position(|r| match r.kind {
            RowKind::File => &r.path == file,
            RowKind::Dir => r.folded && inside(file, &r.path),
            RowKind::Skipped => false,
        })
    }

    /// The directory the cursor is on.
    pub fn on_dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Why teer can't show the file at `path`, when it is one of those.
    pub fn skipped_why(&self, path: &Path) -> Option<&str> {
        let skipped = self.review.skipped.iter().find(|f| f.path == path)?;
        Some(skipped.why.as_str())
    }

    // ---- Files --------------------------------------------------------------

    pub fn file_at(&self, pos: usize) -> &PathBuf {
        &self.review.units[self.order[pos]].file
    }

    /// Does a new file start at `pos`?
    pub fn starts_file(&self, pos: usize) -> bool {
        pos == 0 || self.file_at(pos - 1) != self.file_at(pos)
    }

    /// The positions of the file `pos` is in.
    pub fn file_range(&self, pos: usize) -> Range<usize> {
        let start = (0..=pos).rev().find(|&p| self.starts_file(p)).unwrap_or(0);
        let end = (pos + 1..self.order.len())
            .find(|&p| self.starts_file(p))
            .unwrap_or(self.order.len());
        start..end
    }

    /// The positions of `file`'s listed units, in line order; empty when
    /// none is listed.
    pub fn file_units(&self, file: &Path) -> Range<usize> {
        match (0..self.order.len()).find(|&p| self.file_at(p) == file) {
            Some(p) => self.file_range(p),
            None => 0..0,
        }
    }

    /// The position of `file`'s lead unit: the pending one that needs the
    /// most attention, else the one that does of them all; the first by
    /// line among equals.
    pub fn lead_pos(&self, file: &Path) -> Option<usize> {
        let units = self.file_units(file);
        let most = |pending: bool| {
            units
                .clone()
                .filter(|&p| !pending || self.pending_at(p))
                // The first of the highest level: `max_by_key` takes the last.
                .rev()
                .max_by_key(|&p| self.shown_level(self.order[p]))
        };
        most(true).or_else(|| most(false))
    }

    /// What `file`'s row says about its listed units; `None` when none is
    /// listed.
    pub fn file_facts(&self, file: &Path) -> Option<FileFacts> {
        let at = self.file_units(file);
        let units = || at.clone().map(|p| &self.review.units[self.order[p]]);
        let level = at.clone().map(|p| self.shown_level(self.order[p])).max()?;
        Some(FileFacts {
            level,
            done: units().all(|u| u.decision == Decision::Accepted),
            flagged: units().any(|u| u.decision == Decision::Investigate),
            preparing: units().any(|u| u.prep == Prep::Pending),
            disputed: at
                .clone()
                .any(|p| !self.levels[self.order[p]].doubts.is_empty()),
            changed: units().any(|u| self.changed.contains(&u.id)),
        })
    }

    /// The most attention a listed unit under directory `dir` needs: what
    /// its row says while it is folded. `None` when it holds only files
    /// teer can't show.
    pub fn dir_level(&self, dir: &Path) -> Option<Level> {
        self.under(dir)
            .map(|p| self.shown_level(self.order[p]))
            .max()
    }

    /// Is every listed unit under directory `dir` accepted: nothing left to
    /// do there? What dims its row.
    pub fn dir_done(&self, dir: &Path) -> bool {
        self.under(dir)
            .all(|p| self.review.units[self.order[p]].decision == Decision::Accepted)
    }

    /// The positions of the listed units under directory `dir`.
    fn under(&self, dir: &Path) -> impl Iterator<Item = usize> + '_ {
        let dir = dir.to_path_buf();
        (0..self.order.len()).filter(move |&p| inside(self.file_at(p), &dir))
    }

    // ---- The cursor ---------------------------------------------------------

    /// Is the selected unit its file's lead unit?
    pub(crate) fn on_lead(&self) -> bool {
        self.order
            .get(self.sel)
            .is_some_and(|_| self.lead_pos(self.file_at(self.sel)) == Some(self.sel))
    }

    /// Put the cursor on the unit at `pos`: on its file's row, the folded
    /// directories it is in opened, that unit selected.
    pub(crate) fn cursor_to(&mut self, pos: usize) {
        self.sel = pos.min(self.order.len().saturating_sub(1));
        self.dir = None;
        if self.order.is_empty() {
            return;
        }
        let file = self.file_at(self.sel).clone();
        self.folded.retain(|d| !inside(&file, d));
    }

    /// Put the cursor on the file of the unit at `pos` as a move over the
    /// rows does: in the queue, its lead unit is selected.
    pub(crate) fn row_to(&mut self, pos: usize) {
        self.cursor_to(pos);
        if self.screen == Screen::Queue
            && !self.order.is_empty()
            && let Some(lead) = self.lead_pos(&self.file_at(self.sel).clone())
        {
            self.sel = lead;
        }
    }

    /// Put the cursor on row `i` of [`App::rows`]: a file's lead unit is
    /// selected. A row the cursor skips keeps it where it is.
    pub fn select_row(&mut self, i: usize) -> Vec<Effect> {
        let rows = self.rows();
        let Some(row) = rows.get(i) else {
            return Vec::new();
        };
        match row.kind {
            RowKind::Skipped => return Vec::new(),
            RowKind::File => {
                if let Some(pos) = self.lead_pos(&row.path) {
                    self.sel = pos;
                }
                self.dir = None;
            }
            RowKind::Dir => {
                // Its first file, for when the directory is listed no more.
                let first = self.under(&row.path).next();
                if let Some(pos) = first {
                    self.sel = pos;
                }
                self.dir = Some(row.path.clone());
            }
        }
        self.landed()
    }

    /// A click on the row named `id` ([`TreeRow::id`]): the cursor goes
    /// there, and a directory folds or opens.
    pub fn click_row(&mut self, id: &str) -> Vec<Effect> {
        let rows = self.rows();
        let Some(i) = rows.iter().position(|r| r.id() == id) else {
            return Vec::new();
        };
        let effects = self.select_row(i);
        if rows[i].kind == RowKind::Dir {
            self.toggle_dir(&rows[i].path);
        }
        effects
    }

    /// Fold directory `dir`, or open it again. A visual selection whose
    /// first row the fold hides ends.
    pub fn toggle_dir(&mut self, dir: &Path) {
        if !self.folded.remove(dir) {
            self.folded.insert(dir.to_path_buf());
        }
        if let Some(id) = &self.visual
            && !self.rows().iter().any(|r| &r.id() == id)
        {
            self.visual = None;
        }
    }

    /// Move `step` rows up or down the tree, over the rows the cursor
    /// skips, as far as there are rows. With the cursor on no row (only
    /// files teer can't show), from before the first or after the last.
    pub(crate) fn move_row(&mut self, step: isize) -> Vec<Effect> {
        let rows = self.rows();
        let stops: Vec<usize> = (0..rows.len())
            .filter(|&i| rows[i].kind != RowKind::Skipped)
            .collect();
        let Some(last) = stops.len().checked_sub(1) else {
            return Vec::new();
        };
        let at = self
            .cursor_row()
            .and_then(|i| stops.iter().position(|&s| s == i))
            .map_or(if step > 0 { -1 } else { stops.len() as isize }, |at| {
                at as isize
            });
        let to = at.saturating_add(step).clamp(0, last as isize);
        self.select_row(stops[to as usize])
    }

    /// `3gg`, `3G`: the cursor on row `n` (from 1), as Neovim's line
    /// numbers; on a row the cursor skips, the next one it doesn't.
    pub(crate) fn go_to_row(&mut self, n: usize) -> Vec<Effect> {
        let rows = self.rows();
        let stops = |i: &usize| rows[*i].kind != RowKind::Skipped;
        let at = n.saturating_sub(1).min(rows.len().saturating_sub(1));
        let to = (at..rows.len())
            .find(stops)
            .or_else(|| (0..at).rev().find(stops));
        match to {
            Some(i) => self.select_row(i),
            None => Vec::new(),
        }
    }

    /// `l`, `enter` on a directory: fold it, or open it again.
    pub(crate) fn toggle_dir_at_cursor(&mut self) -> bool {
        match self.dir.clone() {
            Some(dir) => {
                self.toggle_dir(&dir);
                true
            }
            None => false,
        }
    }

    /// `h`: an open directory folds; from a file or a folded directory,
    /// the cursor goes to the directory it is in.
    pub(crate) fn fold_or_parent(&mut self) -> Vec<Effect> {
        let rows = self.rows();
        let Some(at) = self.cursor_row() else {
            return Vec::new();
        };
        let row = &rows[at];
        if row.kind == RowKind::Dir && !row.folded {
            self.toggle_dir(&row.path.clone());
            return Vec::new();
        }
        let parent = (0..at)
            .rev()
            .find(|&i| rows[i].kind == RowKind::Dir && rows[i].depth + 1 == row.depth);
        match parent {
            Some(i) => self.select_row(i),
            None => Vec::new(),
        }
    }

    /// The rows of the visual selection: from the row it started on to the
    /// cursor. `None` outside visual mode.
    pub fn visual_rows(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let anchor = self.visual.as_deref()?;
        let anchor = self.rows().iter().position(|r| r.id() == anchor)?;
        let at = self.cursor_row()?;
        Some(anchor.min(at)..=anchor.max(at))
    }

    /// The positions of the listed units the rows `rows` stand for: a
    /// file's, and under a folded directory every file's.
    pub(crate) fn units_of_rows(&self, rows: std::ops::RangeInclusive<usize>) -> Vec<usize> {
        let all = self.rows();
        let mut out = BTreeSet::new();
        for row in all.get(rows).unwrap_or_default() {
            match row.kind {
                RowKind::File => out.extend(self.file_units(&row.path)),
                RowKind::Dir if row.folded => out.extend(self.under(&row.path)),
                _ => {}
            }
        }
        out.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[&str], skipped: &[&str], folded: &[&str]) -> Vec<String> {
        let files = (files.iter().map(|f| (Path::new(*f), RowKind::File)))
            .chain(skipped.iter().map(|f| (Path::new(*f), RowKind::Skipped)));
        let folded = folded.iter().map(PathBuf::from).collect();
        rows(files, &folded)
            .iter()
            .map(|r| {
                let mark = match r.kind {
                    RowKind::Dir if r.folded => "+ ",
                    RowKind::Dir => "- ",
                    RowKind::File => "",
                    RowKind::Skipped => "! ",
                };
                format!("{}{mark}{}", "  ".repeat(r.depth), r.name)
            })
            .collect()
    }

    #[test]
    fn directories_come_before_files_each_by_name() {
        let t = tree(
            &[
                "src/b.rs",
                "README.md",
                "src/tui/x.rs",
                "src/a.rs",
                "docs/a.md",
            ],
            &[],
            &[],
        );
        assert_eq!(
            t,
            [
                "- docs",
                "  a.md",
                "- src",
                "  - tui",
                "    x.rs",
                "  a.rs",
                "  b.rs",
                "README.md"
            ]
        );
    }

    #[test]
    fn a_chain_of_single_directories_is_one_row() {
        let t = tree(
            &[
                "src/session/view/a.rs",
                "src/session/view/deep/b.rs",
                "x/y/z.rs",
            ],
            &[],
            &[],
        );
        assert_eq!(
            t,
            [
                "- src/session/view",
                "  - deep",
                "    b.rs",
                "  a.rs",
                "- x/y",
                "  z.rs"
            ]
        );
        // The row is named by the whole chain's path.
        let all = rows([(Path::new("x/y/z.rs"), RowKind::File)], &BTreeSet::new());
        assert_eq!(all[0].path, PathBuf::from("x/y"));
        assert_eq!(
            (all[0].id(), all[1].id()),
            ("x/y/".into(), "x/y/z.rs".into())
        );
        // A file beside the one directory ends the chain.
        assert_eq!(
            tree(&["a/b/c.rs", "a/d.rs"], &[], &[]),
            ["- a", "  - b", "    c.rs", "  d.rs"]
        );
    }

    #[test]
    fn a_folded_directory_hides_its_rows() {
        let files = ["src/tui/x.rs", "src/a.rs", "b.rs"];
        assert_eq!(tree(&files, &[], &["src"]), ["+ src", "b.rs"]);
        assert_eq!(
            tree(&files, &[], &["src/tui"]),
            ["- src", "  + tui", "  a.rs", "b.rs"]
        );
        // A fold is by path: one of a directory not listed does nothing.
        assert_eq!(tree(&["b.rs"], &[], &["src"]), ["b.rs"]);
    }

    #[test]
    fn files_teer_cannot_show_are_rows_too() {
        let t = tree(&["src/a.rs"], &["src/logo.png", "assets/big.bin"], &[]);
        assert_eq!(
            t,
            ["- assets", "  ! big.bin", "- src", "  a.rs", "  ! logo.png"]
        );
    }

    #[test]
    fn path_order_is_the_trees() {
        let mut paths: Vec<PathBuf> = ["src/b.rs", "a.rs", "src/tui/x.rs", "src/a.rs", "docs/a.md"]
            .iter()
            .map(PathBuf::from)
            .collect();
        paths.sort_by(|a, b| path_order(a, b));
        let listed: Vec<PathBuf> = rows(
            paths.iter().map(|p| (p.as_path(), RowKind::File)),
            &BTreeSet::new(),
        )
        .into_iter()
        .filter(|r| r.kind == RowKind::File)
        .map(|r| r.path)
        .collect();
        assert_eq!(paths, listed);
    }
}
