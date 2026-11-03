//! Undo and redo of decisions (`history.undo`, `history.redo`), one stack
//! per review, saved with it so they survive quitting; a re-run keeps the
//! steps of units whose code didn't change (`store::merge_previous`). A
//! flag's step also holds the note typed on the line it opened, so `u`
//! takes the flag and its note back together.

use serde::{Deserialize, Serialize};

use super::model::Decision;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    /// Steps to undo, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub undo: Vec<Step>,
    /// Steps undone, to redo, the latest undone last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redo: Vec<Step>,
}

/// One decision action: a unit, or every unit of a file or a selection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// In the order they were made.
    pub changes: Vec<Change>,
    /// The notes written on the line the action opened (`f`, `F`, `f` on a
    /// selection), in the order they were written.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<NoteChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    pub before: Decision,
    pub after: Decision,
}

/// What a step did to the note on unit `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteChange {
    pub id: String,
    pub before: Note,
    pub after: Note,
}

/// A unit's note and the code it was written on (`Dive.note`,
/// `Dive.note_on`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
}

impl History {
    pub fn is_empty(&self) -> bool {
        self.undo.is_empty() && self.redo.is_empty()
    }

    /// Record a step that was just made. A new step forgets what was undone.
    /// Changes that change nothing are left out, and a step of none is not kept.
    pub fn record(&mut self, mut step: Step) {
        step.changes.retain(|c| c.before != c.after);
        step.notes.retain(|n| n.before != n.after);
        if step.changes.is_empty() && step.notes.is_empty() {
            return;
        }
        self.undo.push(step);
        self.redo.clear();
    }

    /// The step to take back: put each change's `before` back, last first.
    pub fn undo(&mut self) -> Option<Step> {
        let step = self.undo.pop()?;
        self.redo.push(step.clone());
        Some(step)
    }

    /// The step to make again: put each change's `after` back.
    pub fn redo(&mut self) -> Option<Step> {
        let step = self.redo.pop()?;
        self.undo.push(step.clone());
        Some(step)
    }

    /// Keep only changes to units `keep` says are still the same, and the
    /// steps that still have one (after a re-run).
    pub fn retain_units(&mut self, keep: impl Fn(&str) -> bool) {
        for stack in [&mut self.undo, &mut self.redo] {
            for step in stack.iter_mut() {
                step.changes.retain(|c| keep(&c.id));
                step.notes.retain(|n| keep(&n.id));
            }
            stack.retain(|s| !s.changes.is_empty() || !s.notes.is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, before: Decision, after: Decision) -> Step {
        Step {
            changes: vec![Change {
                id: id.into(),
                before,
                after,
            }],
            ..Default::default()
        }
    }

    fn note(text: &str) -> Note {
        Note {
            text: text.into(),
            on: Some("fp".into()),
        }
    }

    fn noted(id: &str, before: Note, after: Note) -> NoteChange {
        NoteChange {
            id: id.into(),
            before,
            after,
        }
    }

    #[test]
    fn undo_then_redo_then_a_new_step_forgets_the_redo() {
        let mut h = History::default();
        h.record(step("a", Decision::Pending, Decision::Accepted));
        h.record(step("b", Decision::Pending, Decision::Investigate));
        assert_eq!(h.undo().unwrap().changes[0].id, "b");
        assert_eq!(h.redo().unwrap().changes[0].id, "b");
        assert_eq!(h.undo().unwrap().changes[0].id, "b");
        h.record(step("c", Decision::Pending, Decision::Accepted));
        assert!(h.redo().is_none());
        assert_eq!(h.undo().unwrap().changes[0].id, "c");
        assert_eq!(h.undo().unwrap().changes[0].id, "a");
        assert!(h.undo().is_none());
    }

    #[test]
    fn steps_that_change_nothing_are_not_kept() {
        let mut h = History::default();
        h.record(step("a", Decision::Accepted, Decision::Accepted));
        h.record(Step::default());
        assert!(h.is_empty());
    }

    #[test]
    fn retain_drops_changed_units_and_empty_steps() {
        let mut h = History::default();
        h.record(step("a", Decision::Pending, Decision::Accepted));
        h.record(Step {
            changes: vec![
                Change {
                    id: "b".into(),
                    before: Decision::Pending,
                    after: Decision::Accepted,
                },
                Change {
                    id: "c".into(),
                    before: Decision::Pending,
                    after: Decision::Accepted,
                },
            ],
            ..Default::default()
        });
        h.undo();
        h.retain_units(|id| id != "a" && id != "b");
        assert!(h.undo.is_empty());
        assert_eq!(h.redo.len(), 1);
        assert_eq!(h.redo[0].changes.len(), 1);
        assert_eq!(h.redo[0].changes[0].id, "c");
    }

    #[test]
    fn a_steps_notes_round_trip_and_go_with_it() {
        // Accepting a unit can put its note away (the note on changed
        // code): the note change is part of the step.
        let mut h = History::default();
        let mut s = step("a", Decision::Pending, Decision::Accepted);
        s.notes = vec![
            noted("a", note("x"), Note::default()),
            noted("b", note("same"), note("same")),
        ];
        h.record(s);
        assert_eq!(h.undo[0].notes, [noted("a", note("x"), Note::default())]);
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(serde_json::from_str::<History>(&json).unwrap(), h);
        h.retain_units(|id| id != "a");
        assert!(
            h.is_empty(),
            "its unit changed: the step goes, note and all"
        );
    }

    #[test]
    fn it_round_trips_and_an_empty_one_is_not_written() {
        let mut h = History::default();
        h.record(step("a", Decision::Pending, Decision::Accepted));
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(serde_json::from_str::<History>(&json).unwrap(), h);
        assert_eq!(serde_json::to_string(&History::default()).unwrap(), "{}");
    }
}
