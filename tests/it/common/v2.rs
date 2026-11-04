//! Key replay for v2: a [`Driver`] around [`App`] on a synthetic review (no
//! repo, no agent, no network). Keys go in as Neovim notation (`"3a:w<cr>"`)
//! through [`App::on_key`], one key event at a time, the way the terminal
//! sends them; the effects come back as data and nothing runs.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use teer::config::Config;
use teer::keymap::{Keymap, parse_keys};
use teer::review::model::{Decision, Prep, Review, SessionRef, Unit, UnitKind, UnitStatus};
use teer::session::app::{Effect, Overlay, Screen};
use teer::tui::app::App;
use teer::tui::ui;

/// Unit `u{i}` (symbol `s{i}`), a changed function in `file` with no
/// statements, pending.
pub fn unit(i: usize, file: &str) -> Unit {
    Unit {
        id: format!("u{i}"),
        file: PathBuf::from(file),
        symbol: format!("s{i}"),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: None,
        new_range: None,
        renamed_from: None,
        fingerprint: format!("fp{i}"),
        changes: Vec::new(),
        prep: Prep::Skipped,
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Pending,
        summary: None,
        nbc_reason: None,
        dive: Default::default(),
        accepted_by: None,
        reasons: Vec::new(),
        dive_pending: Default::default(),
        checks: Default::default(),
    }
}

/// `n` units, three per file (`f0.rs`: u0-u2, `f1.rs`: u3-u5, ...), listed
/// in that order (up to 30 units, so the file names sort as numbers do).
pub fn units(n: usize) -> Vec<Unit> {
    assert!(n <= 30, "f10.rs would sort before f2.rs");
    (0..n).map(|i| unit(i, &format!("f{}.rs", i / 3))).collect()
}

/// A working-tree review of `units`.
pub fn review(units: Vec<Unit>) -> Review {
    Review {
        id: "v2".into(),
        repo_root: PathBuf::from("/v2/repo"),
        base: "base".into(),
        head: teer::git::WORKTREE.into(),
        head_commit: "head".into(),
        agent_session: None,
        pr: None,
        intent: Default::default(),
        created_at: chrono::Utc::now(),
        files: Vec::new(),
        units,
        skipped: Vec::new(),
        graph: Default::default(),
        questions: Vec::new(),
        version: 0,
        history: Default::default(),
        branch: None,
        cursor: None,
    }
}

/// Type `keys` (Neovim notation) into `app`; the effects of every key, in order.
pub fn press(app: &mut App, keys: &str) -> Vec<Effect> {
    parse_keys(keys)
        .unwrap_or_else(|e| panic!("keys {keys:?}: {e}"))
        .into_iter()
        .flat_map(|k| app.on_key(k.event()))
        .collect()
}

/// The screen `app` draws at `w` x `h`, one line per row.
pub fn screen(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::draw(f, app)).unwrap();
    let buf = t.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// An [`App`] to type into, and every effect it returned so far.
pub struct Driver {
    pub app: App,
    pub effects: Vec<Effect>,
}

impl Driver {
    /// `n` pending units, three per file (see [`units`]), default keys.
    pub fn new(n: usize) -> Driver {
        Driver::of(units(n))
    }

    /// A review of `units` with the default keys.
    pub fn of(units: Vec<Unit>) -> Driver {
        Driver::with(review(units), Keymap::default())
    }

    /// `review` with `keys`; teer's data and config point at a temp dir.
    pub fn with(review: Review, keys: Keymap) -> Driver {
        super::isolate_env();
        let app = App::new(review, Config::default(), keys);
        assert_eq!(app.screen, Screen::Queue);
        Driver {
            app,
            effects: Vec::new(),
        }
    }

    /// Type `keys` (Neovim notation: `3a`, `]f`, `<space>n`, `:w<cr>`); the
    /// effects of these keys (also added to [`Driver::effects`]).
    pub fn keys(&mut self, keys: &str) -> Vec<Effect> {
        let out = press(&mut self.app, keys);
        self.effects.extend(out.iter().cloned());
        out
    }

    /// Let the pending-key timeout (1000 ms by default) pass. True when a
    /// pending sequence was dropped.
    pub fn timeout(&mut self) -> bool {
        self.app
            .keys
            .tick(Instant::now() + Duration::from_millis(1100))
    }

    /// The decision of the unit at list position `pos`.
    pub fn decision(&self, pos: usize) -> Decision {
        self.app.review.units[self.app.order[pos]].decision
    }

