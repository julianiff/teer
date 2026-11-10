//! What the help (`?`) lists: the actions each screen and panel handles,
//! in the help's groups (Move, Decide, Look closer, More, Finish), pairs on
//! one row, built from the keymap. Front ends lay the rows out.

use serde::Serialize;

use super::app::{App, Overlay, Screen};
use crate::keymap::Action;

/// The actions the queue handles, for the help, in the order it lists them
/// inside each of its groups (Move, Decide, Look closer, More, Finish).
pub const QUEUE: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::MoveNextPending,
    Action::MovePrevPending,
    Action::MoveNextFile,
    Action::MovePrevFile,
    Action::ScrollHalfDown,
    Action::ScrollHalfUp,
    Action::ScrollPageDown,
    Action::ScrollPageUp,
    Action::FindUnit,
    Action::DecideAccept,
    Action::DecideFlag,
    Action::DecideAcceptFile,
    Action::DecideFlagFile,
    Action::NoteEdit,
    Action::VisualStart,
    Action::HistoryUndo,
    Action::HistoryRedo,
    Action::HistoryRepeat,
    Action::ViewOpen,
    Action::AskOpen,
    Action::GotoDefinition,
    Action::GotoCallers,
    Action::GotoEditor,
    Action::ToolAdvice,
    Action::ToolTests,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
    Action::ToolSecurity,
    Action::ToggleAutoAccepted,
    Action::ToggleByFile,
    Action::ToggleSecurityOrder,
    Action::ToolHistory,
    Action::ToolNotes,
    Action::ToolFlagged,
    Action::ToolSettings,
    Action::FinishApp,
    Action::HelpOpen,
    Action::FinishOpen,
    Action::ReviewRerun,
    Action::ReviewReset,
    Action::ViewClose,
];

/// The actions the unit view handles, for the help, in the order it lists
/// them inside each of its groups (Move, Decide, Look closer, More, Finish).
pub const UNIT: &[Action] = &[
    Action::MoveNextPending,
    Action::MovePrevPending,
    Action::MoveNextHunk,
    Action::MovePrevHunk,
    Action::MoveNextFile,
    Action::MovePrevFile,
    Action::MoveNextTile,
    Action::MovePrevTile,
    Action::ToggleZoom,
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::ScrollHalfDown,
    Action::ScrollHalfUp,
    Action::ScrollPageDown,
    Action::ScrollPageUp,
    Action::FindUnit,
    Action::ViewBack,
    Action::DecideAccept,
    Action::DecideFlag,
    Action::DecideAcceptFile,
    Action::DecideFlagFile,
    Action::NoteEdit,
    Action::HistoryUndo,
    Action::HistoryRedo,
    Action::HistoryRepeat,
    Action::AskOpen,
    Action::GotoDefinition,
    Action::GotoCallers,
    Action::GotoEditor,
    Action::GotoBrowser,
    Action::ToolAdvice,
    Action::ToolTests,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
    Action::ToolSecurity,
    Action::ToolProve,
    Action::ToolHistory,
    Action::ToolNotes,
    Action::ToolFlagged,
    Action::ToolSettings,
    Action::FinishApp,
    Action::HelpOpen,
    Action::FinishOpen,
    Action::ReviewRerun,
    Action::ReviewReset,
    Action::ViewClose,
];

/// The actions the flagged list handles, for the help, in the order it
/// lists them inside each of its groups.
pub const FLAGGED: &[Action] = &[
    Action::MoveNextPending,
    Action::MovePrevPending,
    Action::MoveNextHunk,
    Action::MovePrevHunk,
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::ScrollHalfDown,
    Action::ScrollHalfUp,
    Action::ScrollPageDown,
    Action::ScrollPageUp,
    Action::FindUnit,
    Action::ViewBack,
    Action::DecideAccept,
    Action::NoteEdit,
    Action::HistoryUndo,
    Action::HistoryRedo,
    Action::ViewOpen,
    Action::AskOpen,
    Action::GotoDefinition,
    Action::GotoCallers,
    Action::GotoEditor,
    Action::ToolAdvice,
    Action::ToolTests,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
    Action::ToolSecurity,
    Action::ToolFlagged,
    Action::ToolHistory,
    Action::ToolNotes,
    Action::HelpOpen,
    Action::FinishOpen,
    Action::ViewClose,
];

