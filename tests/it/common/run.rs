//! Running a review the way `tui::run` does, without a terminal. A
//! [`Session`] owns the [`App`] and a [`Runtime`], carries out every effect
//! (the ones `tui::run` gives the terminal for are done here without one:
//! git commits with its output captured, the agent CLI runs to its exit),
//! and feeds what the runtime's background work sends back to the app, with
//! the run loop's 100 ms tick. The model is a [`MockAdapter`] ([`model`]);
//! gh, the browser opener and the agent CLIs are scripts ([`Fakes`]); git is
//! real, in temp repos.
//!
//! [`launch`] runs `cli::run_on` up to the review (start detection, saved
//! state, agent and session discovery), with the screens stood in for.
//!
//! Once the agent CLI exits, the session does what `tui::run` does with
//! the terminal back: `Runtime::agent_exited` (the exit is the signal, the
//! review re-runs). The tick drives `Runtime::watch` as the run loop's does.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use teer::agent::AgentAdapter;
use teer::agent::live::Live;
use teer::agent::mock::MockAdapter;
use teer::approve::Tools;
use teer::cli::Screens;
use teer::commit::{self, Io};
use teer::config::{Config, Provider};
use teer::keymap::{Action, Keymap, parse_keys};
use teer::review::BuildOptions;
use teer::review::model::{Decision, Review, SessionRef};
use teer::review::store;
use teer::tui::app::{App, AppEvent, Effect};
use teer::tui::setup::{Ended, Opened};
use teer::tui::{Launch, Runtime};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use super::sandbox::script;
use super::{FixtureRepo, isolate_env};

// ---- The process ---------------------------------------------------------------

/// Set once per test binary, before anything reads them: teer's data and
/// config in a temp dir ([`isolate_env`]); a `PATH` that starts with
/// stand-ins for the clipboard programs (so nothing reaches the real
/// clipboard) and for `claude` and `codex` (installed, doing nothing: the
/// agent a test hands notes to is its own [`Fakes`] script); Claude Code's
/// home (`CLAUDE_CONFIG_DIR`) empty. Where Claude Code keeps sessions.
pub fn env() -> PathBuf {
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    isolate_env();
    HOME.get_or_init(|| {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for clip in ["pbcopy", "wl-copy", "xclip", "xsel"] {
            script(&bin.join(clip), "cat > \"$(dirname \"$0\")/clipboard\"");
        }
        for agent in ["claude", "codex"] {
            script(&bin.join(agent), "exit 0");
        }
        std::fs::create_dir_all(d.path().join("claude/projects")).unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![bin];
        dirs.extend(std::env::split_paths(&path));
        // SAFETY: set once, before the tests of this binary read them.
        unsafe {
            std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
            std::env::set_var("CLAUDE_CONFIG_DIR", d.path().join("claude"));
        }
        d
    })
    .path()
    .join("claude/projects")
}

/// Claude Code's folder for sessions started in `dir`.
pub fn project_dir(dir: &Path) -> PathBuf {
    let name: String = dir
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    env().join(name)
}

/// A Claude Code session `id`, started in `repo` just now, that edited
/// `file` there: the session `teer` finds as the one that wrote the change.
pub fn wrote_session(repo: &Path, id: &str, file: &str) {
    let dir = project_dir(repo);
    std::fs::create_dir_all(&dir).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    let lines = [
        json!({"type": "user", "cwd": repo, "message": {"content": "change it"}}),
        json!({
            "type": "assistant", "cwd": repo, "timestamp": now,
            "message": {"content": [{"type": "tool_use", "name": "Edit",
                "input": {"file_path": repo.join(file)}}]}
        }),
    ];
    let body: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    std::fs::write(dir.join(format!("{id}.jsonl")), body.join("\n") + "\n").unwrap();
}

// ---- Fixture repos ---------------------------------------------------------------

