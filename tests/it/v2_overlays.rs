//! v2 Phase 1: overlays, prompts, sheets and text inputs read their keys
//! through the keymap's modes. Text inputs take every letter as text, y/n
//! prompts say yes to `y` and ⏎ only, sheets have their own keys, and the
//! panels (notes, the jump list) move with the normal keys. Keys are typed
//! in Neovim notation and go through `App::on_key`. No agent, no network;
//! the commit test uses a temp git repository and stops at the effect.

use std::path::{Path, PathBuf};
use std::process::Command;

use teer::approve::{EditOutcome, Tools, Verdict};
use teer::config::Config;
use teer::git::{ChangedFile, FileStatus};
use teer::keymap::{Keymap, parse_keys};
use teer::review::model::{
    Decision, Dive, PrInfo, Prep, Range, Review, Unit, UnitKind, UnitStatus,
};
use teer::review::{BuildOptions, build_review};
use teer::tui::Runtime;
use teer::tui::app::{App, DraftState, Effect, JumpKind, Overlay, Screen};
use teer::tui::finish::Step;

const OLD: &str = "def f():\n    return 1\n\n\ndef g():\n    return 2\n";
const NEW: &str = "def f():\n    return 10\n\n\ndef g():\n    return 20\n";
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

// ------------------------------------------------------------- fixtures

/// Point teer's data and config at the test process's temp dir.
fn isolate_env() {
    crate::common::isolate_env();
}

fn unit(symbol: &str, old: (u32, u32), new: (u32, u32), note: &str) -> Unit {
    Unit {
        id: format!("a.py::{symbol}"),
        file: PathBuf::from("a.py"),
        symbol: symbol.into(),
        kind: UnitKind::Function,
        status: UnitStatus::Changed,
        old_range: Some(Range::new(old.0, old.1)),
        new_range: Some(Range::new(new.0, new.1)),
        renamed_from: None,
        fingerprint: format!("fp-{symbol}"),
        changes: Vec::new(),
        prep: Prep::Skipped,
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Pending,
        summary: None,
        nbc_reason: None,
        dive: Dive {
            note: note.into(),
            ..Default::default()
        },
        accepted_by: None,
        reasons: Vec::new(),
        dive_pending: Default::default(),
        checks: Default::default(),
    }
}

fn pr_info() -> PrInfo {
    PrInfo {
        host: "github.com".into(),
        owner: "acme".into(),
        repo: "shop".into(),
        number: 7,
        remote: Some("origin".into()),
        head_sha: Some(SHA.into()),
        repo_dir: Some(PathBuf::from("/r")),
        ..Default::default()
    }
}

/// Two units `f` and `g` in `a.py`, with these notes; a pull request
/// review when `pr` is given.
fn app(notes: [&str; 2], pr: Option<PrInfo>) -> App {
    isolate_env();
    let review = Review {
        id: "t".into(),
        repo_root: PathBuf::from("/repo"),
        base: "HEAD".into(),
        head: teer::git::WORKTREE.into(),
        head_commit: SHA.into(),
        agent_session: None,
        pr,
        created_at: chrono::Utc::now(),
        files: vec![ChangedFile {
            path: PathBuf::from("a.py"),
            old_path: None,
            status: FileStatus::Modified,
            old_content: Some(OLD.into()),
            new_content: Some(NEW.into()),
        }],
        units: vec![
            unit("f", (1, 2), (1, 2), notes[0]),
            unit("g", (5, 6), (5, 6), notes[1]),
        ],
        skipped: Vec::new(),
        graph: Default::default(),
        intent: Default::default(),
        questions: Vec::new(),
        version: 0,
        history: Default::default(),
        branch: None,
        cursor: None,
    };
    App::new(review, Config::default(), Keymap::default())
}

/// Type `keys` (Neovim notation) and collect the effects.
fn press(a: &mut App, keys: &str) -> Vec<Effect> {
    let mut out = Vec::new();
    for k in parse_keys(keys).unwrap() {
        out.extend(a.on_key(k.event()));
    }
    out
}

fn note<'a>(a: &'a App, symbol: &str) -> &'a str {
    &a.review.unit(&format!("a.py::{symbol}")).unwrap().dive.note
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=teer",
            "-c",
            "user.email=teer@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository with one commit and `a.py` changed on top, reviewed and