/// What the flagged list says an action does where `?` lists it, when
/// that differs from the keymap's words.
pub fn flagged_describe(a: Action) -> Option<&'static str> {
    Some(match a {
        Action::MoveNextPending | Action::MoveDown => "next flagged unit",
        Action::MovePrevPending | Action::MoveUp => "previous flagged unit",
        Action::ViewOpen => "look closer at it",
        Action::DecideAccept => "accept it: it leaves the list",
        Action::ToolFlagged => "back",
        _ => return None,
    })
}

/// The actions the sheet handles, for the help. `⏎` (the step) and `esc`
/// (close) are the sheet's own keys, as on every sheet.
pub const FINISH: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::MoveNextPending,
    Action::NoteEdit,
    Action::FinishYank,
    Action::ScrollHalfDown,
    Action::ScrollHalfUp,
    Action::ScrollPageDown,
    Action::ScrollPageUp,
    Action::ViewBack,
    Action::ViewClose,
    Action::HelpOpen,
];

/// The actions the notes view handles.
pub const NOTES: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::NoteEdit,
    Action::NoteDelete,
    Action::ViewOpen,
    Action::ViewBack,
    Action::ViewClose,
    Action::HelpOpen,
    Action::FinishYank,
    Action::FinishOpen,
];

/// The actions the commits view handles.
pub const COMMITS: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::ViewOpen,
    Action::ViewBack,
    Action::ViewClose,
    Action::ToolHistory,
];

/// The actions a jump list (`gd`, `gr`) handles.
pub const JUMP: &[Action] = &[
    Action::MoveDown,
    Action::MoveUp,
    Action::MoveFirst,
    Action::MoveLast,
    Action::ViewOpen,
    Action::ViewBack,
    Action::ViewClose,
];

/// The panel the help is about, if any: the one it was opened over
/// ([`App::help_over`]), else the one open. Its name and the actions it
/// handles.
pub fn help_panel(app: &App) -> Option<(&'static str, &'static [Action])> {
    let over = match &app.overlay {
        Overlay::Help => app.help_over.as_ref()?,
        other => other,
    };
    match over {
        Overlay::Notes { .. } | Overlay::ConfirmDeleteNote { .. } => Some(("notes", NOTES)),
        Overlay::Commits { .. } => Some(("commits", COMMITS)),
        Overlay::Jump { .. } => Some(("jump list", JUMP)),
        Overlay::Finish { .. } => Some(("finish", FINISH)),
        _ => None,
    }
}

/// The actions the screen or panel the help is about handles: the open
/// panel, else the screen.
pub fn screen_actions(app: &App) -> &'static [Action] {
    match help_panel(app) {
        Some((_, actions)) => actions,
        None => match app.screen {
            Screen::Queue => QUEUE,
            Screen::Unit => UNIT,
            Screen::Flagged => FLAGGED,
        },
    }
}

/// What the help calls the screen or panel it is about.
pub fn screen_name(app: &App) -> &'static str {
    match help_panel(app) {
        Some((name, _)) => name,
        None => match app.screen {
            Screen::Queue => "queue",
            Screen::Unit => "unit",
            Screen::Flagged => "flagged units",
        },
    }
}

/// The help's groups, in its order.
pub const GROUPS: [&str; 5] = ["Move", "Decide", "Look closer", "More", "Finish"];

/// Which of [`GROUPS`] `a` is listed under: moving through the review,
/// deciding on it, looking closer at a unit (its view, asking, going to
/// code, the tools beside it and moving between them), the rest of what
/// opens, and finishing.
pub fn group_of(a: Action) -> usize {
    use Action::*;
    match a {
        MoveDown | MoveUp | MoveFirst | MoveLast | MoveNextPending | MovePrevPending
        | MoveNextHunk | MovePrevHunk | MoveNextFile | MovePrevFile | ScrollHalfDown
        | ScrollHalfUp | ScrollPageDown | ScrollPageUp | FindUnit | ViewBack => 0,
        DecideAccept | DecideFlag | NoteEdit | DecideAcceptFile | DecideFlagFile | VisualStart
        | HistoryUndo | HistoryRedo | HistoryRepeat | NoteDelete => 1,
        ViewOpen | AskOpen | GotoDefinition | GotoCallers | GotoEditor | GotoBrowser
        | ToolAdvice | ToolTests | ToolRunOldNew | ToolDiagram | ToolSecurity | ToolProve
        | ToolFlagged | MoveNextTile | MovePrevTile | ToggleZoom => 2,
        ToggleAutoAccepted | ToggleByFile | ToggleSecurityOrder | ToolHistory | ToolNotes
        | ToolSettings | HelpOpen => 3,
        FinishOpen | FinishYank | FinishApp | ReviewRerun | ReviewReset | ViewClose => 4,
    }
}