/// A repo on `main` whose commits use its own identity, no signing and its
/// own (empty) hooks, whatever the user's global git config says.
pub fn repo() -> FixtureRepo {
    env();
    let r = FixtureRepo::new();
    r.git(&["config", "user.name", "Ada Lovelace"]);
    r.git(&["config", "user.email", "ada@example.com"]);
    r.git(&["config", "commit.gpgsign", "false"]);
    let hooks = r.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    r.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    r
}

static NEXT_PR: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(4800);

/// A pull request on "GitHub": a bare repo standing in for
/// `https://github.com/acme/shop.git` (git rewrites the URL to it), with
/// `refs/pull/<n>/head` and `/merge`. The clone is on `main`, clean.
pub struct GithubPr {
    pub repo: FixtureRepo,
    _bare: tempfile::TempDir,
    pub n: u32,
    pub head: String,
}

impl GithubPr {
    /// `files` committed on `main`, then changed to `changed` on the PR.
    pub fn new(files: &[(&str, &str)], changed: &[(&str, &str)]) -> GithubPr {
        let n = NEXT_PR.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let repo = repo();
        for (f, c) in files {
            repo.write(f, c);
        }
        repo.commit("init");
        repo.git(&["checkout", "-q", "-b", "pr"]);
        for (f, c) in changed {
            repo.write(f, c);
        }
        repo.commit("pr change");
        let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.git(&["checkout", "-q", "main"]);
        let bare = tempfile::tempdir().unwrap();
        let st = std::process::Command::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(bare.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(st.success());
        let path = bare.path().canonicalize().unwrap();
        repo.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/shop.git",
        ]);
        repo.git(&[
            "config",
            &format!("url.{}.insteadOf", path.display()),
            "https://github.com/acme/shop.git",
        ]);
        repo.git(&["push", "-q", "origin", "main"]);
        repo.git(&["push", "-q", "origin", &format!("pr:refs/pull/{n}/head")]);
        repo.git(&["checkout", "-q", "--detach", "main"]);
        repo.git(&["merge", "-q", "--no-ff", "pr", "-m", "merge"]);
        repo.git(&["push", "-q", "origin", &format!("HEAD:refs/pull/{n}/merge")]);
        repo.git(&["checkout", "-q", "main"]);
        GithubPr {
            repo,
            _bare: bare,
            n,
            head,
        }
    }

    pub fn url(&self) -> String {
        format!("https://github.com/acme/shop/pull/{}", self.n)
    }

    /// The author pushes `changed` on top of the PR: its new head.
    pub fn push(&mut self, changed: &[(&str, &str)]) -> String {
        self.repo.git(&["checkout", "-q", "pr"]);
        for (f, c) in changed {
            self.repo.write(f, c);
        }
        self.repo.commit("more");
        self.head = self.repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        let n = self.n;
        self.repo.git(&[
            "push",
            "-q",
            "-f",
            "origin",
            &format!("pr:refs/pull/{n}/head"),
        ]);
        self.repo.git(&["checkout", "-q", "--detach", "main"]);
        self.repo
            .git(&["merge", "-q", "--no-ff", "pr", "-m", "merge"]);
        self.repo.git(&[
            "push",
            "-q",
            "-f",
            "origin",
            &format!("HEAD:refs/pull/{n}/merge"),
        ]);
        self.repo.git(&["checkout", "-q", "main"]);
        self.head.clone()
    }
}

// ---- The programs teer starts ------------------------------------------------------

/// gh, the browser opener, the editor and the agent CLIs as scripts in one
/// temp dir. gh records each call (`call_<k>`: its arguments one per line,
/// `stdin_<k>`) and answers `{"id":1}`, or fails once [`Fakes::gh_fails`];
/// `gh pr list` answers [`Fakes::pr_list`] without being recorded. The
/// agent records its arguments (`agent.args`) and directory (`agent.cwd`),
/// and its last one (`agent.prompt`), then runs what [`Fakes::agent`] gave
/// it.
pub struct Fakes {
    pub dir: tempfile::TempDir,
}