/// every unit accepted, started like the real thing, and the `:w` sheet
/// open on its commit.
fn done_app(dir: &Path) -> App {
    isolate_env();
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.py"), OLD).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "initial"]);
    std::fs::write(dir.join("a.py"), NEW).unwrap();
    let mut rv = build_review(dir, &BuildOptions::default()).unwrap();
    for u in &mut rv.units {
        u.decision = Decision::Accepted;
    }
    let mut a = App::new(rv, Config::default(), Keymap::default());
    a.start(false, None, Vec::new(), None);
    press(&mut a, ":w<cr>");
    assert_eq!(a.finish_step(), Some(Step::Commit));
    a
}

// ------------------------------------------------------------- note line

#[test]
fn the_note_line_takes_every_letter_and_saves_on_enter() {
    let mut a = app(["", ""], None);
    let id = a.selected_id().unwrap();
    press(&mut a, "i");
    assert!(matches!(&a.overlay, Overlay::Note { id: i, back: None, .. } if *i == id));
    // `j`, `q`, `:` and `a` are actions in normal mode; here they are text.
    press(&mut a, "jq: a<bs>b");
    assert!(matches!(&a.overlay, Overlay::Note { text, .. } if text == "jq: b"));
    assert_eq!(a.screen, Screen::Queue);
    let e = press(&mut a, "<cr>");
    assert_eq!(e, vec![Effect::Save]);
    assert_eq!(a.overlay, Overlay::None);
    let symbol = a.review.unit(&id).unwrap().symbol.clone();
    assert_eq!(note(&a, &symbol), "jq: b");
    assert!(
        a.review
            .units
            .iter()
            .all(|u| u.decision == Decision::Pending)
    );
}

#[test]
fn esc_cancels_the_note_line() {
    let mut a = app(["kept", "kept"], None);
    press(&mut a, "i");
    press(&mut a, "<c-u>gone");
    assert!(matches!(&a.overlay, Overlay::Note { text, .. } if text == "gone"));
    let e = press(&mut a, "<esc>");
    assert!(e.is_empty());
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(note(&a, "f"), "kept");
    assert_eq!(note(&a, "g"), "kept");
}

// ------------------------------------------------------------- y/n prompt

#[test]
fn the_reset_prompt_says_no_to_n_and_yes_to_y() {
    let mut a = app(["", ""], None);
    a.review.units[0].decision = Decision::Accepted;
    press(&mut a, ":e!<cr>");
    assert_eq!(a.overlay, Overlay::ConfirmReset);
    // `n` (move.next_pending elsewhere) is no.
    let e = press(&mut a, "n");
    assert!(e.is_empty());
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(a.review.units[0].decision, Decision::Accepted);
    press(&mut a, ":e!<cr>");
    assert_eq!(a.overlay, Overlay::ConfirmReset);
    // `y` (finish.yank elsewhere) is yes: reset, then re-run (Phase 7).
    let e = press(&mut a, "y");
    assert_eq!(e, vec![Effect::Save, Effect::Rerun]);
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(a.review.units[0].decision, Decision::Pending);
}

// ------------------------------------------------------------- notes view

#[test]
fn the_notes_view_deletes_after_asking() {
    let mut a = app(["first", "second"], None);
    press(&mut a, "<space>n");
    assert!(matches!(a.overlay, Overlay::Notes { sel: 0 }));
    press(&mut a, "j");
    assert_eq!(a.overlay, Overlay::Notes { sel: 1 });
    press(&mut a, "d");
    assert!(matches!(
        &a.overlay,
        Overlay::ConfirmDeleteNote { sel: 1, .. }
    ));
    // No: nothing deleted, back in the notes.
    press(&mut a, "<esc>");
    assert_eq!(a.overlay, Overlay::Notes { sel: 1 });
    assert_eq!(note(&a, "g"), "second");
    press(&mut a, "d");
    let e = press(&mut a, "y");
    assert_eq!(e, vec![Effect::Save]);
    assert_eq!(note(&a, "g"), "");
    assert_eq!(note(&a, "f"), "first");
    assert!(matches!(a.overlay, Overlay::Notes { .. }));
}

#[test]
fn y_in_the_notes_view_copies_them() {
    let mut a = app(["first", "second"], None);
    press(&mut a, "<space>n");
    let e = press(&mut a, "y");
    let [Effect::Copy { text, what }] = &e[..] else {
        panic!("no copy: {e:?}")
    };
    assert!(text.contains("first") && text.contains("second"), "{text}");
    assert_eq!(what, "2 notes as Markdown");
    assert!(matches!(a.overlay, Overlay::Notes { .. }));
}

