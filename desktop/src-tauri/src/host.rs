//! One review per window. A task owns the review's session (the
//! [`teer::session::App`] and its [`Runtime`]): it takes the window's
//! keys, actions and clicks, carries out the effects, and sends the window
//! the view ([`teer::session::view`]) whenever it changed. Effects that in
//! the terminal take over the screen are the window's (see [`crate::front`]).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, EventTarget};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use teer::keymap::{Action, parse_keys};
use teer::review::store;
use teer::session::view::{self, Hint, TileId};
use teer::session::{App, AppEvent, Effect, Runtime};
use teer::start::Begun;
use teer::syntax::Cache;

/// What the window sends its review.
pub enum Input {
    /// Keys in Neovim notation (`a`, `<space>t`, `<c-u>`), resolved by
    /// teer's keymap so `[keys]` rebinding applies.
    Keys(String),
    /// An action by its id (`decide.accept`), as a click or a menu runs it.
    Action {
        id: String,
        count: Option<u32>,
    },
    Click(Click),
    /// Text pasted while something is typed: each character as a key.
    Paste(String),
    /// What a front-end handoff came back with (see [`crate::front`]).
    Returned(Returned),
    /// Every action with its keys, for the command palette.
    Actions(oneshot::Sender<Vec<Hint>>),
    /// The window is closing, or hands the review to the terminal: save,
    /// let go of the review, then say so on the sender.
    Close(Option<oneshot::Sender<()>>),
}

/// Where a click landed.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind")]
pub enum Click {
    /// A queue row: the cursor goes there.
    Row { pos: usize },
    /// A unit: the unit view opens on it.
    Open { id: String },
    /// A tile of the unit view, or its text (`None`): it takes the focus.
    Tile { id: Option<TileId> },
    /// A tile's close button.
    CloseTile { id: TileId },
    /// The unit view's way back.
    Back,
    /// An entry of the flagged list.
    Flagged { i: usize },
    /// A link in the ask conversation.
    Link { i: usize },
    /// An item of the overlay's list: a step of the sheet, a note, a jump.
    Item { i: usize },
    /// A proposed hunk of the unit on show: `fix.apply` then puts it in.
    Fix { i: usize },
}

/// What a handoff the window ran came back with.
pub enum Returned {
    /// The agent CLI the notes went to exited (`Ok`), or couldn't start.
    Agent {
        cli: teer::config::Provider,
        ran: std::io::Result<std::process::ExitStatus>,
    },
    /// The text written in the window for the commit message or the
    /// review summary; `None` when it was cancelled.
    Written(Option<String>),
    /// The commit ran.
    Committed(teer::commit::Outcome),
    /// The settings were saved.
    Settings(Box<teer::config::Config>),
    /// A status line to show.
    Status(String),
}

/// A window's review, as the commands reach it.
#[derive(Clone)]
pub struct Host {
    pub tx: UnboundedSender<Input>,
    /// The last view sent, for a window that loads again.
    pub view: Arc<Mutex<Option<serde_json::Value>>>,
}

/// An action the window scrolls for.
#[derive(Debug, Clone, Serialize)]
struct Scroll {
    action: &'static str,
    count: u32,
}

/// What the window is told when its review is over.
#[derive(Debug, Clone, Serialize)]
pub struct Ended {
    /// What `:w` did (a commit, a post), as teer prints it on leaving.
    pub line: Option<String>,
}

/// Start the review `begun` in window `label`. `release` runs once it is
/// over (a pull request's checkout is let go).
pub fn spawn(
    handle: AppHandle,
    label: String,
    begun: Begun,
    release: Box<dyn FnOnce() + Send>,
) -> Host {
    let (tx, inputs) = unbounded_channel();
    let view = Arc::new(Mutex::new(None));
    let host = Host {
        tx: tx.clone(),
        view: view.clone(),
    };
    tauri::async_runtime::spawn(async move {
        let Begun {
            app,
            runtime,
            events,
            initial,
            lock,
        } = begun;
        let mut s = Session {
            handle,
            label,
            notice: Notice::of(&app),
            app,
            rt: runtime,
            syntax: Cache::default(),
            view,
            tx,
            committing: false,
        };
        let (ended, closed) = s.run(initial, inputs, events).await;
        // Saved: another window or the terminal may have it now.
        drop(lock);
        release();
        {
            use tauri::Manager;
            let windows = s.handle.state::<crate::commands::Windows>();
            if let Ok(mut map) = windows.0.lock()
                && let Some(state) = map.get_mut(&s.label)
            {
                state.host = None;
                state.what = None;
            }
        }
        let _ = s
            .handle
            .emit_to(EventTarget::webview_window(&s.label), "ended", ended);
        if let Some(done) = closed {
            let _ = done.send(());
        }
    });
    host
}

