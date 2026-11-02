//! Every key teer reads goes through here. Key sequences become named
//! actions; screens handle actions and never match keys themselves.
//!
//! Normal mode resolves sequences the way Neovim does: a prefix (`g`, `]`,
//! `[`, `z`, `<space>`) waits up to `timeoutlen` milliseconds for the next
//! key, digits before an action are its count (`3a`), and `:` opens a
//! command line (`:w`, `:e`, `:e!`, `:q`). Text inputs ([`Keymap::edit`]),
//! yes/no prompts ([`Keymap::confirm`]) and sheets ([`Keymap::sheet`]) are
//! modes of their own with fixed keys.
//!
//! The default table follows four rules, checked by [`Keymap::check_rules`]:
//! a key has one action on every screen; lowercase decides on the unit and
//! uppercase on the whole file (other uppercase keys follow Neovim); `g`
//! sequences move, `<space>` sequences open something and `:` commands
//! finish or restart; `i` only ever starts typing.
//!
//! `config.toml` rebinds by action id, in Neovim's key notation:
//!
//! ```toml
//! timeoutlen = 1000
//!
//! [keys]
//! "decide.accept" = "a"
//! "move.down" = ["j", "<down>"]
//! ```
//!
//! v1's names and keys (`accept = "y"`, `next_group = "ctrl+n"`) are read
//! too, through [`V1_NAMES`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

/// Everything a key can ask for. The id ([`Action::id`]) is what
/// `config.toml` rebinds; `v1.*` actions keep the old screens usable until
/// Phase 8 removes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    MoveDown,
    MoveUp,
    MoveFirst,
    MoveLast,
    MoveNextPending,
    MovePrevPending,
    MoveNextHunk,
    MovePrevHunk,
    MoveNextFile,
    MovePrevFile,
    ScrollHalfDown,
    ScrollHalfUp,
    ScrollPageDown,
    ScrollPageUp,
    FindUnit,
    DecideAccept,
    DecideFlag,
    NoteEdit,
    DecideAcceptFile,
    DecideFlagFile,
    VisualStart,
    HistoryUndo,
    HistoryRedo,
    HistoryRepeat,
    ViewOpen,
    ViewBack,
    AskOpen,
    GotoDefinition,
    GotoCallers,
    GotoEditor,
    GotoBrowser,
    ToggleAutoAccepted,
    /// The queue by level (each unit under its own) or by file (each file
    /// in the box of its most critical unit).
    ToggleByFile,
    ToolTests,
    ToolRunOldNew,
    ToolDiagram,
    ToolHistory,
    ToolNotes,
    ToolSettings,
    FinishOpen,
    FinishYank,
    ReviewRerun,
    ReviewReset,
    ViewClose,
    HelpOpen,
    /// Delete the selected note (notes panel only).
    NoteDelete,
    /// Hand the notes to Claude Code or Codex (notes panel only; `:w` takes
    /// over in Phase 6).
    V1NotesToAgent,
    /// Post the notes on the pull request (notes panel only; `:w` takes over
    /// in Phase 6).
    V1NotesPost,
    /// The swipe screen.
    V1Swipe,
    /// Focus the next or previous tile of the Flagged screen.
    V1NextTile,
    V1PrevTile,
    /// The tile in focus fills the screen, or no longer does.
    V1Zoom,
    /// The Recommend tile (the unit view's "what to do" from Phase 5).
    V1Recommend,
    /// The second model's opinion tile.
    V1Disagree,
    /// The agent reads the tests the search found.
    V1InterpretTests,
    /// Resolution of a flagged unit (gone in Phase 8: a flag and its note
    /// say it).
    V1ResolveOk,
    V1ResolveFix,
    /// Ask the tile in focus again.
    V1DiveRerun,
}

/// The default table: each action, its id and its default keys in Neovim
/// notation. A key starting with `:` is a command (`:w`).
const TABLE: &[(Action, &str, &[&str], &str)] = &[
    (Action::MoveDown, "move.down", &["j", "<down>"], "move down"),
    (Action::MoveUp, "move.up", &["k", "<up>"], "move up"),
    (
        Action::MoveFirst,
        "move.first",
        &["gg", "<home>"],
        "first unit",
    ),
    (Action::MoveLast, "move.last", &["G", "<end>"], "last unit"),
    (
        Action::MoveNextPending,
        "move.next_pending",
        &["n"],
        "next pending unit",
    ),
    (
        Action::MovePrevPending,
        "move.prev_pending",
        &["N"],
        "previous pending unit",
    ),
    (Action::MoveNextHunk, "move.next_hunk", &["]c"], "next hunk"),
    (
        Action::MovePrevHunk,
        "move.prev_hunk",
        &["[c"],
        "previous hunk",
    ),
    (Action::MoveNextFile, "move.next_file", &["]f"], "next file"),
    (
        Action::MovePrevFile,
        "move.prev_file",
        &["[f"],
        "previous file",
    ),
    (
        Action::ScrollHalfDown,
        "scroll.half_down",
        &["<c-d>"],
        "scroll half a page down",
    ),
    (
        Action::ScrollHalfUp,
        "scroll.half_up",
        &["<c-u>"],
        "scroll half a page up",
    ),
    (
        Action::ScrollPageDown,
        "scroll.page_down",
        &["<pagedown>"],
        "scroll a page down",
    ),
    (
        Action::ScrollPageUp,
        "scroll.page_up",
        &["<pageup>"],
        "scroll a page up",
    ),
    (
        Action::FindUnit,
        "find.unit",
        &["/"],
        "find a unit by file or name",
    ),
    (
        Action::DecideAccept,
        "decide.accept",
        &["a"],
        "accept the unit",
    ),
    (Action::DecideFlag, "decide.flag", &["f"], "flag the unit"),
    (Action::NoteEdit, "note.edit", &["i"], "edit the note"),
    (
        Action::DecideAcceptFile,
        "decide.accept_file",
        &["A"],
        "accept the file's pending units",
    ),
    (
        Action::DecideFlagFile,
        "decide.flag_file",
        &["F"],
        "flag the file's pending units",
    ),
    (Action::VisualStart, "visual.start", &["V"], "select lines"),
    (
        Action::HistoryUndo,
        "history.undo",
        &["u"],
        "undo the last decision",
    ),
    (Action::HistoryRedo, "history.redo", &["<c-r>"], "redo"),
    (
        Action::HistoryRepeat,
        "history.repeat",
        &["."],
        "repeat the last decision",
    ),
    (Action::ViewOpen, "view.open", &["l", "<cr>"], "open"),
    (Action::ViewBack, "view.back", &["h", "<esc>"], "back"),
    (Action::AskOpen, "ask.open", &["K"], "ask about the unit"),
    (
        Action::GotoDefinition,
        "goto.definition",
        &["gd"],
        "what this unit calls",
    ),
    (
        Action::GotoCallers,
        "goto.callers",
        &["gr"],
        "what calls this unit",
    ),
    (
        Action::GotoEditor,
        "goto.editor",
        &["gf"],
        "open in the editor",
    ),
    (
        Action::GotoBrowser,
        "goto.browser",
        &["gx"],
        "open the picture in the browser",
    ),
    (
        Action::ToggleAutoAccepted,
        "toggle.auto_accepted",
        &["za"],
        "show units accepted for you",
    ),
    (
        Action::ToggleByFile,
        "toggle.by_file",
        &["zf"],
        "list by file or by level",
    ),
    (
        Action::ToolTests,
        "tool.tests",
        &["<space>t"],
        "tests that cover this unit",
    ),
    (
        Action::ToolRunOldNew,
        "tool.run_old_new",
        &["<space>r"],
        "run old and new",
    ),
    (
        Action::ToolDiagram,
        "tool.diagram",
        &["<space>v"],
        "diagram of the change",
    ),
    (
        Action::ToolHistory,
        "tool.history",
        &["<space>c"],
        "commits and PR description",
    ),
    (Action::ToolNotes, "tool.notes", &["<space>n"], "every note"),
    (
        Action::ToolSettings,
        "tool.settings",
        &["<space>s"],
        "settings",
    ),
    (Action::FinishOpen, "finish.open", &[":w"], "finish"),
    (Action::FinishYank, "finish.yank", &["y"], "copy the notes"),
    (
        Action::ReviewRerun,
        "review.rerun",
        &[":e"],
        "re-run, decisions kept",
    ),
    (
        Action::ReviewReset,
        "review.reset",
        &[":e!"],
        "re-run and reset every decision",
    ),
    (
        Action::ViewClose,
        "view.close",
        &["q", ":q"],
        "close; in the list, quit",
    ),
    (Action::HelpOpen, "help.open", &["?"], "keys"),
    (Action::NoteDelete, "note.delete", &["d"], "delete the note"),
    (
        Action::V1NotesToAgent,
        "v1.notes_to_agent",
        &["c"],
        "hand the notes to the agent",
    ),
    (
        Action::V1NotesPost,
        "v1.notes_post",
        &["p"],
        "post the notes on the PR",
    ),
    (
        Action::V1Swipe,
        "v1.swipe",
        &["s"],
        "swipe through the cards",
    ),
    (
        Action::V1NextTile,
        "v1.next_tile",
        &["gt", "<tab>", "<right>"],
        "next tile",
    ),
    (
        Action::V1PrevTile,
        "v1.prev_tile",
        &["gT", "<s-tab>", "<left>"],
        "previous tile",
    ),
    (Action::V1Zoom, "v1.zoom", &["zz"], "zoom the tile"),
    (
        Action::V1Recommend,
        "v1.recommend",
        &["<space>w"],
        "what to do",
    ),
    (
        Action::V1Disagree,
        "v1.disagree",
        &["<space>m"],
        "second model",
    ),
    (
        Action::V1InterpretTests,
        "v1.interpret_tests",
        &["<space>a"],
        "agent reads the tests",
    ),
    (Action::V1ResolveOk, "v1.resolve_ok", &["o"], "resolved: ok"),
    (
        Action::V1ResolveFix,
        "v1.resolve_fix",
        &["x"],
        "resolved: needs fix",
    ),
    (
        Action::V1DiveRerun,
        "v1.dive_rerun",
        &["r"],
        "ask the tile again",
    ),
];