impl Fakes {
    pub fn new() -> Fakes {
        env();
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        script(
            &d.join("gh"),
            r#"d=$(dirname "$0")
case "$1 $2" in
  "pr list") cat "$d/pr_list.json" 2>/dev/null || echo '[]'; exit 0 ;;
esac
n=$(ls "$d" | grep -c '^call_')
n=$((n+1))
: > "$d/call_$n"
for a in "$@"; do printf '%s\n' "$a" >> "$d/call_$n"; done
if [ -p /dev/stdin ] || [ -f /dev/stdin ]; then cat > "$d/stdin_$n"; fi
if [ -f "$d/gh_fails" ]; then echo "HTTP 401: Bad credentials" >&2; exit 1; fi
echo '{"id":1}'"#,
        );
        script(
            &d.join("opener"),
            r#"printf '%s\n' "$@" >> "$(dirname "$0")/opened""#,
        );
        let f = Fakes { dir };
        f.agent("exit 0");
        f
    }

    /// The agent CLI (`claude` and `codex` alike): `body` runs in the
    /// directory teer starts it in, after its arguments are recorded.
    pub fn agent(&self, body: &str) {
        let d = self.dir.path();
        let rec = r#"d=$(dirname "$0")
: > "$d/agent.args"
for a in "$@"; do printf '%s\n' "$a" >> "$d/agent.args"; last=$a; done
printf '%s' "$last" > "$d/agent.prompt"
pwd -P > "$d/agent.cwd"
echo "$(basename "$0")" > "$d/agent.name""#;
        for name in ["claude", "codex"] {
            script(&d.join(name), &format!("{rec}\n{body}"));
        }
    }

    /// What `gh pr list` answers.
    pub fn pr_list(&self, json: &str) {
        std::fs::write(self.dir.path().join("pr_list.json"), json).unwrap();
    }

    pub fn gh_fails(&self) {
        std::fs::write(self.dir.path().join("gh_fails"), "").unwrap();
    }

    pub fn tools(&self) -> Tools {
        let d = self.dir.path();
        Tools {
            gh: d.join("gh").into(),
            opener: d.join("opener").into(),
            editor: d.join("no-editor").into(),
            claude: d.join("claude").into(),
            codex: d.join("codex").into(),
        }
    }

    /// The gh calls recorded (not `gh pr list`).
    pub fn gh_calls(&self) -> usize {
        std::fs::read_dir(self.dir.path())
            .unwrap()
            .filter(|e| {
                let name = e.as_ref().unwrap().file_name();
                name.to_string_lossy().starts_with("call_")
            })
            .count()
    }

    /// What gh call `k` (from 1) read on stdin, as JSON.
    pub fn gh_stdin(&self, k: usize) -> Value {
        let s = std::fs::read_to_string(self.dir.path().join(format!("stdin_{k}"))).unwrap();
        serde_json::from_str(&s).unwrap_or_else(|e| panic!("{e}: {s}"))
    }

    /// The agent's arguments, if it ran.
    pub fn agent_args(&self) -> Option<Vec<String>> {
        let s = std::fs::read_to_string(self.dir.path().join("agent.args")).ok()?;
        Some(s.lines().map(String::from).collect())
    }

    /// The agent's last argument (its first message), if it ran.
    pub fn agent_prompt(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.path().join("agent.prompt")).ok()
    }

    /// Where the agent ran, if it did.
    pub fn agent_cwd(&self) -> Option<PathBuf> {
        let s = std::fs::read_to_string(self.dir.path().join("agent.cwd")).ok()?;
        Some(PathBuf::from(s.trim()))
    }

    /// Which program ran as the agent (`claude`, `codex`), if one did.
    pub fn agent_name(&self) -> Option<String> {
        let s = std::fs::read_to_string(self.dir.path().join("agent.name")).ok()?;
        Some(s.trim().to_string())
    }
}

// ---- The model ---------------------------------------------------------------------

