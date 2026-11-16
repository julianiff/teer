//! A proposed change's data, saved with the unit's answers.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::review::model::{Range, Risk};
use crate::security::SecondCheck;

/// A change the agent proposes for a unit, as teer checked it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// What asked for it.
    pub reason: Reason,
    /// One line from the agent: what the change does, and what it leaves
    /// alone.
    pub summary: String,
    /// In file order, then line order.
    pub hunks: Vec<ProposedHunk>,
    /// The second model's word: does it fix what it says, and change
    /// nothing else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second: Option<SecondCheck>,
    /// Hunks dropped because their text wasn't found exactly once, or
    /// named a file outside the repository (logged to `teer.log`).
    #[serde(default)]
    pub dropped: usize,
}

impl Proposal {
    /// The hunks not applied or suggested yet, by index.
    pub fn open(&self) -> Vec<usize> {
        (0..self.hunks.len())
            .filter(|&i| self.hunks[i].state == HunkState::Proposed)
            .collect()
    }
}

/// What a proposal answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "for", rename_all = "lowercase")]
pub enum Reason {
    /// A security finding that stands: its index in the unit's audit, its
    /// title and severity.
    Finding {
        index: usize,
        title: String,
        severity: Risk,
    },
    /// The reviewer's note on the unit.
    Note,
    /// The advice the agent gave on the unit.
    Advice,
    /// What would make the unit better: the inserts of improve mode (see
    /// [`crate::session::improve`]). Each hunk says what it does for the
    /// change in its [`InsertKind`].
    Improve,
}

/// What an insert does for the change: the box improve mode's list puts
/// it in, the most needed first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InsertKind {
    /// Something the change needs and lacks: a check, a case, an error
    /// path.
    Gap,
    /// Something that makes the change hold up better: a test, a guard
    /// against misuse.
    Stronger,
    /// Something that only makes it clearer: a name, a docstring.
    Polish,
}

impl InsertKind {
    /// The kinds in the list's order.
    pub const EVERY: [InsertKind; 3] = [InsertKind::Gap, InsertKind::Stronger, InsertKind::Polish];

    /// What the agent answers with, and the config says.
    pub fn id(self) -> &'static str {
        match self {
            InsertKind::Gap => "gap",
            InsertKind::Stronger => "stronger",
            InsertKind::Polish => "polish",
        }
    }

    pub fn from_id(id: &str) -> Option<InsertKind> {
        InsertKind::EVERY.into_iter().find(|k| k.id() == id.trim())
    }

    /// The box's title: `Fills a gap`, `Makes it stronger`, `Polish`.
    pub fn title(self) -> &'static str {
        match self {
            InsertKind::Gap => "Fills a gap",
            InsertKind::Stronger => "Makes it stronger",
            InsertKind::Polish => "Polish",
        }
    }

    /// On a hunk's box, in a sentence: `fills a gap`, `makes it stronger`,
    /// `polish`.
    pub fn name(self) -> &'static str {
        match self {
            InsertKind::Gap => "fills a gap",
            InsertKind::Stronger => "makes it stronger",
            InsertKind::Polish => "polish",
        }
    }
}

/// One replacement of exact text in a file of the repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedHunk {
    /// Relative to the repository root.
    pub file: PathBuf,
    /// The lines `old` covers in the code reviewed (1-based, inclusive):
    /// where `]c` stops, and what the box shows.
    pub lines: Range,
    /// The exact text replaced: found once in the file.
    pub old: String,
    /// Its replacement.
    pub new: String,
    /// Why this hunk, one line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
    /// What it does for the change, when it is an insert of improve mode;
    /// none on a proposed change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<InsertKind>,
    /// The file still parses with it (no new syntax error), for the
    /// languages teer splits; true for others.
    pub parses: bool,
    #[serde(default)]
    pub state: HunkState,
}

impl ProposedHunk {
    /// The hunk as diff lines, without a header: the lines `old` and
    /// `new` share at the start and the end as context (` `), the rest
    /// removed (`-`) and added (`+`). Each line keeps its text as is.
    pub fn diff_lines(&self) -> Vec<String> {
        let old: Vec<&str> = self.old.lines().collect();
        let new: Vec<&str> = self.new.lines().collect();
        let head = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let tail = old[head..]
            .iter()
            .rev()
            .zip(new[head..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let mut out: Vec<String> = old[..head].iter().map(|l| format!(" {l}")).collect();
        out.extend(old[head..old.len() - tail].iter().map(|l| format!("-{l}")));
        out.extend(new[head..new.len() - tail].iter().map(|l| format!("+{l}")));
        out.extend(old[old.len() - tail..].iter().map(|l| format!(" {l}")));
        out
    }

    /// [`Self::diff_lines`] under a `file:start-end` header, for notes,
    /// the clipboard and the agent.
    pub fn diff(&self) -> String {
        let mut s = format!(
            "--- {}:{}-{}\n",
            self.file.display(),
            self.lines.start,
            self.lines.end
        );
        for l in self.diff_lines() {
            s.push_str(&l);
            s.push('\n');
        }
        s
    }
}

/// Where a proposed hunk stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HunkState {
    #[default]
    Proposed,
    /// Written to the working tree (`p`).
    Applied,
    /// On a pull request: to be posted as a suggestion.
    Suggested,
    /// An insert marked to add (`a`): `:w` writes it to the working tree,
    /// or posts it as a suggestion on a pull request. `a` again takes it
    /// back.
    Picked,
    /// Left out by the reviewer (`x` on an insert): not put in, and not
    /// sent with the notes. `x` again takes it back.
    LeftOut,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(old: &str, new: &str) -> ProposedHunk {
        ProposedHunk {
            file: "a.php".into(),
            lines: Range::new(3, 5),
            old: old.into(),
            new: new.into(),
            why: String::new(),
            kind: None,
            parses: true,
            state: HunkState::Proposed,
        }
    }

    #[test]
    fn a_hunk_keeps_what_old_and_new_share_as_context() {
        let h = hunk(
            "function show($id) {\n    return $this->find($id);\n}",
            "function show($id) {\n    $this->denyAccessUnlessGranted('VIEW');\n    return $this->find($id);\n}",
        );
        assert_eq!(
            h.diff_lines(),
            [
                " function show($id) {",
                "+    $this->denyAccessUnlessGranted('VIEW');",
                "     return $this->find($id);",
                " }",
            ]
        );
        assert!(h.diff().starts_with("--- a.php:3-5\n"));
        let h = hunk("a\nb", "c");
        assert_eq!(h.diff_lines(), ["-a", "-b", "+c"]);
    }
}