impl Action {
    pub fn all() -> impl Iterator<Item = Action> {
        TABLE.iter().map(|(a, ..)| *a)
    }

    fn row(self) -> &'static (Action, &'static str, &'static [&'static str], &'static str) {
        TABLE
            .iter()
            .find(|(a, ..)| *a == self)
            .expect("every action is in the table")
    }

    /// The id `config.toml` rebinds: `decide.accept`.
    pub fn id(self) -> &'static str {
        self.row().1
    }

    pub fn from_id(id: &str) -> Option<Action> {
        TABLE.iter().find(|(_, i, ..)| *i == id).map(|(a, ..)| *a)
    }

    /// What the action does, for the help and the prefix hint.
    pub fn describe(self) -> &'static str {
        self.row().3
    }

    /// Decisions `.` repeats.
    pub fn is_decision(self) -> bool {
        matches!(
            self,
            Action::DecideAccept
                | Action::DecideFlag
                | Action::DecideAcceptFile
                | Action::DecideFlagFile
        )
    }

    /// Takes you somewhere: what `g` sequences may do.
    fn moves(self) -> bool {
        let id = self.id();
        id.starts_with("move.")
            || id.starts_with("goto.")
            || matches!(self, Action::V1NextTile | Action::V1PrevTile)
    }

    /// Opens something: what `<space>` sequences may do.
    fn opens(self) -> bool {
        self.id().starts_with("tool.")
            || matches!(
                self,
                Action::V1Recommend | Action::V1Disagree | Action::V1InterpretTests
            )
    }

    /// Finishes or restarts: what `:` commands may do.
    fn finishes(self) -> bool {
        let id = self.id();
        id.starts_with("finish.") || id.starts_with("review.") || self == Action::ViewClose
    }
}

/// One key press. Shift is folded into the character (`G`, not shift+g).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Key {
    pub const fn plain(code: KeyCode) -> Key {
        Key {
            code,
            mods: KeyModifiers::NONE,
        }
    }

    pub fn from_event(k: &KeyEvent) -> Key {
        let mods = k.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let code = match k.code {
            // Some terminals send ctrl+letter uppercase.
            KeyCode::Char(c) if mods.contains(KeyModifiers::CONTROL) => {
                KeyCode::Char(c.to_ascii_lowercase())
            }
            // Others send shift+g for `G`.
            KeyCode::Char(c) if shift => KeyCode::Char(c.to_ascii_uppercase()),
            KeyCode::Tab if shift => KeyCode::BackTab,
            other => other,
        };
        Key { code, mods }
    }

    pub fn event(self) -> KeyEvent {
        KeyEvent::new(self.code, self.mods)
    }

    fn ctrl(self) -> bool {
        self.mods.contains(KeyModifiers::CONTROL)
    }

    /// A character typed as text: no ctrl or alt.
    fn text(self) -> Option<char> {
        match self.code {
            KeyCode::Char(c)
                if !c.is_control()
                    && !self
                        .mods
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Some(c)
            }
            _ => None,
        }
    }

    /// How the key is shown on screen: `g`, `⏎`, `^d`, `␣`.
    pub fn label(self) -> String {
        let key = match self.code {
            KeyCode::Enter => "⏎".to_string(),
            KeyCode::Tab => "tab".into(),
            KeyCode::BackTab => "⇧tab".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Backspace => "⌫".into(),
            KeyCode::Char(' ') => "␣".into(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pgup".into(),
            KeyCode::PageDown => "pgdn".into(),
            other => format!("{other:?}").to_lowercase(),
        };
        if self.ctrl() {
            format!("^{key}")
        } else if self.mods.contains(KeyModifiers::ALT) {
            format!("M-{key}")
        } else {
            key
        }
    }

    /// The key in Neovim notation: `g`, `<cr>`, `<c-d>`, `<space>`.
    pub fn notation(self) -> String {
        let name = match self.code {
            KeyCode::Char(' ') => Some("space".to_string()),
            KeyCode::Char('<') => Some("lt".into()),
            KeyCode::Char(c) if self.mods.is_empty() => return c.to_string(),
            KeyCode::Char(c) => Some(c.to_string()),
            KeyCode::Enter => Some("cr".into()),
            KeyCode::Esc => Some("esc".into()),
            KeyCode::Backspace => Some("bs".into()),
            KeyCode::Tab => Some("tab".into()),
            KeyCode::BackTab => Some("s-tab".into()),
            KeyCode::Up => Some("up".into()),
            KeyCode::Down => Some("down".into()),
            KeyCode::Left => Some("left".into()),
            KeyCode::Right => Some("right".into()),
            KeyCode::Home => Some("home".into()),
            KeyCode::End => Some("end".into()),
            KeyCode::PageUp => Some("pageup".into()),
            KeyCode::PageDown => Some("pagedown".into()),
            _ => None,
        };
        let name = name.unwrap_or_else(|| format!("{:?}", self.code).to_lowercase());
        let prefix = if self.ctrl() {
            "c-"
        } else if self.mods.contains(KeyModifiers::ALT) {
            "a-"
        } else {
            ""
        };
        format!("<{prefix}{name}>")
    }
}