/// What the mock model recommends, answers, sums up and drafts.
pub const RECOMMEND: &str = "Check that the fee is charged once.";
pub const ANSWER: &str = "It adds the fee to the total, once per order.";
pub const SUMMARY: &str = "Charges a fee on every total.";
pub const DRAFT_SUBJECT: &str = "Charge the fee once";
pub const DRAFT_BODY: &str = "Drafted by the mock model.";

/// The mock model: describes every unit with one change statement citing
/// its first new line, at high risk (▲) for the units whose symbol is in
/// `high` and low risk otherwise; recommends [`RECOMMEND`]; answers every
/// question with [`ANSWER`], streaming a step and a partial answer first;
/// sums the change up; drafts [`DRAFT_SUBJECT`]. Anything else fails.
pub fn model(high: &[&str]) -> MockAdapter {
    let high: Vec<String> = high.iter().map(|s| s.to_string()).collect();
    let mut m = MockAdapter::new(move |prompt, schema| answer(&high, prompt, schema));
    m.live = vec![
        Live::Step("read calc.py".into()),
        Live::Partial("{\"text\": \"It adds the fee".into()),
    ];
    m
}

fn answer(high: &[String], prompt: &str, schema: &Value) -> anyhow::Result<Value> {
    let has = |p: &str| schema["properties"].get(p).is_some();
    if has("units") {
        return Ok(describe(high, prompt));
    }
    if has("summary") {
        return Ok(json!({ "summary": SUMMARY }));
    }
    if has("subject") {
        return Ok(json!({ "subject": DRAFT_SUBJECT, "body": DRAFT_BODY }));
    }
    if prompt.contains("DIVE:Recommend") {
        return Ok(json!({ "text": RECOMMEND, "sources": [] }));
    }
    if prompt.contains("\nASK") || prompt.starts_with("ASK") {
        return Ok(json!({ "text": ANSWER, "show": [], "related": [] }));
    }
    anyhow::bail!("the mock model has no answer for this prompt")
}

/// A prepare answer for every unit in `prompt`.
fn describe(high: &[String], prompt: &str) -> Value {
    let mut units = Vec::new();
    let mut id: Option<String> = None;
    for line in prompt.lines() {
        if let Some(rest) = line.strip_prefix("### Unit ") {
            id = Some(rest.trim().to_string());
        }
        if let (Some(uid), Some(r)) = (&id, line.strip_prefix("new lines: ")) {
            let symbol = uid.rsplit("::").next().unwrap_or(uid);
            let risk = if high.iter().any(|h| h == symbol) {
                "high"
            } else {
                "low"
            };
            let evidence = match r.split_once('-') {
                Some((a, _)) => {
                    json!([{"side": "new", "line": a.trim().parse::<u32>().unwrap_or(1)}])
                }
                None => json!([]),
            };
            units.push(json!({
                "id": uid,
                "summary": format!("{symbol} changes what it returns."),
                "changes": [{
                    "marker": "changed",
                    "statement": format!("{symbol} returns something else now."),
                    "risk": risk,
                    "evidence": evidence,
                }],
            }));
            id = None;
        }
    }
    json!({ "units": units })
}

// ---- Starting as `teer` does -----------------------------------------------------------

/// The screens, stood in for: no terminal for setup, and the review is
/// kept for the test instead of shown.
#[derive(Default)]
pub struct Captured {
    launched: RefCell<Option<Launch>>,
}

#[async_trait::async_trait(?Send)]
impl Screens for Captured {
    async fn setup(
        &self,
        _cfg: Config,
        _path: &Path,
        _opened: Opened,
        _keys: Keymap,
    ) -> anyhow::Result<Option<Ended>> {
        Ok(None)
    }

    async fn review(&self, launch: Launch) -> anyhow::Result<()> {
        *self.launched.borrow_mut() = Some(launch);
        Ok(())
    }
}