    /// Every listed unit's decision, in list order.
    pub fn decisions(&self) -> Vec<Decision> {
        (0..self.app.order.len())
            .map(|p| self.decision(p))
            .collect()
    }

    /// The decision of unit `id`.
    pub fn decision_of(&self, id: &str) -> Decision {
        self.app.review.unit(id).expect("unit exists").decision
    }

    /// List positions whose unit is `d`.
    pub fn positions(&self, d: Decision) -> Vec<usize> {
        (0..self.app.order.len())
            .filter(|&p| self.decision(p) == d)
            .collect()
    }

    /// Put the list cursor on position `pos`, saved there as a move would
    /// (so the next key has no cursor to save).
    pub fn select(&mut self, pos: usize) {
        assert!(pos < self.app.order.len(), "row {pos} is listed");
        self.app.sel = pos;
        self.app.review.cursor = self.app.selected_id();
    }

    /// The list position of unit `id`.
    pub fn pos(&self, id: &str) -> usize {
        (0..self.app.order.len())
            .find(|&p| self.app.review.units[self.app.order[p]].id == id)
            .unwrap_or_else(|| panic!("{id} is listed"))
    }

    /// Put the list cursor on unit `id` (see [`Driver::select`]).
    pub fn select_id(&mut self, id: &str) {
        self.select(self.pos(id));
    }

    /// The list cursor's position.
    pub fn sel(&self) -> usize {
        self.app.sel
    }

    /// The selected unit's id.
    pub fn selected(&self) -> Option<String> {
        self.app.selected_id()
    }

    pub fn overlay(&self) -> &Overlay {
        &self.app.overlay
    }

    pub fn screen(&self) -> Screen {
        self.app.screen
    }

    pub fn status(&self) -> &str {
        &self.app.status
    }

    /// The note on unit `id`.
    pub fn note(&self, id: &str) -> &str {
        &self.app.review.unit(id).expect("unit exists").dive.note
    }

    /// The screen drawn at `w` x `h`.
    pub fn render(&self, w: u16, h: u16) -> String {
        screen(&self.app, w, h)
    }

    /// Steps `u` can take back.
    pub fn undo_steps(&self) -> usize {
        self.app.review.history.undo.len()
    }
}

// ---- Opening a review as `teer` does (Phases 4-6) --------------------------

impl Driver {
    /// `review` under `cfg`, opened as `teer --no-agent` opens it:
    /// [`App::new`], then [`App::start`] without an agent. What `start`
    /// returned is in [`Driver::effects`].
    pub fn open(review: Review, cfg: Config) -> Driver {
        Driver::start(review, cfg, false)
    }

    /// As [`Driver::open`], with an agent set up: a fresh session in the
    /// review's repo, as `teer` runs one when no session wrote the change.
    pub fn open_with_agent(review: Review, cfg: Config) -> Driver {
        Driver::start(review, cfg, true)
    }

    /// `review` under `cfg`, started the way the runtime starts it
    /// (`tui::run`): `agent` is whether an agent is set up. Teer's data and
    /// config point at a temp dir.
    pub fn start(review: Review, cfg: Config, agent: bool) -> Driver {
        super::isolate_env();
        let mut app = App::new(review, cfg, Keymap::default());
        let session = SessionRef::fresh("mock", app.review.repo_root.clone());
        let note = (!agent).then(|| teer::session::app::NO_AGENT.to_string());
        let effects = app.start(agent, note, Vec::new(), Some(session));
        Driver { app, effects }
    }

    /// The bottom row of the screen drawn at `w` x `h`.
    pub fn bar(&self, w: u16, h: u16) -> String {
        self.render(w, h)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string()
    }

    /// The id of the unit at list position `pos`.
    pub fn id_at(&self, pos: usize) -> String {
        self.app.review.units[self.app.order[pos]].id.clone()
    }

    /// Save the review when `effects` ask to, as the runtime does.
    pub fn save_if_asked(&self, effects: &[Effect]) {
        if effects.contains(&Effect::Save) {
            teer::review::store::save(&self.app.review).unwrap();
        } else if effects.contains(&Effect::SaveCursor) {
            teer::review::store::save_cursor(&self.app.review).unwrap();
        }
    }
}

/// Where `text` is first drawn on `screen` (one line per row): its column
/// and row, in cells.
pub fn find(screen: &str, text: &str) -> Option<(u16, u16)> {
    screen.lines().enumerate().find_map(|(y, row)| {
        row.find(text)
            .map(|i| (row[..i].chars().count() as u16, y as u16))
    })
}