/// Parse keys in Neovim notation: `gg`, `]c`, `<space>t`, `<c-r>`, `<cr>`,
/// `<s-tab>`, `<lt>` for `<`.
pub fn parse_keys(s: &str) -> Result<Vec<Key>> {
    let mut keys = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
            && end > 1
        {
            keys.push(
                parse_named(&rest[1..end])
                    .ok_or_else(|| anyhow::anyhow!("unknown key '{}' in '{s}'", &rest[..=end]))?,
            );
            rest = &rest[end + 1..];
            continue;
        }
        keys.push(Key::plain(KeyCode::Char(c)));
        rest = &rest[c.len_utf8()..];
    }
    if keys.is_empty() {
        bail!("empty key");
    }
    Ok(keys)
}

/// The keys a label on screen names ([`Key::label`], [`Trigger::label`]):
/// `gg`, `␣t`, `^d`, `⏎`. `None` for what isn't a key.
pub fn parse_label(label: &str) -> Option<Vec<Key>> {
    const NAMED: &[(&str, KeyCode)] = &[
        ("⏎", KeyCode::Enter),
        ("⇧tab", KeyCode::BackTab),
        ("tab", KeyCode::Tab),
        ("esc", KeyCode::Esc),
        ("⌫", KeyCode::Backspace),
        ("␣", KeyCode::Char(' ')),
        ("←", KeyCode::Left),
        ("→", KeyCode::Right),
        ("↑", KeyCode::Up),
        ("↓", KeyCode::Down),
        ("home", KeyCode::Home),
        ("end", KeyCode::End),
        ("pgup", KeyCode::PageUp),
        ("pgdn", KeyCode::PageDown),
    ];
    let mut keys = Vec::new();
    let mut rest = label;
    while !rest.is_empty() {
        let (ctrl, after) = match rest.strip_prefix('^') {
            Some(r) if !r.is_empty() => (true, r),
            _ => (false, rest),
        };
        let (code, len) = match NAMED.iter().find(|(n, _)| after.starts_with(n)) {
            Some((n, code)) => (*code, n.len()),
            None => {
                let c = after.chars().next()?;
                if c.is_whitespace() {
                    return None;
                }
                (KeyCode::Char(c), c.len_utf8())
            }
        };
        let mods = if ctrl {
            KeyModifiers::CONTROL
        } else {
            KeyModifiers::NONE
        };
        keys.push(Key { code, mods });
        rest = &after[len..];
    }
    (!keys.is_empty()).then_some(keys)
}

/// The inside of `<...>`.
fn parse_named(name: &str) -> Option<Key> {
    let lower = name.to_ascii_lowercase();
    let (mods, base) = match lower.split_once('-') {
        Some(("c", b)) if !b.is_empty() => (KeyModifiers::CONTROL, b),
        Some(("a" | "m", b)) if !b.is_empty() => (KeyModifiers::ALT, b),
        Some(("s", "tab")) => return Some(Key::plain(KeyCode::BackTab)),
        _ => (KeyModifiers::NONE, lower.as_str()),
    };
    let code = match base {
        "space" => KeyCode::Char(' '),
        "cr" | "enter" | "return" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "bs" | "backspace" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "lt" => KeyCode::Char('<'),
        "bar" => KeyCode::Char('|'),
        "bslash" => KeyCode::Char('\\'),
        _ if base.chars().count() == 1 && mods != KeyModifiers::NONE => {
            KeyCode::Char(base.chars().next()?)
        }
        _ => return None,
    };
    Some(Key { code, mods })
}

/// What starts an action: keys typed in normal mode, or a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    Keys(Vec<Key>),
    /// The text after `:` (`w`, `e!`).
    Command(String),
}

impl Trigger {
    pub fn parse(s: &str) -> Result<Trigger> {
        match s.strip_prefix(':') {
            Some(cmd) if !cmd.is_empty() => Ok(Trigger::Command(cmd.to_string())),
            _ => Ok(Trigger::Keys(parse_keys(s)?)),
        }
    }

    /// How the trigger is shown on screen: `gg`, `␣t`, `^d`, `:w`.
    pub fn label(&self) -> String {
        match self {
            Trigger::Keys(keys) => keys.iter().map(|k| k.label()).collect(),
            Trigger::Command(c) => format!(":{c}"),
        }
    }

    /// The trigger in Neovim notation, as `config.toml` takes it.
    pub fn notation(&self) -> String {
        match self {
            Trigger::Keys(keys) => keys.iter().map(|k| k.notation()).collect(),
            Trigger::Command(c) => format!(":{c}"),
        }
    }
}

/// A `[keys]` value: one key or several.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum KeySpec {
    One(String),
    Many(Vec<String>),
}

impl KeySpec {
    fn items(&self) -> Vec<&str> {
        match self {
            KeySpec::One(s) => vec![s.as_str()],
            KeySpec::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

/// What a key did in normal mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing: an unbound key, a dropped count or prefix, a cancelled
    /// command line.
    None,
    /// Waiting for the next key of a sequence or a count's action
    /// ([`Keymap::pending`], [`Keymap::hint`]).
    Pending,
    /// The command line is being typed ([`Keymap::command_line`]).
    Command,
    /// Run `action`, `count` times when a count was typed.
    Run { action: Action, count: Option<u32> },
    /// An unknown command, for the bottom bar. Nothing changed.
    Error(String),
}

/// A key in a text input (a note, a question, a commit subject).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Submit,
    Cancel,
    Backspace,
    ClearLine,
    Char(char),
    Up,
    Down,
    PageUp,
    PageDown,
    /// Moves the focus out of the text (the Ask view's links).
    Tab,
    BackTab,
    /// Asks the last question again.
    Retry,
    None,
}

/// A key on a sheet that confirms something: commit, approve, post.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sheet {
    Confirm,
    Cancel,
    /// Edit the message (rule 4: `i`).
    Edit,
    Yank,
    /// Cycle the review verdict.
    Verdict,
    /// Post the notes as inline comments, or not.
    Inline,
    Down,
    Up,
    PageDown,
    PageUp,
    Quit,
}

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1000);
const MAX_COUNT: u32 = 9999;

#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: Vec<(Action, Trigger)>,
    timeout: Duration,
    /// Keys typed so far of a sequence.
    pending: Vec<Key>,
    count: Option<u32>,
    /// When the last key of a pending sequence came.
    since: Option<Instant>,
    /// The command line being typed, after `:`.
    command: Option<String>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap::new(&HashMap::new(), DEFAULT_TIMEOUT.as_millis() as u64)
            .expect("the default keys are valid")
            .0
    }
}