/// `teer <args>` in `dir` with the config at `config`, as far as the
/// review: what `cli::run_on` hands the screens, `None` when it ends
/// before one (nothing to review, the picker without a terminal).
pub async fn launch(dir: &Path, args: &[&str], config: &Path) -> anyhow::Result<Option<Launch>> {
    use clap::Parser;
    env();
    let args = teer::cli::Args::try_parse_from(
        ["teer", "-C", dir.to_str().unwrap()]
            .into_iter()
            .chain(args.iter().copied()),
    )?;
    let screens = Captured::default();
    teer::cli::run_on(args, config, &screens).await?;
    Ok(screens.launched.borrow_mut().take())
}

/// A config file in its own temp dir with `toml` in it (kept with the dir).
pub struct ConfigFile {
    dir: tempfile::TempDir,
}

impl ConfigFile {
    pub fn new(toml: &str) -> ConfigFile {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), toml).unwrap();
        ConfigFile { dir }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.path().join("config.toml")
    }

    pub fn load(&self) -> Config {
        Config::load(&self.path()).unwrap()
    }
}

// ---- The session ----------------------------------------------------------------------

/// A review running as `tui::run` runs it: see the module doc.
pub struct Session {
    pub app: App,
    pub rt: Runtime,
    rx: UnboundedReceiver<AppEvent>,
    /// The model, when the review has an agent.
    pub model: Option<MockAdapter>,
    /// An effect asked teer to quit; the review was saved then.
    pub quit: bool,
    /// Every effect the app returned, in order.
    pub effects: Vec<Effect>,
    /// What the runtime's background work sent, by kind, in order.
    pub events: Vec<&'static str>,
    /// What was put on the clipboard.
    pub copied: Vec<String>,
    /// Texts handed to the editor (`Effect::EditComment`).
    pub edited: Vec<String>,
}

/// The kind of a background event, for [`Session::events`].
fn tag(ev: &AppEvent) -> &'static str {
    match ev {
        AppEvent::Dive(_) => "Dive",
        AppEvent::Prep(_) => "Prep",
        AppEvent::Rerun(Ok(_)) => "Rerun",
        AppEvent::Rerun(Err(_)) => "RerunFailed",
        AppEvent::PrReviewed { .. } => "PrReviewed",
        AppEvent::CommitDrafted(_) => "CommitDrafted",
        AppEvent::Summary(_) => "Summary",
        AppEvent::Answer { .. } => "Answer",
        AppEvent::Asking { .. } => "Asking",
        AppEvent::Tree { .. } => "Tree",
        #[allow(unreachable_patterns)]
        _ => "Other",
    }
}

impl Session {
    /// The review `cli::run_on` launched, started as `tui::run` starts it,
    /// with `model` in place of the agent it found (none when it found none).
    pub fn launched(l: Launch, model: MockAdapter, tools: Tools) -> Session {
        let repo = l.review.repo_root.clone();
        let (agent, sessions, note) = match l.agent {
            Ok(a) => (true, a.sessions, None),
            Err(why) => (false, Vec::new(), Some(why)),
        };
        let pr = l.review.pr.is_some();
        let session = match l.session_arg {
            Some(id) => {
                let mut s = teer::cli::session_by_id(&id, &sessions, &repo);
                if pr {
                    s.cwd = repo.clone();
                }
                s
            }
            None => SessionRef::fresh(l.cfg.provider().name(), repo),
        };
        Session::begin(
            l.review,
            l.cfg,
            l.keymap,
            agent.then_some(model),
            note,
            sessions,
            session,
            tools,
        )
    }

    /// `review` under `cfg`, started as `tui::run` starts it: with `model`
    /// as the agent, in `session` (a fresh one in the repo when `None`), or
    /// as `--no-agent` without a model.
    pub fn start(
        review: Review,
        cfg: Config,
        model: Option<MockAdapter>,
        session: Option<SessionRef>,
        tools: Tools,
    ) -> Session {
        let keys = cfg.keymap().expect("valid keys").0;
        let session = session
            .unwrap_or_else(|| SessionRef::fresh(cfg.provider().name(), review.repo_root.clone()));
        let note = model.is_none().then(|| teer::cli::NO_AGENT.to_string());
        Session::begin(review, cfg, keys, model, note, Vec::new(), session, tools)
    }