/// Actions the help puts on one row when a screen has both.
pub const PAIRS: &[(Action, Action)] = &[
    (Action::MoveDown, Action::MoveUp),
    (Action::MoveFirst, Action::MoveLast),
    (Action::MoveNextPending, Action::MovePrevPending),
    (Action::MoveNextFile, Action::MovePrevFile),
    (Action::MoveNextHunk, Action::MovePrevHunk),
    (Action::ScrollHalfDown, Action::ScrollHalfUp),
    (Action::ScrollPageDown, Action::ScrollPageUp),
    (Action::DecideAccept, Action::DecideFlag),
    (Action::DecideAcceptFile, Action::DecideFlagFile),
    (Action::HistoryUndo, Action::HistoryRedo),
    (Action::ViewOpen, Action::ViewBack),
    (Action::GotoDefinition, Action::GotoCallers),
    (Action::MoveNextTile, Action::MovePrevTile),
];

/// Two descriptions on one row, the words they share said once:
/// `move down / up`, `next / previous file`.
pub fn joined(a: &str, b: &str) -> String {
    let x: Vec<&str> = a.split(' ').collect();
    let y: Vec<&str> = b.split(' ').collect();
    let pre = x.iter().zip(&y).take_while(|(p, q)| p == q).count();
    // Both keep a word of their own.
    let room = x.len().min(y.len()).saturating_sub(pre + 1);
    let suf = x
        .iter()
        .rev()
        .zip(y.iter().rev())
        .take(room)
        .take_while(|(p, q)| p == q)
        .count();
    if pre >= x.len().min(y.len()) {
        return format!("{a} / {b}");
    }
    let mut out = x[..pre].join(" ");
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(&x[pre..x.len() - suf].join(" "));
    out.push_str(" / ");
    out.push_str(&y[pre..y.len() - suf].join(" "));
    if suf > 0 {
        out.push(' ');
        out.push_str(&x[x.len() - suf..].join(" "));
    }
    out
}

/// One row of the help: the keys and what they do; and each action on
/// it with its keys, for a click to press.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HelpRow {
    pub keys: String,
    pub what: String,
    pub each: Vec<Hint>,
}

/// A key and what it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hint {
    /// As the keymap labels it: `gd`, `<space>a`, `:w`.
    pub keys: String,
    /// The action's id (`decide.accept`), for a click to run.
    pub action: &'static str,
    pub what: String,
}

impl Hint {
    /// `a` with every key of it, and what it does where the help was opened.
    pub fn of(app: &App, a: Action) -> Hint {
        Hint {
            keys: app.keys.labels(a),
            action: a.id(),
            what: describe(app, a).to_string(),
        }
    }
}

/// What `a` does where the help was opened: the finish sheet's `i` writes
/// the commit message or the review summary, not a note.
pub fn describe(app: &App, a: Action) -> &'static str {
    match (help_panel(app), a) {
        (Some(("finish", _)), Action::NoteEdit) => "edit the commit message or summary",
        (None, a) if app.screen == Screen::Flagged => flagged_describe(a).unwrap_or(a.describe()),
        _ => a.describe(),
    }
}