impl Keymap {
    /// The default table with `overrides` (action id → keys) applied, and
    /// warnings to show the user. An override replaces the action's keys;
    /// a default key it takes is removed from the action that had it.
    /// Unknown action ids are warned about and skipped; keys that can't be
    /// parsed, or that clash, are errors.
    ///
    /// v1's names (`accept`, `ctrl+d`) still work ([`V1_NAMES`]): each
    /// becomes its v2 action unless it only repeats v1's default key, the
    /// v2 id is set too, or its keys don't fit; names v2 dropped warn once
    /// each. Nothing about them is an error, and nothing is written back.
    pub fn new(
        overrides: &HashMap<String, KeySpec>,
        timeout_ms: u64,
    ) -> Result<(Keymap, Vec<String>)> {
        let mut warnings = Vec::new();
        let mut over: Vec<(Action, Vec<Trigger>)> = Vec::new();
        // v1 names, applied after the v2 ids, each only if it fits.
        let mut v1: Vec<(&str, Action, Vec<Trigger>)> = Vec::new();
        let mut ids: Vec<&String> = overrides.keys().collect();
        ids.sort();
        for id in ids {
            let parsed = overrides[id]
                .items()
                .into_iter()
                .map(parse_trigger)
                .collect::<Result<Vec<_>>>();
            if let Some(action) = Action::from_id(id) {
                let triggers = parsed.map_err(|e| anyhow::anyhow!("[keys] '{id}': {e}"))?;
                over.push((action, triggers));
                continue;
            }
            let Some(&(name, default, became)) = V1_NAMES.iter().find(|(n, ..)| n == id) else {
                warnings.push(format!("[keys]: unknown action '{id}', ignored"));
                continue;
            };
            let Some(action) = became else {
                warnings.push(format!("`[keys].{name}` is no longer used"));
                continue;
            };
            match parsed {
                Err(e) => warnings.push(format!("`[keys].{name}` is ignored: {e}")),
                // v1's own default: v2's default stands.
                Ok(t) if t == [parse_trigger(default).expect("v1 defaults parse")] => {}
                Ok(t) => v1.push((name, action, t)),
            }
        }
        validate(&bind(&over).0)?;
        for (name, action, triggers) in v1 {
            if over.iter().any(|(a, _)| *a == action) {
                warnings.push(format!(
                    "`[keys].{name}` is ignored: \"{}\" is set",
                    action.id()
                ));
                continue;
            }
            over.push((action, triggers));
            if let Err(e) = validate(&bind(&over).0) {
                over.pop();
                warnings.push(format!(
                    "`[keys].{name}` (now \"{}\") is ignored: {e}",
                    action.id()
                ));
            }
        }
        let (bindings, taken) = bind(&over);
        warnings.extend(taken);
        Ok((
            Keymap {
                bindings,
                timeout: Duration::from_millis(timeout_ms),
                pending: Vec::new(),
                count: None,
                since: None,
                command: None,
            },
            warnings,
        ))
    }

    pub fn bindings(&self) -> impl Iterator<Item = (Action, &Trigger)> {
        self.bindings.iter().map(|(a, t)| (*a, t))
    }

    pub fn triggers(&self, action: Action) -> impl Iterator<Item = &Trigger> {
        self.bindings
            .iter()
            .filter(move |(a, _)| *a == action)
            .map(|(_, t)| t)
    }

    /// The first key of `action` as shown on screen; empty when unbound.
    pub fn label(&self, action: Action) -> String {
        self.triggers(action)
            .next()
            .map(Trigger::label)
            .unwrap_or_default()
    }

    /// Every key of `action`, joined with `/`.
    pub fn labels(&self, action: Action) -> String {
        self.triggers(action)
            .map(Trigger::label)
            .collect::<Vec<_>>()
            .join("/")
    }

    // ---- Normal mode -----------------------------------------------------

    /// Feed one key typed in normal mode at `now`.
    pub fn feed(&mut self, k: &KeyEvent, now: Instant) -> Outcome {
        let key = Key::from_event(k);
        if self.command.is_some() {
            return self.feed_command(key);
        }
        self.expire(now);
        if self.pending.is_empty() {
            if key.code == KeyCode::Esc && key.mods.is_empty() && self.count.is_some() {
                self.reset();
                return Outcome::None;
            }
            if let KeyCode::Char(d @ '0'..='9') = key.code
                && key.mods.is_empty()
                && (d != '0' || self.count.is_some())
            {
                let n = self.count.unwrap_or(0) * 10 + d.to_digit(10).unwrap_or(0);
                self.count = Some(n.min(MAX_COUNT));
                return Outcome::Pending;
            }
            if key.code == KeyCode::Char(':') && key.mods.is_empty() {
                self.reset();
                self.command = Some(String::new());
                return Outcome::Command;
            }
        }
        let mut seq = std::mem::take(&mut self.pending);
        seq.push(key);
        let mut exact = None;
        let mut longer = false;
        for (action, trigger) in &self.bindings {
            let Trigger::Keys(keys) = trigger else {
                continue;
            };
            if *keys == seq {
                exact = Some(*action);
            } else if keys.len() > seq.len() && keys.starts_with(&seq) {
                longer = true;
            }
        }
        if longer {
            self.pending = seq;
            self.since = Some(now);
            return Outcome::Pending;
        }
        let count = self.count.take();
        self.reset();
        match exact {
            Some(action) => Outcome::Run { action, count },
            None => Outcome::None,
        }
    }

    fn feed_command(&mut self, key: Key) -> Outcome {
        let line = self.command.as_mut().expect("in the command line");
        match key.code {
            KeyCode::Esc => {
                self.command = None;
                Outcome::None
            }
            KeyCode::Enter => {
                let cmd = self.command.take().unwrap_or_default();
                let cmd = cmd.trim();
                if cmd.is_empty() {
                    return Outcome::None;
                }
                let found = self.bindings.iter().find_map(|(a, t)| match t {
                    Trigger::Command(c) if c == cmd => Some(*a),
                    _ => None,
                });
                match found {
                    Some(action) => Outcome::Run {
                        action,
                        count: None,
                    },
                    None => Outcome::Error(format!("Not a command: :{cmd}")),
                }
            }
            KeyCode::Backspace => {
                if line.pop().is_none() {
                    self.command = None;
                    return Outcome::None;
                }
                Outcome::Command
            }
            KeyCode::Char('u') if key.ctrl() => {
                line.clear();
                Outcome::Command
            }
            _ => {
                if let Some(c) = key.text() {
                    line.push(c);
                }
                Outcome::Command
            }
        }
    }

    /// Drop a sequence that waited longer than `timeoutlen`, with its
    /// count. True when one was dropped, so the hint goes away. A count on
    /// its own waits until esc.
    pub fn tick(&mut self, now: Instant) -> bool {
        let had = !self.pending.is_empty();
        self.expire(now);
        had && self.pending.is_empty()
    }

    fn expire(&mut self, now: Instant) {
        if !self.pending.is_empty()
            && let Some(since) = self.since
            && now.saturating_duration_since(since) > self.timeout
        {
            self.reset();
        }
    }