    #[allow(clippy::too_many_arguments)]
    fn begin(
        review: Review,
        cfg: Config,
        keys: Keymap,
        model: Option<MockAdapter>,
        note: Option<String>,
        sessions: Vec<teer::agent::SessionInfo>,
        session: SessionRef,
        tools: Tools,
    ) -> Session {
        env();
        let base = (review.head != teer::git::WORKTREE).then(|| review.base.clone());
        let build = BuildOptions {
            base,
            languages: cfg.languages.clone(),
        };
        let mut app = App::new(review, cfg, keys);
        let initial = app.start(model.is_some(), note, sessions, Some(session));
        let (tx, rx) = unbounded_channel();
        let adapter = model.clone().map(|m| Arc::new(m) as Arc<dyn AgentAdapter>);
        let rt = Runtime {
            adapter,
            tx,
            build,
            tools,
        };
        let mut s = Session {
            app,
            rt,
            rx,
            model,
            quit: false,
            effects: Vec::new(),
            events: Vec::new(),
            copied: Vec::new(),
            edited: Vec::new(),
        };
        s.run(initial);
        s
    }

    /// Type `keys` (Neovim notation) and carry out what each key asks for.
    /// The effects of these keys.
    pub fn keys(&mut self, keys: &str) -> Vec<Effect> {
        let mut all = Vec::new();
        for k in parse_keys(keys).unwrap_or_else(|e| panic!("keys {keys:?}: {e}")) {
            if self.quit {
                break;
            }
            let effects = self.app.on_key(k.event());
            all.extend(effects.iter().cloned());
            self.run(effects);
        }
        all
    }

    /// Carry out `effects` as `tui::run` does.
    pub fn run(&mut self, effects: Vec<Effect>) {
        let was_quit = self.quit;
        for e in effects {
            self.effects.push(e.clone());
            match e {
                Effect::Quit => self.quit = true,
                // Need a terminal and a person: the tests answer for them.
                Effect::Edit { .. } | Effect::Settings => {}
                Effect::EditComment { text } => self.edited.push(text),
                Effect::Copy { text, what } => {
                    self.copied.push(text);
                    // As the runtime says it: copied to be fixed elsewhere,
                    // briefly, then that teer waits.
                    self.app.status = if self.app.waiting.is_some() {
                        format!("Copied to the clipboard. {}", self.app.waiting_line())
                    } else {
                        format!("Copied {what} to the clipboard.")
                    };
                }
                Effect::Commit {
                    repo,
                    paths,
                    message,
                } => {
                    let outcome = commit::run_commit(&repo, &paths, &message, Io::Capture);
                    let follow = self.app.commit_finished(outcome);
                    self.run(follow);
                }
                Effect::OpenAgent {
                    cli,
                    cwd,
                    prompt,
                    resume,
                } => {
                    let ran = run_agent(&self.rt.tools, cli, &cwd, &prompt, resume.as_deref());
                    // What `tui::run` does with the terminal back: the
                    // agent's exit is the signal, the review re-runs.
                    self.quit |= self.rt.agent_exited(&mut self.app, cli, ran);
                }
                e => self.quit |= self.rt.exec(&mut self.app, e),
            }
        }
        // `tui::run` saves the review once its loop ends.
        if self.quit && !was_quit {
            store::save(&self.app.review).unwrap();
        }
    }

    /// One tick of the run loop (every 100 ms): a key prefix past its time
    /// is dropped, and while teer waits for copied notes' changes it looks
    /// at the working tree when a look is due.
    pub fn tick(&mut self) {
        self.app.tick = self.app.tick.wrapping_add(1);
        self.app.keys.tick(Instant::now());
        self.rt.watch(&mut self.app, Instant::now());
    }