/// The help's rows for `actions`, by group: each group's name and its
/// rows, pairs on one row, unbound actions left out.
pub fn help_groups(app: &App, actions: &[Action]) -> Vec<(&'static str, Vec<HelpRow>)> {
    let mut groups: Vec<(&'static str, Vec<HelpRow>)> =
        GROUPS.iter().map(|g| (*g, Vec::new())).collect();
    for &a in actions {
        // The second of a pair is on the first one's row.
        if PAIRS
            .iter()
            .any(|(x, y)| *y == a && actions.contains(x) && !app.keys.labels(*x).is_empty())
        {
            continue;
        }
        let keys = app.keys.labels(a);
        if keys.is_empty() {
            continue;
        }
        let other = PAIRS
            .iter()
            .find(|(x, y)| *x == a && actions.contains(y))
            .map(|(_, y)| *y)
            .filter(|y| !app.keys.labels(*y).is_empty());
        let row = match other {
            Some(b) => HelpRow {
                keys: format!("{keys} {}", app.keys.labels(b)),
                what: joined(describe(app, a), describe(app, b)),
                each: vec![Hint::of(app, a), Hint::of(app, b)],
            },
            None => HelpRow {
                keys,
                what: describe(app, a).to_string(),
                each: vec![Hint::of(app, a)],
            },
        };
        groups[group_of(a)].1.push(row);
    }
    groups.retain(|(_, rows)| !rows.is_empty());
    groups
}

/// An item of the keys tile: the actions it stands for (the keys of all
/// of them shown, the first one run on a click) and a word or two for it.
pub type KeyItem = (&'static [Action], &'static str);

/// What the keys tile lists, by group.
pub const KEY_GROUPS: &[(&str, &[KeyItem])] = &[
    (
        "Decide",
        &[
            (&[Action::DecideAccept], "accept"),
            (&[Action::DecideFlag], "flag"),
            (&[Action::NoteEdit], "note"),
            (&[Action::HistoryUndo], "undo"),
        ],
    ),
    (
        "Look closer",
        &[
            (&[Action::ToolAdvice], "advice"),
            (&[Action::AskOpen], "ask"),
            (&[Action::ToolTests], "tests"),
            (&[Action::ToolRunOldNew], "old vs new"),
            (&[Action::ToolDiagram], "diagram"),
            (&[Action::ToolSecurity], "security"),
            (&[Action::GotoDefinition], "calls"),
            (&[Action::GotoCallers], "callers"),
            (&[Action::GotoEditor], "editor"),
            (&[Action::ToolFlagged], "all flagged"),
        ],
    ),
    (
        "Move",
        &[
            (
                &[Action::MoveNextPending, Action::MovePrevPending],
                "pending",
            ),
            (&[Action::MoveNextHunk, Action::MovePrevHunk], "hunks"),
            (&[Action::MoveNextTile], "tiles"),
            (&[Action::ToggleZoom], "zoom"),
            (&[Action::ViewBack], "back"),
        ],
    ),
    (
        "Finish",
        &[
            (&[Action::FinishOpen], "finish"),
            (&[Action::HelpOpen], "every key"),
        ],
    ),
];

/// Items of the keys tile that need an agent.
pub const AGENT_KEYS: [Action; 6] = [
    Action::ToolAdvice,
    Action::AskOpen,
    Action::ToolRunOldNew,
    Action::ToolDiagram,
    Action::ToolSecurity,
    Action::ToolProve,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_help_lists_each_action_once_and_no_v1_ones() {
        for (i, a) in QUEUE.iter().enumerate() {
            assert!(!QUEUE[i + 1..].contains(a), "{} twice", a.id());
            assert!(!a.id().starts_with("v1."), "{}", a.id());
        }
        for a in [
            Action::DecideAccept,
            Action::DecideFlag,
            Action::ToggleAutoAccepted,
            Action::ToggleByFile,
            Action::FinishOpen,
            Action::ToolHistory,
        ] {
            assert!(QUEUE.contains(&a), "{}", a.id());
        }
        assert!(!QUEUE.contains(&Action::FinishYank));
        assert!(!QUEUE.contains(&Action::MoveNextHunk));
    }

    #[test]
    fn a_pair_says_the_words_it_shares_once() {
        assert_eq!(joined("move down", "move up"), "move down / up");
        assert_eq!(joined("first unit", "last unit"), "first / last unit");
        assert_eq!(
            joined("accept the unit", "flag the unit"),
            "accept / flag the unit"
        );
        assert_eq!(
            joined("scroll half a page down", "scroll half a page up"),
            "scroll half a page down / up"
        );
        assert_eq!(
            joined("undo the last decision", "redo"),
            "undo the last decision / redo"
        );
        assert_eq!(joined("open", "back"), "open / back");
        assert_eq!(joined("same", "same"), "same / same");
    }

    #[test]
    fn every_action_is_on_some_screen_once() {
        let tables = [QUEUE, UNIT, FINISH, NOTES, COMMITS, JUMP];
        for a in Action::all() {
            assert!(
                tables.iter().any(|t| t.contains(&a)),
                "{} is on no screen",
                a.id()
            );
        }
        for t in tables {
            for (i, a) in t.iter().enumerate() {
                assert!(!t[i + 1..].contains(a), "{} twice", a.id());
            }
        }
    }
}