    /// Forget a pending sequence, count and command line.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.count = None;
        self.since = None;
        self.command = None;
    }

    /// The count and keys typed so far (`3g`), while waiting for more.
    pub fn pending(&self) -> Option<String> {
        if self.pending.is_empty() && self.count.is_none() {
            return None;
        }
        let count = self.count.map(|n| n.to_string()).unwrap_or_default();
        let keys: String = self.pending.iter().map(|k| k.label()).collect();
        Some(format!("{count}{keys}"))
    }

    /// What can follow the pending keys: the rest of each sequence and its
    /// action, in table order. Empty when no sequence is pending.
    pub fn hint(&self) -> Vec<(String, Action)> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        self.bindings
            .iter()
            .filter_map(|(a, t)| match t {
                Trigger::Keys(keys)
                    if keys.len() > self.pending.len() && keys.starts_with(&self.pending) =>
                {
                    let rest: String = keys[self.pending.len()..]
                        .iter()
                        .map(|k| k.label())
                        .collect();
                    Some((rest, *a))
                }
                _ => None,
            })
            .collect()
    }

    /// The command line being typed, without the `:`.
    pub fn command_line(&self) -> Option<&str> {
        self.command.as_deref()
    }

    // ---- Other modes -----------------------------------------------------

    /// `ctrl+c`, which quits from anywhere.
    pub fn is_interrupt(&self, k: &KeyEvent) -> bool {
        let key = Key::from_event(k);
        key.ctrl() && key.code == KeyCode::Char('c')
    }

    /// A key typed into a text input.
    pub fn edit(&self, k: &KeyEvent) -> Edit {
        let key = Key::from_event(k);
        if let Some(c) = key.text() {
            return Edit::Char(c);
        }
        match (key.code, key.ctrl()) {
            (KeyCode::Enter, _) => Edit::Submit,
            (KeyCode::Esc, _) => Edit::Cancel,
            (KeyCode::Backspace, _) => Edit::Backspace,
            (KeyCode::Char('u'), true) => Edit::ClearLine,
            (KeyCode::Char('r'), true) => Edit::Retry,
            (KeyCode::Up, _) => Edit::Up,
            (KeyCode::Down, _) => Edit::Down,
            // Readline's previous and next line, as in fzf.
            (KeyCode::Char('p'), true) => Edit::Up,
            (KeyCode::Char('n'), true) => Edit::Down,
            (KeyCode::PageUp, _) => Edit::PageUp,
            (KeyCode::PageDown, _) => Edit::PageDown,
            (KeyCode::Tab, _) => Edit::Tab,
            (KeyCode::BackTab, _) => Edit::BackTab,
            _ => Edit::None,
        }
    }

    /// A yes/no prompt: `y` or enter says yes, anything else no.
    pub fn confirm(&self, k: &KeyEvent) -> bool {
        let key = Key::from_event(k);
        key.mods.is_empty() && matches!(key.code, KeyCode::Enter | KeyCode::Char('y' | 'Y'))
    }

    /// A key on a sheet.
    pub fn sheet(&self, k: &KeyEvent) -> Option<Sheet> {
        let key = Key::from_event(k);
        if !key.mods.is_empty() {
            return None;
        }
        Some(match key.code {
            KeyCode::Enter => Sheet::Confirm,
            KeyCode::Esc => Sheet::Cancel,
            KeyCode::Char('i') => Sheet::Edit,
            KeyCode::Char('y') => Sheet::Yank,
            KeyCode::Char('v') => Sheet::Verdict,
            KeyCode::Char('t') => Sheet::Inline,
            KeyCode::Char('j') | KeyCode::Down => Sheet::Down,
            KeyCode::Char('k') | KeyCode::Up => Sheet::Up,
            KeyCode::PageDown => Sheet::PageDown,
            KeyCode::PageUp => Sheet::PageUp,
            KeyCode::Char('q') => Sheet::Quit,
            _ => return None,
        })
    }

    // ---- Rules -----------------------------------------------------------

    /// Every way the bindings break the four rules (see the module docs).
    pub fn check_rules(&self) -> Vec<String> {
        let mut broken = Vec::new();
        if let Err(e) = validate(&self.bindings) {
            broken.push(format!("rule 1: {e}"));
        }
        let key_of = |a: Action| -> Vec<&Trigger> { self.triggers(a).collect() };
        for (lower, upper) in [
            (Action::DecideAccept, Action::DecideAcceptFile),
            (Action::DecideFlag, Action::DecideFlagFile),
        ] {
            for (l, u) in key_of(lower).iter().zip(key_of(upper)) {
                let (Trigger::Keys(l), Trigger::Keys(u)) = (l, u) else {
                    continue;
                };
                let upper_of = |k: &Key| match k.code {
                    KeyCode::Char(c) => Key::plain(KeyCode::Char(c.to_ascii_uppercase())),
                    _ => *k,
                };
                if l.iter().map(upper_of).collect::<Vec<_>>() != *u {
                    broken.push(format!(
                        "rule 2: {} and {} are not lower and upper case",
                        lower.id(),
                        upper.id()
                    ));
                }
            }
        }
        const NEOVIM_UPPER: &[char] = &['G', 'N', 'K', 'V'];
        for (action, trigger) in &self.bindings {
            let Trigger::Keys(keys) = trigger else {
                if !action.finishes() {
                    broken.push(format!(
                        "rule 3: :{} runs {}, which neither finishes nor restarts",
                        trigger.label().trim_start_matches(':'),
                        action.id()
                    ));
                }
                continue;
            };
            if let [
                Key {
                    code: KeyCode::Char(c),
                    mods,
                },
            ] = keys.as_slice()
                && mods.is_empty()
                && c.is_ascii_uppercase()
                && !action.is_decision()
                && !NEOVIM_UPPER.contains(c)
            {
                broken.push(format!("rule 2: '{c}' runs {}", action.id()));
            }
            let first = keys[0];
            if keys.len() > 1 && first == Key::plain(KeyCode::Char('g')) && !action.moves() {
                broken.push(format!(
                    "rule 3: {} runs {}, which doesn't move",
                    trigger.notation(),
                    action.id()
                ));
            }
            if keys.len() > 1 && first == Key::plain(KeyCode::Char(' ')) && !action.opens() {
                broken.push(format!(
                    "rule 3: {} runs {}, which doesn't open anything",
                    trigger.notation(),
                    action.id()
                ));
            }
            if keys.contains(&Key::plain(KeyCode::Char('i'))) && *action != Action::NoteEdit {
                broken.push(format!(
                    "rule 4: {} runs {}, not note.edit",
                    trigger.notation(),
                    action.id()
                ));
            }
        }
        broken
    }
}

/// The default table with `over` in place of the keys of the actions it
/// names, and a warning for each default key an override took away.
fn bind(over: &[(Action, Vec<Trigger>)]) -> (Vec<(Action, Trigger)>, Vec<String>) {
    let taken: Vec<&Trigger> = over.iter().flat_map(|(_, t)| t).collect();
    let mut bindings = Vec::new();
    let mut warnings = Vec::new();
    for (action, _, keys, _) in TABLE {
        if let Some((_, triggers)) = over.iter().find(|(a, _)| a == action) {
            bindings.extend(triggers.iter().map(|t| (*action, t.clone())));
            continue;
        }
        for k in *keys {
            let t = Trigger::parse(k).expect("default keys parse");
            if taken.contains(&&t) {
                warnings.push(format!(
                    "[keys]: '{k}' is rebound, so '{}' no longer has it",
                    action.id()
                ));
            } else {
                bindings.push((*action, t));
            }
        }
    }
    (bindings, warnings)
}