    fn feed(&mut self, ev: AppEvent) {
        self.events.push(tag(&ev));
        let effects = self.app.on_app_event(ev);
        self.run(effects);
    }

    /// Run the loop until `done` holds or `within` passed; whether it held.
    pub async fn until(&mut self, within: Duration, done: impl Fn(&Session) -> bool) -> bool {
        let end = Instant::now() + within;
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            if done(self) {
                return true;
            }
            if self.quit || Instant::now() >= end {
                return false;
            }
            tokio::select! {
                Some(ev) = self.rx.recv() => self.feed(ev),
                _ = tick.tick() => self.tick(),
            }
        }
    }

    /// Run the loop until nothing came for half a second (at most 30 s):
    /// the background work started so far is done.
    pub async fn settle(&mut self) {
        let end = Instant::now() + Duration::from_secs(30);
        let mut last = Instant::now();
        while !self.quit && Instant::now() < end && last.elapsed() < Duration::from_millis(500) {
            match tokio::time::timeout(Duration::from_millis(100), self.rx.recv()).await {
                Ok(Some(ev)) => {
                    self.feed(ev);
                    last = Instant::now();
                }
                _ => self.tick(),
            }
        }
    }

    /// Run the loop for `d`, whatever comes.
    pub async fn wait(&mut self, d: Duration) {
        self.until(d, |_| false).await;
    }

    /// Let the background work started so far finish (nothing came for
    /// half a second), and keep what it sent from the app: answers still
    /// on their way, handed over later with [`Session::deliver`].
    pub async fn hold(&mut self) -> Vec<AppEvent> {
        let mut held = Vec::new();
        while let Ok(Some(ev)) =
            tokio::time::timeout(Duration::from_millis(500), self.rx.recv()).await
        {
            held.push(ev);
        }
        held
    }

    /// Hand `events` to the app now, in order, as the run loop does.
    pub fn deliver(&mut self, events: Vec<AppEvent>) {
        for ev in events {
            self.feed(ev);
        }
    }

    /// How many times the runtime's re-run has come back.
    pub fn reruns(&self) -> usize {
        self.events.iter().filter(|e| **e == "Rerun").count()
    }

    // ---- What a test looks at --------------------------------------------------------

    pub fn render(&self, w: u16, h: u16) -> String {
        super::v2::screen(&self.app, w, h)
    }

    pub fn status(&self) -> &str {
        &self.app.status
    }

    /// The id of the unit with `symbol`.
    pub fn id(&self, symbol: &str) -> String {
        self.app
            .review
            .units
            .iter()
            .find(|u| u.symbol == symbol)
            .unwrap_or_else(|| panic!("no unit {symbol}"))
            .id
            .clone()
    }

    pub fn decision(&self, symbol: &str) -> Decision {
        let id = self.id(symbol);
        self.app.review.unit(&id).unwrap().decision
    }

    pub fn note(&self, symbol: &str) -> String {
        let id = self.id(symbol);
        self.app.review.unit(&id).unwrap().dive.note.clone()
    }

    /// The selected unit's symbol.
    pub fn selected(&self) -> Option<String> {
        let id = self.app.selected_id()?;
        self.app.review.unit(&id).map(|u| u.symbol.clone())
    }

    /// Put the cursor on the unit with `symbol`, by moving it down the
    /// list with `j` (so the move is saved as any other).
    pub fn go_to(&mut self, symbol: &str) {
        let id = self.id(symbol);
        let pos = self
            .app
            .order
            .iter()
            .position(|&i| self.app.review.units[i].id == id)
            .unwrap_or_else(|| panic!("{symbol} is listed"));
        self.keys("gg");
        for _ in 0..pos {
            self.keys("j");
        }
        assert_eq!(self.selected().as_deref(), Some(symbol));
    }

    /// Symbols in list order.
    pub fn listed(&self) -> Vec<String> {
        self.app
            .order
            .iter()
            .map(|&i| self.app.review.units[i].symbol.clone())
            .collect()
    }

    /// (accepted, flagged, pending).
    pub fn counts(&self) -> (usize, usize, usize) {
        self.app.decision_counts()
    }

    /// The key bound to `action`, as the screen names it.
    pub fn key(&self, action: Action) -> String {
        self.app.keys.label(action)
    }

    /// Every effect so far that `f` picks.
    pub fn seen<T>(&self, f: impl Fn(&Effect) -> Option<T>) -> Vec<T> {
        self.effects.iter().filter_map(f).collect()
    }
}