/// A review on its task.
pub struct Session {
    pub handle: AppHandle,
    pub label: String,
    pub app: App,
    pub rt: Runtime,
    syntax: Cache,
    view: Arc<Mutex<Option<serde_json::Value>>>,
    /// The window's own channel, for handoffs to come back on.
    pub tx: UnboundedSender<Input>,
    /// A commit is on its way: `⏎` again doesn't start another.
    pub committing: bool,
    /// What the review was when it was last looked at for news.
    notice: Notice,
}

/// What is news in a review, for a notification while the window is
/// behind others: prepare finished, a second model disputes a unit.
#[derive(Debug, Clone, Copy)]
struct Notice {
    preparing: bool,
    disputed: usize,
}

impl Notice {
    fn of(app: &App) -> Notice {
        let (done, total) = app.prep_progress();
        Notice {
            preparing: done < total,
            disputed: app.levels.iter().filter(|l| !l.doubts.is_empty()).count(),
        }
    }
}

/// How often the review looks at its timers: key prefixes, the watched tree.
const TICK: Duration = Duration::from_millis(100);
/// Fewest milliseconds between two views sent while answers stream in.
const FRAME: Duration = Duration::from_millis(33);

impl Session {
    async fn run(
        &mut self,
        initial: Vec<Effect>,
        mut inputs: UnboundedReceiver<Input>,
        mut events: UnboundedReceiver<AppEvent>,
    ) -> (Ended, Option<oneshot::Sender<()>>) {
        let mut closed = None;
        let mut quit = self.exec_all(initial);
        let mut tick = tokio::time::interval(TICK);
        let mut dirty = true;
        let mut sent = Instant::now() - FRAME;
        while !quit {
            if dirty && sent.elapsed() >= FRAME {
                self.send_view();
                sent = Instant::now();
                dirty = false;
            }
            // Streamed answers come fast: one view per frame at most.
            let wait = FRAME.saturating_sub(sent.elapsed());
            tokio::select! {
                input = inputs.recv() => {
                    let Some(input) = input else { break };
                    if let Input::Close(done) = input {
                        closed = done;
                        break;
                    }
                    match self.on_input(input) {
                        Some(effects) => quit |= self.exec_all(effects),
                        None => break,
                    }
                    self.scroll_natively();
                    dirty = true;
                }
                Some(ev) = events.recv() => {
                    quit |= self.on_event(ev);
                    while let Ok(ev) = events.try_recv() {
                        quit |= self.on_event(ev);
                    }
                    self.news();
                    dirty = true;
                }
                _ = tokio::time::sleep(wait), if dirty => {}
                _ = tick.tick() => {
                    let now = Instant::now();
                    self.app.tick = self.app.tick.wrapping_add(1);
                    // A key prefix waiting past `timeoutlen` is dropped.
                    dirty |= self.app.keys.tick(now);
                    // Waiting for copied notes' changes: look at the tree.
                    self.rt.watch(&mut self.app, now);
                }
            }
        }
        if let Err(e) = store::save(&self.app.review) {
            eprintln!("teer: couldn't save the review: {e:#}");
        }
        self.send_view();
        let ended = Ended {
            line: self.app.finish.exit_line.clone(),
        };
        (ended, closed)
    }

    /// Say what is news since the last look, when the window is behind.
    fn news(&mut self) {
        let now = Notice::of(&self.app);
        let subject = self.app.review.subject();
        if self.notice.preparing && !now.preparing {
            crate::notify::send(
                &self.handle,
                &self.label,
                "The review is prepared",
                &format!("{subject}: every unit is described."),
            );
        }
        if now.disputed > self.notice.disputed {
            let n = now.disputed - self.notice.disputed;
            crate::notify::send(
                &self.handle,
                &self.label,
                "A second model disputes",
                &format!(
                    "{subject}: {n} unit{} described differently.",
                    if n == 1 { "" } else { "s" }
                ),
            );
        }
        self.notice = now;
    }