/// v1's `[keys]` names (teer 0.1), each with its v1 default key and the
/// action it became; `None` where v2 dropped it: the swipe and Flagged
/// screens, their tiles and resolutions, the all-reviewed popup (`:w`
/// finishes now).
pub const V1_NAMES: &[(&str, &str, Option<Action>)] = &[
    ("quit", "q", Some(Action::ViewClose)),
    ("down", "j", Some(Action::MoveDown)),
    ("up", "k", Some(Action::MoveUp)),
    ("preview_down", "J", Some(Action::ScrollHalfDown)),
    ("preview_up", "K", Some(Action::ScrollHalfUp)),
    ("top", "g", Some(Action::MoveFirst)),
    ("bottom", "G", Some(Action::MoveLast)),
    ("open", "enter", Some(Action::ViewOpen)),
    ("swipe", "s", None),
    ("rerun", "r", Some(Action::ReviewRerun)),
    ("reset", "R", Some(Action::ReviewReset)),
    ("edit", "e", Some(Action::GotoEditor)),
    ("accept", "l", Some(Action::DecideAccept)),
    ("investigate", "i", Some(Action::DecideFlag)),
    ("flagged", "d", None),
    ("dive_recommend", "n", None),
    ("dive_visualize", "v", Some(Action::ToolDiagram)),
    ("dive_behavior", "t", Some(Action::ToolRunOldNew)),
    ("dive_disagree", "x", None),
    ("dive_tests", "c", Some(Action::ToolTests)),
    ("interpret_tests", "a", None),
    ("open_visual", "b", Some(Action::GotoBrowser)),
    ("resolve_ok", "o", None),
    ("resolve_fix", "f", None),
    ("edit_note", "m", Some(Action::NoteEdit)),
    ("notes", "N", Some(Action::ToolNotes)),
    ("dive_rerun", "r", None),
    ("prev_tile", "left", None),
    ("next_tile", "right", None),
    ("zoom", "z", None),
    ("undo", "u", Some(Action::HistoryUndo)),
    ("back", "esc", Some(Action::ViewBack)),
    ("help", "?", Some(Action::HelpOpen)),
    ("next_pending", "]", Some(Action::MoveNextPending)),
    ("prev_pending", "[", Some(Action::MovePrevPending)),
    ("prev_card", "h", None),
    ("next_group", "ctrl+d", Some(Action::MoveNextFile)),
    ("prev_group", "ctrl+u", Some(Action::MovePrevFile)),
    ("accept_file", "A", Some(Action::DecideAcceptFile)),
    ("grouping", "f", Some(Action::ToggleByFile)),
    ("approve_pr", "a", None),
    ("commit", "c", None),
    ("commits", "C", Some(Action::ToolHistory)),
    ("ask", "/", Some(Action::AskOpen)),
];

/// A `[keys]` value: Neovim notation, or v1's ([`v1_key`]).
fn parse_trigger(s: &str) -> Result<Trigger> {
    match v1_key(s) {
        Some(k) => Ok(Trigger::Keys(vec![k])),
        None => Trigger::parse(s),
    }
}

/// A key in v1's notation (`ctrl+d`, `shift+tab`, `enter`, `left`), which
/// `[keys]` still takes. `None` for anything else, a single character
/// included: it means the same in both.
fn v1_key(s: &str) -> Option<Key> {
    let (mods, name) = match s.rsplit_once('+') {
        Some((m, k)) if !m.is_empty() && !k.is_empty() => (m, k),
        _ => ("", s),
    };
    let mut modifiers = KeyModifiers::NONE;
    for m in mods.split('+') {
        modifiers |= match m.to_ascii_lowercase().as_str() {
            "ctrl" => KeyModifiers::CONTROL,
            "alt" => KeyModifiers::ALT,
            "shift" => KeyModifiers::SHIFT,
            "" if mods.is_empty() => KeyModifiers::NONE,
            _ => return None,
        };
    }
    let code = match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "backspace" => KeyCode::Backspace,
        _ if !modifiers.is_empty() && name.chars().count() == 1 => {
            KeyCode::Char(name.chars().next()?)
        }
        _ => return None,
    };
    Some(Key::from_event(&KeyEvent::new(code, modifiers)))
}