/// Run the agent CLI as `tui::run_agent` does, without the terminal (and
/// with stdin closed): the program for `cli`, `--resume <id>
/// --fork-session` when there is a session, then the prompt, in `cwd`,
/// until it exits. How it went, for `Runtime::agent_exited`.
pub fn run_agent(
    tools: &Tools,
    cli: Provider,
    cwd: &Path,
    prompt: &str,
    resume: Option<&str>,
) -> std::io::Result<std::process::ExitStatus> {
    let program = match cli {
        Provider::Codex => &tools.codex,
        _ => &tools.claude,
    };
    let mut cmd = std::process::Command::new(program);
    if let Some(id) = resume {
        cmd.args(["--resume", id, "--fork-session"]);
    }
    cmd.arg(prompt)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .status()
}

// ---- What teer v1 saved ----------------------------------------------------------------

/// `calc.py` before and after the change teer v1 (`e394565`) reviewed and
/// saved as `tests/fixtures/v1/review.json` (`teer --json --no-agent`),
/// with a v1 user's decisions put in: `total` flagged, resolved "needs
/// fix", with a note; `fee` flagged, resolved "ok"; `tax` flagged;
/// `discount` accepted; `rounding` pending.
pub const V1_CALC_OLD: &str = "def total(a, b):\n    return a + b\n\n\ndef fee(x):\n    return x + 1\n\n\ndef tax(x):\n    return x * 0.2\n\n\ndef discount(x):\n    return x - 1\n\n\ndef rounding(x):\n    return round(x)\n";
pub const V1_CALC_NEW: &str = "def total(a, b):\n    return a + b + fee(a)\n\n\ndef fee(x):\n    return x + 2\n\n\ndef tax(x):\n    return x * 0.25\n\n\ndef discount(x):\n    return x - 2\n\n\ndef rounding(x):\n    return round(x, 2)\n";

/// A file of `tests/fixtures/v1`: `review.json`, `config.toml`.
pub fn v1_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/v1")
        .join(name)
}

/// A repo with the change v1 reviewed: `calc.py` changed, not committed.
pub fn v1_repo() -> FixtureRepo {
    let r = repo();
    r.write("calc.py", V1_CALC_OLD).commit("init");
    r.write("calc.py", V1_CALC_NEW);
    r
}

/// Put v1's saved review of `repo`'s change where teer looks for it (the
/// fixture with this repo's path and commits). What v2 builds for the
/// change.
pub fn save_v1_review(repo: &FixtureRepo) -> Review {
    let built = teer::review::build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let text = std::fs::read_to_string(v1_fixture("review.json")).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    // The fixture's units are this change's: same ids, same code.
    let fingerprints = |units: &Value| -> Vec<(String, String)> {
        units
            .as_array()
            .unwrap()
            .iter()
            .map(|u| {
                (
                    u["id"].as_str().unwrap().to_string(),
                    u["fingerprint"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    let now = serde_json::to_value(&built.units).unwrap();
    assert_eq!(
        fingerprints(&v["units"]),
        fingerprints(&now),
        "v2 knows the v1 units by the same ids and fingerprints"
    );
    v["repo_root"] = json!(built.repo_root);
    v["base"] = json!(built.base);
    v["head_commit"] = json!(built.head_commit);
    let path = store::review_path(
        &built.repo_root,
        None,
        &built.base,
        &built.head,
        &built.head_commit,
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    built
}