#[test]
fn i_in_the_notes_view_edits_and_comes_back() {
    let mut a = app(["first", "second"], None);
    press(&mut a, "<space>nG");
    assert_eq!(a.overlay, Overlay::Notes { sel: 1 });
    press(&mut a, "i");
    assert!(matches!(&a.overlay, Overlay::Note { back: Some(1), text, .. } if text == "second"));
    press(&mut a, "!<cr>");
    assert_eq!(note(&a, "g"), "second!");
    assert_eq!(a.overlay, Overlay::Notes { sel: 1 });
    // gg, then esc closes.
    press(&mut a, "gg");
    assert_eq!(a.overlay, Overlay::Notes { sel: 0 });
    press(&mut a, "<esc>");
    assert_eq!(a.overlay, Overlay::None);
}

// ------------------------------------------------------------- commit sheet

#[test]
fn the_commit_sheet_edits_commits_and_closes() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let mut a = done_app(&dir);
    // No agent: the template message.
    let message = a.finish.message.clone();
    assert_eq!(a.finish.draft, Some(DraftState::Template));
    // `i` edits the message in the editor (v1 `e`).
    let e = press(&mut a, "i");
    assert_eq!(
        e,
        vec![Effect::EditComment {
            text: message.clone()
        }]
    );
    assert_eq!(a.finish.draft, Some(DraftState::Edited));
    // No editor: the subject is edited on one line, where letters are text.
    a.comment_edited(EditOutcome::Unavailable("no editor".into()));
    press(&mut a, "<c-u>Fix jq");
    assert_eq!(a.finish.editing.as_deref(), Some("Fix jq"));
    press(&mut a, "<cr>");
    assert_eq!(a.finish.editing, None);
    assert!(
        a.finish.message.starts_with("Fix jq"),
        "{}",
        a.finish.message
    );
    assert!(matches!(a.overlay, Overlay::Finish { .. }));
    // `y` copies the notes (none here), never commits; ⏎ does.
    assert!(press(&mut a, "y").is_empty());
    let e = press(&mut a, "<cr>");
    let [Effect::Commit { paths, message, .. }] = &e[..] else {
        panic!("no commit: {e:?}; status {:?}", a.status)
    };
    assert_eq!(paths, &[PathBuf::from("a.py")]);
    assert!(message.starts_with("Fix jq"), "{message}");
    // esc closes.
    press(&mut a, "<esc>");
    assert_eq!(a.overlay, Overlay::None);
}

// ------------------------------------------------------------- the editor