/// One action per trigger, no trigger the start of another (it could never
/// wait for the rest), and nothing a count or the command line takes.
fn validate(bindings: &[(Action, Trigger)]) -> Result<()> {
    for (i, (a, t)) in bindings.iter().enumerate() {
        if let Trigger::Keys(keys) = t {
            let first = keys[0];
            if first.mods.is_empty() && matches!(first.code, KeyCode::Char('1'..='9' | ':')) {
                bail!(
                    "'{}' ({}) is taken by counts or the command line",
                    t.notation(),
                    a.id()
                );
            }
        }
        for (b, u) in &bindings[i + 1..] {
            if t == u {
                bail!(
                    "'{}' is bound to both {} and {}",
                    t.notation(),
                    a.id(),
                    b.id()
                );
            }
            if let (Trigger::Keys(x), Trigger::Keys(y)) = (t, u) {
                let (short, long, sa, la) = if x.len() < y.len() {
                    (x, y, a, b)
                } else {
                    (y, x, b, a)
                };
                if short.len() < long.len() && long.starts_with(short) {
                    bail!(
                        "'{}' ({}) is the start of '{}' ({})",
                        Trigger::Keys(short.clone()).notation(),
                        sa.id(),
                        Trigger::Keys(long.clone()).notation(),
                        la.id()
                    );
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(km: &mut Keymap, keys: &str, now: Instant) -> Vec<Outcome> {
        parse_keys(keys)
            .unwrap()
            .into_iter()
            .map(|k| km.feed(&k.event(), now))
            .collect()
    }

    fn run(keys: &str) -> Outcome {
        let mut km = Keymap::default();
        let now = Instant::now();
        feed_all(&mut km, keys, now).pop().unwrap()
    }

    fn ran(action: Action, count: Option<u32>) -> Outcome {
        Outcome::Run { action, count }
    }

    #[test]
    fn every_default_binding_runs_its_action() {
        let km = Keymap::default();
        let mut seen = 0;
        for (action, trigger) in km.bindings() {
            let typed = match trigger {
                Trigger::Keys(_) => trigger.notation(),
                Trigger::Command(c) => format!(":{c}<cr>"),
            };
            assert_eq!(run(&typed), ran(action, None), "{typed}");
            seen += 1;
        }
        assert!(seen >= TABLE.len());
        for (action, ..) in TABLE {
            assert!(
                km.triggers(*action).next().is_some(),
                "{} has a key",
                action.id()
            );
        }
    }

    #[test]
    fn the_default_keymap_keeps_the_four_rules() {
        assert_eq!(Keymap::default().check_rules(), Vec::<String>::new());
    }

    #[test]
    fn the_rules_catch_a_bad_rebinding() {
        let over = HashMap::from([
            ("finish.yank".to_string(), KeySpec::One("gy".into())),
            ("view.back".to_string(), KeySpec::One("i".into())),
            ("tool.notes".to_string(), KeySpec::One("Z".into())),
        ]);
        let (km, _) = Keymap::new(&over, 1000).unwrap();
        let broken = km.check_rules().join("\n");
        assert!(broken.contains("rule 3: gy runs finish.yank"), "{broken}");
        assert!(broken.contains("rule 4: i runs view.back"), "{broken}");
        assert!(broken.contains("rule 2: 'Z' runs tool.notes"), "{broken}");
    }

    #[test]
    fn notation_round_trips() {
        for s in [
            "gg",
            "]c",
            "<space>t",
            "<c-r>",
            "<cr>",
            "<esc>",
            "<s-tab>",
            "<lt>",
            "G",
            "<pagedown>",
        ] {
            let keys = parse_keys(s).unwrap();
            assert_eq!(Trigger::Keys(keys).notation(), s);
        }
        assert_eq!(parse_keys("<C-D>").unwrap(), parse_keys("<c-d>").unwrap());
        assert!(parse_keys("<nope>").is_err());
        // An unclosed `<` is the character.
        assert_eq!(
            parse_keys("<").unwrap(),
            vec![Key::plain(KeyCode::Char('<'))]
        );
    }

    #[test]
    fn shift_and_ctrl_are_normalized() {
        let mut km = Keymap::default();
        let now = Instant::now();
        let shifted = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(km.feed(&shifted, now), ran(Action::MoveLast, None));
        let ctrl_upper = KeyEvent::new(KeyCode::Char('D'), KeyModifiers::CONTROL);
        assert_eq!(km.feed(&ctrl_upper, now), ran(Action::ScrollHalfDown, None));
        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(km.feed(&backtab, now), ran(Action::V1PrevTile, None));
    }

    #[test]
    fn a_prefix_waits_and_hints_what_can_follow() {
        let mut km = Keymap::default();
        let now = Instant::now();
        assert_eq!(feed_all(&mut km, "g", now), vec![Outcome::Pending]);
        assert_eq!(km.pending().as_deref(), Some("g"));
        let hint = km.hint();
        assert!(hint.contains(&("g".to_string(), Action::MoveFirst)));
        assert!(hint.contains(&("d".to_string(), Action::GotoDefinition)));
        assert!(hint.iter().all(|(_, a)| a.moves()));
        assert_eq!(
            feed_all(&mut km, "d", now),
            vec![ran(Action::GotoDefinition, None)]
        );
        assert_eq!(km.pending(), None);
        assert!(km.hint().is_empty());
    }

    #[test]
    fn a_prefix_times_out() {
        let mut km = Keymap::default();
        let now = Instant::now();
        feed_all(&mut km, "<space>", now);
        assert!(!km.tick(now + Duration::from_millis(900)));
        assert!(km.pending().is_some());
        assert!(km.tick(now + Duration::from_millis(1100)));
        assert_eq!(km.pending(), None);
        // `t` alone, after the timeout, is not `<space>t`.
        let late = now + Duration::from_millis(1200);
        assert_eq!(feed_all(&mut km, "t", late), vec![Outcome::None]);
    }

    #[test]
    fn the_timeout_is_configurable() {
        let (mut km, _) = Keymap::new(&HashMap::new(), 3000).unwrap();
        let now = Instant::now();
        feed_all(&mut km, "g", now);
        let later = now + Duration::from_millis(2000);
        assert_eq!(
            feed_all(&mut km, "g", later),
            vec![ran(Action::MoveFirst, None)]
        );
    }

    #[test]
    fn a_wrong_second_key_drops_the_sequence() {
        let mut km = Keymap::default();
        let now = Instant::now();
        assert_eq!(
            feed_all(&mut km, "]q", now),
            vec![Outcome::Pending, Outcome::None]
        );
        assert_eq!(km.pending(), None);
        assert_eq!(
            feed_all(&mut km, "j", now),
            vec![ran(Action::MoveDown, None)]
        );
    }

    #[test]
    fn counts_go_with_the_action() {
        assert_eq!(run("3a"), ran(Action::DecideAccept, Some(3)));
        assert_eq!(run("12j"), ran(Action::MoveDown, Some(12)));
        assert_eq!(run("2]c"), ran(Action::MoveNextHunk, Some(2)));
        assert_eq!(run("99999999j"), ran(Action::MoveDown, Some(MAX_COUNT)));
        // `0` alone is no count.
        assert_eq!(run("0"), Outcome::None);
        assert_eq!(run("10j"), ran(Action::MoveDown, Some(10)));
    }

    #[test]
    fn esc_drops_a_count_and_does_nothing_else() {
        let mut km = Keymap::default();
        let now = Instant::now();
        assert_eq!(
            feed_all(&mut km, "5<esc>", now),
            vec![Outcome::Pending, Outcome::None]
        );
        assert_eq!(km.pending(), None);
        // Without a count esc is view.back.
        assert_eq!(
            feed_all(&mut km, "<esc>", now),
            vec![ran(Action::ViewBack, None)]
        );
        // The count is gone: `a` is plain.
        assert_eq!(
            feed_all(&mut km, "a", now),
            vec![ran(Action::DecideAccept, None)]
        );
    }

    #[test]
    fn the_command_line_runs_its_four_commands() {
        assert_eq!(run(":w<cr>"), ran(Action::FinishOpen, None));
        assert_eq!(run(":e<cr>"), ran(Action::ReviewRerun, None));
        assert_eq!(run(":e!<cr>"), ran(Action::ReviewReset, None));
        assert_eq!(run(":q<cr>"), ran(Action::ViewClose, None));
    }

    #[test]
    fn an_unknown_command_is_an_error_and_nothing_else() {
        assert_eq!(run(":wq<cr>"), Outcome::Error("Not a command: :wq".into()));
        let mut km = Keymap::default();
        let now = Instant::now();
        feed_all(&mut km, ":x<cr>", now);
        assert_eq!(km.command_line(), None);
        assert_eq!(
            feed_all(&mut km, "j", now),
            vec![ran(Action::MoveDown, None)]
        );
    }

    #[test]
    fn the_command_line_edits_and_cancels() {
        let mut km = Keymap::default();
        let now = Instant::now();
        feed_all(&mut km, ":ex", now);
        assert_eq!(km.command_line(), Some("ex"));
        feed_all(&mut km, "<bs>", now);
        assert_eq!(km.command_line(), Some("e"));
        assert_eq!(feed_all(&mut km, "<esc>", now), vec![Outcome::None]);
        assert_eq!(km.command_line(), None);
        // Backspace on an empty line leaves it.
        feed_all(&mut km, ":<bs>", now);
        assert_eq!(km.command_line(), None);
        // A count before `:` is dropped.
        assert_eq!(run("3:w<cr>"), ran(Action::FinishOpen, None));
    }

    #[test]
    fn overrides_replace_keys_and_take_them_from_the_defaults() {
        let over = HashMap::from([(
            "decide.accept".to_string(),
            KeySpec::Many(vec!["l".into(), "<c-a>".into()]),
        )]);
        let (km, warnings) = Keymap::new(&over, 1000).unwrap();
        assert_eq!(km.labels(Action::DecideAccept), "l/^a");
        // `l` moved, so view.open keeps only enter.
        assert_eq!(km.labels(Action::ViewOpen), "⏎");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let mut km = km;
        assert_eq!(
            feed_all(&mut km, "a", Instant::now()),
            vec![Outcome::None],
            "`a` is free now"
        );
    }

    #[test]
    fn unknown_ids_warn_and_bad_keys_fail() {
        let over = HashMap::from([("reject".to_string(), KeySpec::One("l".into()))]);
        let (_, warnings) = Keymap::new(&over, 1000).unwrap();
        assert_eq!(
            warnings,
            vec!["[keys]: unknown action 'reject', ignored".to_string()]
        );
        let over = HashMap::from([("move.down".to_string(), KeySpec::One("<bogus>".into()))]);
        assert!(Keymap::new(&over, 1000).is_err());
        let over = HashMap::from([
            ("move.down".to_string(), KeySpec::One("x".into())),
            ("move.up".to_string(), KeySpec::One("x".into())),
        ]);
        let err = Keymap::new(&over, 1000).unwrap_err().to_string();
        assert!(err.contains("bound to both"), "{err}");
        let over = HashMap::from([("move.down".to_string(), KeySpec::One("g".into()))]);
        let err = Keymap::new(&over, 1000).unwrap_err().to_string();
        assert!(err.contains("is the start of"), "{err}");
        let over = HashMap::from([("move.down".to_string(), KeySpec::One("2".into()))]);
        assert!(Keymap::new(&over, 1000).is_err());
    }

    fn v1(entries: &[(&str, &str)]) -> (Keymap, Vec<String>) {
        let over = entries
            .iter()
            .map(|(k, v)| (k.to_string(), KeySpec::One(v.to_string())))
            .collect();
        Keymap::new(&over, 1000).unwrap()
    }

    #[test]
    fn every_v1_name_is_known_once_and_its_default_parses() {
        let names: Vec<&str> = V1_NAMES.iter().map(|(n, ..)| *n).collect();
        for (i, n) in names.iter().enumerate() {
            assert!(!names[i + 1..].contains(n), "{n} twice");
            assert!(Action::from_id(n).is_none(), "{n} is a v2 id");
        }
        for (name, default, _) in V1_NAMES {
            assert!(parse_trigger(default).is_ok(), "{name} = {default}");
        }
        // v1's `Action::ALL` had 44 names.
        assert_eq!(V1_NAMES.len(), 44);
    }

    #[test]
    fn v1_names_rebind_their_v2_action() {
        let (km, warnings) = v1(&[("accept", "y"), ("next_group", "ctrl+n"), ("down", "e")]);
        assert_eq!(km.labels(Action::DecideAccept), "y");
        assert_eq!(km.labels(Action::MoveNextFile), "^n");
        assert_eq!(km.labels(Action::MoveDown), "e");
        // `y` came from finish.yank: said, as for any v2 id.
        assert_eq!(
            warnings,
            vec!["[keys]: 'y' is rebound, so 'finish.yank' no longer has it".to_string()]
        );
        let mut km = km;
        let now = Instant::now();
        assert_eq!(
            feed_all(&mut km, "y", now),
            vec![ran(Action::DecideAccept, None)]
        );
        assert_eq!(
            feed_all(&mut km, "<c-n>", now),
            vec![ran(Action::MoveNextFile, None)]
        );
    }

    #[test]
    fn a_v1_default_leaves_the_v2_default() {
        // What v1's docs showed: the keys v1 had anyway.
        let (km, warnings) = v1(&[
            ("accept", "l"),
            ("investigate", "i"),
            ("open", "enter"),
            ("next_group", "ctrl+d"),
            ("preview_up", "K"),
        ]);
        assert!(warnings.is_empty(), "{warnings:?}");
        let def = Keymap::default();
        for a in Action::all() {
            assert_eq!(km.labels(a), def.labels(a), "{}", a.id());
        }
    }

    #[test]
    fn removed_v1_names_warn_once_each_and_nothing_fails() {
        let (km, warnings) = v1(&[
            ("swipe", "w"),
            ("zoom", "z"),
            ("resolve_fix", "x"),
            ("approve_pr", "ctrl+a"),
        ]);
        assert_eq!(
            warnings,
            vec![
                "`[keys].approve_pr` is no longer used".to_string(),
                "`[keys].resolve_fix` is no longer used".to_string(),
                "`[keys].swipe` is no longer used".to_string(),
                "`[keys].zoom` is no longer used".to_string(),
            ]
        );
        let def = Keymap::default();
        for a in Action::all() {
            assert_eq!(km.labels(a), def.labels(a), "{}", a.id());
        }
    }

    #[test]
    fn v1_keys_that_dont_fit_are_skipped_not_fatal() {
        // `z` is the start of `za`, `zf`; `bogus+x` isn't a key; the v2 id
        // wins over the v1 name of the same action.
        let over = HashMap::from([
            ("bottom".to_string(), KeySpec::One("z".into())),
            ("top".to_string(), KeySpec::One("<nope>".into())),
            ("accept".to_string(), KeySpec::One("y".into())),
            ("decide.accept".to_string(), KeySpec::One("e".into())),
        ]);
        let (km, warnings) = Keymap::new(&over, 1000).unwrap();
        let joined = warnings.join("\n");
        assert!(
            joined.contains(
                "`[keys].bottom` (now \"move.last\") is ignored: 'z' (move.last) is the start of"
            ),
            "{joined}"
        );
        assert!(joined.contains("`[keys].top` is ignored"), "{joined}");
        assert!(
            joined.contains("`[keys].accept` is ignored: \"decide.accept\" is set"),
            "{joined}"
        );
        assert_eq!(km.labels(Action::MoveLast), "G/end");
        assert_eq!(km.labels(Action::DecideAccept), "e");
        assert_eq!(km.labels(Action::FinishYank), "y");
    }

    #[test]
    fn v1_key_names_are_read_for_v2_ids_too() {
        for (v1, v2) in [
            ("ctrl+d", "<c-d>"),
            ("Ctrl+Shift+X", "<c-x>"),
            ("alt+j", "<a-j>"),
            ("shift+tab", "<s-tab>"),
            ("shift+j", "J"),
            ("enter", "<cr>"),
            ("Esc", "<esc>"),
            ("space", "<space>"),
            ("left", "<left>"),
            ("backspace", "<bs>"),
        ] {
            assert_eq!(
                parse_trigger(v1).unwrap(),
                Trigger::parse(v2).unwrap(),
                "{v1}"
            );
        }
        // Neovim notation and single keys are what they were.
        for s in ["j", "+", "gg", "<c-d>", ":w", "<space>t"] {
            assert_eq!(parse_trigger(s).unwrap(), Trigger::parse(s).unwrap(), "{s}");
        }
        let (km, warnings) = v1(&[("move.down", "ctrl+n")]);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(km.labels(Action::MoveDown), "^n");
    }

    #[test]
    fn text_inputs_take_every_letter() {
        let km = Keymap::default();
        let key = |s: &str| parse_keys(s).unwrap()[0].event();
        assert_eq!(km.edit(&key("j")), Edit::Char('j'));
        assert_eq!(km.edit(&key("q")), Edit::Char('q'));
        assert_eq!(km.edit(&key(" ")), Edit::Char(' '));
        assert_eq!(km.edit(&key("<cr>")), Edit::Submit);
        assert_eq!(km.edit(&key("<esc>")), Edit::Cancel);
        assert_eq!(km.edit(&key("<bs>")), Edit::Backspace);
        assert_eq!(km.edit(&key("<c-u>")), Edit::ClearLine);
        assert_eq!(km.edit(&key("<c-r>")), Edit::Retry);
        assert!(km.confirm(&key("y")));
        assert!(km.confirm(&key("<cr>")));
        assert!(!km.confirm(&key("n")));
        assert!(!km.confirm(&key("<esc>")));
        assert_eq!(km.sheet(&key("i")), Some(Sheet::Edit));
        assert_eq!(km.sheet(&key("<cr>")), Some(Sheet::Confirm));
        assert!(km.is_interrupt(&key("<c-c>")));
    }

    #[test]
    fn labels_show_sequences_and_commands() {
        let km = Keymap::default();
        assert_eq!(km.label(Action::MoveFirst), "gg");
        assert_eq!(km.label(Action::ToolTests), "␣t");
        assert_eq!(km.label(Action::ScrollHalfDown), "^d");
        assert_eq!(km.label(Action::FinishOpen), ":w");
        assert_eq!(km.labels(Action::ViewOpen), "l/⏎");
    }
}