    /// The window scrolls its panes itself: an action that scrolls, or in
    /// the unit view moves, is handed to it too.
    fn scroll_natively(&mut self) {
        use Action::*;
        let Some((a, count)) = self.app.ran.take() else {
            return;
        };
        let scrolls = matches!(
            a,
            ScrollHalfDown
                | ScrollHalfUp
                | ScrollPageDown
                | ScrollPageUp
                | MoveNextHunk
                | MovePrevHunk
        ) || (self.app.screen == teer::session::Screen::Unit
            && self.app.overlay == teer::session::Overlay::None
            && matches!(a, MoveDown | MoveUp | MoveFirst | MoveLast));
        if scrolls {
            let _ = self.handle.emit_to(
                EventTarget::webview_window(&self.label),
                "scroll",
                Scroll {
                    action: a.id(),
                    count: count.unwrap_or(1),
                },
            );
        }
    }

    /// What came back from the runtime. True to quit.
    fn on_event(&mut self, ev: AppEvent) -> bool {
        if matches!(ev, AppEvent::Rerun(Ok(_))) {
            // The code changed: colour it again.
            self.syntax.clear();
        }
        let effects = self.app.on_app_event(ev);
        self.exec_all(effects)
    }

    /// The window's input; `None` to stop.
    fn on_input(&mut self, input: Input) -> Option<Vec<Effect>> {
        let app = &mut self.app;
        Some(match input {
            Input::Keys(keys) => match parse_keys(&keys) {
                Ok(keys) => keys.into_iter().flat_map(|k| app.on_key(k)).collect(),
                Err(e) => {
                    app.status = format!("{e:#}");
                    Vec::new()
                }
            },
            Input::Action { id, count } => match Action::from_id(&id) {
                // As its key would: an open panel takes it as its own.
                Some(a) => app.press(a, count),
                None => Vec::new(),
            },
            Input::Click(c) => match c {
                Click::Row { pos } if pos < app.order.len() => app.move_sel(pos),
                Click::Row { .. } => Vec::new(),
                Click::Open { id } => app.open_unit(id),
                Click::Tile { id } => {
                    app.unit_click_tile(id.map(TileId::panel));
                    Vec::new()
                }
                Click::Back => {
                    app.unit_click_back();
                    Vec::new()
                }
                Click::CloseTile { id } => {
                    app.unit_click_close(id.panel());
                    Vec::new()
                }
                Click::Flagged { i } => {
                    app.flagged_click(i);
                    Vec::new()
                }
                Click::Link { i } => app.follow_ask_link(i),
                Click::Item { i } => app.pick(i),
                Click::Fix { i } => {
                    app.select_fix(i);
                    Vec::new()
                }
            },
            Input::Paste(text) => text
                .chars()
                .filter(|c| !c.is_control())
                .flat_map(|c| app.on_key(teer::keymap::Key::plain(teer::keymap::KeyCode::Char(c))))
                .collect(),
            Input::Returned(r) => return Some(crate::front::returned(self, r)),
            Input::Actions(reply) => {
                // `:app` is the terminal's way here.
                let hints = Action::all()
                    .filter(|a| !app.keys.label(*a).is_empty() && *a != Action::FinishApp)
                    .map(|a| Hint {
                        keys: app.keys.labels(a),
                        action: a.id(),
                        what: a.describe().into(),
                    })
                    .collect();
                let _ = reply.send(hints);
                Vec::new()
            }
            Input::Close(_) => return None,
        })
    }

    /// Carry out `effects`: the window's own, the runtime the rest. True
    /// to quit.
    pub fn exec_all(&mut self, effects: Vec<Effect>) -> bool {
        let mut quit = false;
        for e in effects {
            if let Some(e) = crate::front::take(self, e) {
                quit |= self.rt.exec(&mut self.app, e)
            }
        }
        quit
    }

    fn send_view(&self) {
        let v = view::snapshot(&self.app, Some(&self.syntax));
        let Ok(json) = serde_json::to_value(&v) else {
            return;
        };
        let _ = self
            .handle
            .emit_to(EventTarget::webview_window(&self.label), "view", &json);
        if let Ok(mut last) = self.view.lock() {
            *last = Some(json);
        }
    }
}