/// An editor in `dir`: a script that writes how it was started, after its
/// `name`, to `log` there.
fn editor_script(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    let log = dir.join("log");
    let body = format!(
        "#!/bin/sh\nprintf '{name} %s\\n' \"$*\" >> '{}'\n",
        log.display()
    );
    std::fs::write(&p, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

/// An editor saved with `<space>s` opens at once, not after a restart:
/// `i` on the commit sheet hands it the message, `gf` the file at its
/// line, each as `tui::run` does.
#[test]
fn an_editor_saved_in_the_settings_opens_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let mut a = done_app(&dir);
    let bin = tempfile::tempdir().unwrap();
    let old = editor_script(bin.path(), "old");
    let new = editor_script(bin.path(), "new");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rt = Runtime {
        adapter: None,
        tx,
        build: BuildOptions::default(),
        tools: Tools {
            editor: old.into(),
            ..Default::default()
        },
    };
    let mut saved = a.cfg.clone();
    saved.editor = format!("'{}' +{{line}} {{file}}", new.display());
    assert_eq!(rt.settings_saved(&mut a, saved), "Settings saved.");

    let e = press(&mut a, "i");
    let [Effect::EditComment { text }] = &e[..] else {
        panic!("{e:?}")
    };
    let edited = teer::approve::edit_message_in_editor(&rt.tools.editor, text);
    assert!(matches!(edited, EditOutcome::Edited(_)), "{edited:?}");
    a.comment_edited(edited);
    press(&mut a, "<esc>");
    let e = press(&mut a, "gf");
    let [Effect::Edit { file, line }] = &e[..] else {
        panic!("{e:?}")
    };
    let opened = rt
        .tools
        .editor
        .command(&dir.join(file), *line)
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(opened.success());

    let log = std::fs::read_to_string(bin.path().join("log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert!(lines[0].starts_with("new +1 "), "{log}");
    assert!(lines[0].ends_with("COMMIT_EDITMSG"), "{log}");
    assert_eq!(
        lines[1],
        format!("new +{line} {}", dir.join("a.py").display())
    );
}

/// A saved editor that doesn't parse leaves the one there was, and says so.
#[test]
fn an_editor_that_does_not_parse_keeps_the_one_there_was() {
    let mut a = app(["", ""], None);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rt = Runtime {
        adapter: None,
        tx,
        build: BuildOptions::default(),
        tools: Tools::default(),
    };
    let before = rt.tools.editor.clone();
    let mut saved = a.cfg.clone();
    saved.editor = "'code".into();
    let status = rt.settings_saved(&mut a, saved);
    assert!(
        status.starts_with("Settings saved. The editor stays nvim:"),
        "{status}"
    );
    assert_eq!(rt.tools.editor, before);
}

// ------------------------------------------------------------- post sheet

#[test]
fn the_post_sheet_picks_the_verdict_with_j_and_k() {
    let mut a = app(["first", ""], Some(pr_info()));
    a.review.units[0].decision = Decision::Investigate;
    let verdict = |a: &App| a.finish_step().and_then(|s| s.verdict());
    // `:w` opens on keeping on reviewing: a unit is still pending.
    press(&mut a, ":w<cr>");
    assert_eq!(a.finish_step(), Some(Step::KeepReviewing));
    press(&mut a, "j");
    assert_eq!(verdict(&a), Some(Verdict::RequestChanges));
    press(&mut a, "j");
    assert_eq!(verdict(&a), Some(Verdict::Comment));
    press(&mut a, "k");
    assert_eq!(verdict(&a), Some(Verdict::RequestChanges));
    // `i` edits the summary in the editor.
    let e = press(&mut a, "i");
    assert!(matches!(&e[..], [Effect::EditComment { .. }]), "{e:?}");
    press(&mut a, "<esc>");
    assert_eq!(a.overlay, Overlay::None);
}

// ------------------------------------------------------------- Ask

#[test]
fn letters_typed_in_the_ask_panel_go_into_the_question() {
    use teer::tui::unit::Panel;
    // From the queue, `K` asks in the unit view's panel.
    let mut a = app(["", ""], None);
    assert_eq!(a.screen, Screen::Queue);
    press(&mut a, "K");
    assert_eq!((a.screen, a.unit.panel), (Screen::Unit, Some(Panel::Ask)));
    press(&mut a, "jkq ?");
    assert_eq!(a.ask.draft, "jkq ?");
    assert_eq!((a.screen, a.unit.panel), (Screen::Unit, Some(Panel::Ask)));
    press(&mut a, "<bs><esc>");
    assert_eq!(a.ask.draft, "jkq ");
    assert_eq!((a.screen, a.unit.panel), (Screen::Unit, Some(Panel::Ask)));
    assert!(!a.unit.typing);
    press(&mut a, "<esc>");
    assert_eq!((a.screen, a.unit.panel), (Screen::Unit, None));
    // In the unit view, `K` opens it there and keeps the draft.
    press(&mut a, "K?");
    assert_eq!(a.ask.draft, "jkq ?");
    assert_eq!(a.overlay, Overlay::None, "no Ask view over the screen");
}

// ------------------------------------------------------------- jump list

#[test]
fn the_find_list_takes_a_query_and_enter_selects_the_unit() {
    let mut a = app(["", ""], None);
    a.sel = 0;
    let first = a.selected_id().unwrap();
    // `/` opens it once the normal keys route find.unit here.
    a.open_jump(JumpKind::Find);
    assert!(matches!(a.overlay, Overlay::Jump { .. }));
    assert_eq!(a.jump_items().len(), 2);
    let other = if first == "a.py::f" { "g" } else { "f" };
    press(&mut a, &format!("a.py {other}"));
    let items = a.jump_items();
    assert_eq!(a.review.units[items[0]].symbol, other);
    press(&mut a, "<cr>");
    assert_eq!(a.overlay, Overlay::None);
    assert_eq!(a.selected_id().unwrap(), format!("a.py::{other}"));
}
