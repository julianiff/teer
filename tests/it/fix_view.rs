//! Proposed changes: fix_view (see FEATURES.md, proposed changes).
//!
//! The "Proposed change" section of the unit view's text: its heading,
//! summary, the second model's word and each hunk boxed like the code
//! change, `]c` `[c` stopping at each after the code change's hunks, the
//! current hunk `p` puts in (after a y/n prompt; on a pull request as a
//! suggestion), `P` every open one, `<space>e` asking for a change, the
//! queue's preview line, and the agent asked on its own once a high
//! security finding stands.

use crate::common;

use std::path::PathBuf;

use common::v2::{Driver, review, unit};
use teer::config::ProposeFixes;
use teer::fix::{HunkState, Proposal, ProposedHunk, Reason};
use teer::git::{ChangedFile, FileStatus};
use teer::review::dive::DiveEvent;
use teer::review::model::{
    Cite, Decision, DiveKind, DiveResult, PrInfo, Range, Risk, SessionRef, Unit,
};
use teer::security::{Audit, Class, Finding, SecondCheck};
use teer::session::app::{AppEvent, Effect, Overlay, Screen};
use teer::session::unit::Panel;
use teer::session::view::{OverlayView, snapshot};

/// Twenty lines `l1`..`l20`, changed at lines 2 and 17: a code change of
/// two hunks.
fn file(path: &str) -> ChangedFile {
    let old: Vec<String> = (1..=20).map(|i| format!("l{i}")).collect();
    let mut new = old.clone();
    new[1] = "L2".into();
    new[16] = "L17".into();
    ChangedFile {
        path: PathBuf::from(path),
        old_path: None,
        status: FileStatus::Modified,
        old_content: Some(old.join("\n")),
        new_content: Some(new.join("\n")),
    }
}

/// Unit `u{i}` over all twenty lines of `path`.
fn changed(i: usize, path: &str) -> Unit {
    let mut u = unit(i, path);
    u.old_range = Some(Range::new(1, 20));
    u.new_range = Some(Range::new(1, 20));
    u
}

/// Two units with a code change each (`u0` in `a.rs`, `u1` in `b.rs`),
/// and an agent.
fn driver() -> Driver {
    let mut r = review(vec![changed(0, "a.rs"), changed(1, "b.rs")]);
    r.files = vec![file("a.rs"), file("b.rs")];
    let mut d = Driver::with(r, teer::keymap::Keymap::default());
    agent(&mut d);
    d
}

fn agent(d: &mut Driver) {
    d.app.has_agent = true;
    d.app.review.agent_session = Some(SessionRef::default());
}

/// A hunk replacing line `line` of `file` (`l{line}`) with `new`, between
/// a line of context on each side.
fn hunk(file: &str, line: u32, new: &str, why: &str) -> ProposedHunk {
    ProposedHunk {
        file: PathBuf::from(file),
        lines: Range::new(line - 1, line + 1),
        old: format!("l{}\nl{line}\nl{}", line - 1, line + 1),
        new: format!("l{}\n{new}\nl{}", line - 1, line + 1),
        why: why.into(),
        parses: true,
        state: HunkState::Proposed,
    }
}

const TITLE: &str = "the id reaches the query unbound";

/// A proposal answering a high finding, its hunks `hunks`.
fn proposal(hunks: Vec<ProposedHunk>) -> Proposal {
    Proposal {
        reason: Reason::Finding {
            index: 0,
            title: TITLE.into(),
            severity: Risk::High,
        },
        summary: "Casts the id before the query; nothing else changes.".into(),
        hunks,
        second: Some(SecondCheck {
            agree: true,
            why: "it fixes the finding".into(),
            evidence: None,
        }),
        dropped: 0,
    }
}

/// Three hunks: two in `a.rs` (lines 5 and 12), one in `b.rs` (line 9).
fn three() -> Proposal {
    proposal(vec![
        hunk("a.rs", 5, "L5 = int(l5)", "cast before it is used"),
        hunk("a.rs", 12, "L12 checked", "check the owner"),
        hunk("b.rs", 9, "L9 logged", "log the refusal"),
    ])
}

/// Unit `id` with proposal `p`.
fn propose(d: &mut Driver, id: &str, p: Proposal) {
    let u = d.app.review.unit_mut(id).unwrap();
    u.dive.results.insert(
        DiveKind::Fix,
        DiveResult {
            text: p.summary.clone(),
            proposal: Some(p),
            ..Default::default()
        },
    );
}

fn states(d: &Driver, id: &str) -> Vec<HunkState> {
    let u = d.app.review.unit(id).unwrap();
    u.dive.results[&DiveKind::Fix]
        .proposal
        .as_ref()
        .unwrap()
        .hunks
        .iter()
        .map(|h| h.state)
        .collect()
}

/// The unit view on `u0`, the text alone (`zz`), drawn `h` rows high.
fn open(d: &mut Driver, h: u16) -> String {
    d.keys("l");
    assert_eq!(d.screen(), Screen::Unit);
    d.keys("zz");
    d.render(100, h)
}

fn row_of(screen: &str, text: &str) -> usize {
    row_after(screen, text, 0)
}

/// The colour of the left edge of the box whose top row shows `title`,
/// drawn `w` x `h`.
fn edge_colour(d: &Driver, w: u16, h: u16, title: &str) -> Option<ratatui::style::Color> {
    use ratatui::{Terminal, backend::TestBackend};
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| teer::tui::ui::draw(f, &d.app)).unwrap();
    let buf = t.backend().buffer();
    let y = (0..h)
        .find(|&y| {
            let row: String = (0..w).map(|x| buf[(x, y)].symbol()).collect();
            row.contains(title)
        })
        .unwrap_or_else(|| panic!("{title} isn't drawn"));
    buf[(1, y)].style().fg
}

/// The first row from `from` on where `text` is drawn.
fn row_after(screen: &str, text: &str, from: usize) -> usize {
    screen
        .lines()
        .skip(from)
        .position(|l| l.contains(text))
        .map(|r| r + from)
        .unwrap_or_else(|| panic!("{text:?} isn't drawn after row {from}:\n{screen}"))
}

#[test]
fn the_section_shows_what_it_answers_its_summary_and_each_hunk_in_a_box() {
    let mut d = driver();
    let mut p = three();
    p.dropped = 1;
    p.hunks[2].parses = false;
    p.hunks[2].state = HunkState::Applied;
    propose(&mut d, "u0", p);
    let text = open(&mut d, 120);
    // After the code change.
    let code = row_of(&text, "╭ code change");
    let head = row_of(&text, "Proposed change");
    assert!(head > code, "{text}");
    let heading = text.lines().nth(head).unwrap();
    assert!(
        heading.contains(&format!("for ▲ {TITLE} (high)")),
        "{heading}"
    );
    // Who wrote it: after it, or under it where it doesn't fit.
    let by = row_of(&text, "agent (");
    assert!(by == head || by == head + 1, "{text}");
    for s in [
        "Casts the id before the query; nothing else changes.",
        "✓ check agrees",
        "1 hunk dropped",
    ] {
        assert!(row_of(&text, s) > head, "{s}:\n{text}");
    }
    // Each hunk in a box: its place, its state, why, its lines.
    let first = row_of(&text, "╭ a.rs:4-6 ");
    assert!(
        text.lines().nth(first).unwrap().contains("proposed ╮"),
        "{text}"
    );
    assert!(
        row_of(&text, "cast before it is used") == first + 1,
        "{text}"
    );
    for s in ["  4   l4", "  5 - l5", "  5 + L5 = int(l5)", "  6   l6"] {
        let r = row_after(&text, s, first);
        assert!(r > first && r < first + 7, "{s}:\n{text}");
        assert!(text.lines().nth(r).unwrap().starts_with(" │ "), "{text}");
    }
    let last = row_of(&text, "╭ b.rs:8-10 ");
    assert!(
        text.lines().nth(last).unwrap().contains("put in ╮"),
        "{text}"
    );
    assert_eq!(row_of(&text, "⚠ doesn't parse"), last + 2, "{text}");
    // The current one (the first: nearest the top) says what puts it in.
    let keys = row_of(&text, "p put · P put all");
    assert!(
        keys > first && keys < row_of(&text, "╭ a.rs:11-13"),
        "{text}"
    );
    assert_eq!(text.matches("p put").count(), 1, "{text}");
    // Its border in the accent, the others' as the code change's.
    let accent = d.app.theme.accent().fg;
    assert_eq!(edge_colour(&d, 100, 120, "╭ a.rs:4-6 "), accent);
    let plain = d.app.theme.text().fg;
    assert_ne!(plain, accent);
    assert_eq!(edge_colour(&d, 100, 120, "╭ a.rs:11-13 "), plain);
    assert_eq!(edge_colour(&d, 100, 120, "╭ code change"), plain);

    // Disputed: the second model's why, as a warning.
    let mut p = three();
    p.second = Some(SecondCheck {
        agree: false,
        why: "it also drops the log line".into(),
        evidence: None,
    });
    propose(&mut d, "u0", p);
    let text = d.render(100, 120);
    assert!(
        text.contains("⚠ disputed: it also drops the log line"),
        "{text}"
    );
    assert!(!text.contains("check agrees"), "{text}");
}

#[test]
fn while_a_change_is_on_its_way_the_heading_spins_and_says_what_it_does() {
    let mut d = driver();
    let text = open(&mut d, 80);
    assert!(!text.contains("Proposed change"), "nothing asked:\n{text}");
    d.app.review.unit_mut("u0").unwrap().dive.note = "check the owner".into();
    d.app
        .review
        .unit_mut("u0")
        .unwrap()
        .dive_pending
        .insert(DiveKind::Fix);
    let text = d.render(100, 80);
    let head = row_of(&text, "Proposed change");
    let heading = text.lines().nth(head).unwrap();
    assert!(heading.contains("for your note"), "{heading}");
    let spinner = heading
        .split("Proposed change")
        .nth(1)
        .and_then(|r| r.trim_start().chars().next())
        .unwrap();
    assert!(('⠀'..='⣿').contains(&spinner), "{heading}");
    assert!(
        text.lines()
            .nth(head + 1)
            .unwrap()
            .contains("asking the agent"),
        "{text}"
    );
    d.app.progress.insert(
        ("u0".into(), DiveKind::Fix),
        "checking the hunks parse".into(),
    );
    let text = d.render(100, 80);
    assert!(text.contains("checking the hunks parse"), "{text}");
}

#[test]
fn hunk_jumps_reach_the_proposed_hunks_after_the_code_changes() {
    let mut d = driver();
    propose(&mut d, "u0", three());
    open(&mut d, 20);
    let code = d.app.unit.hunks.borrow().clone();
    let fixes = d.app.unit.fixes.borrow().clone();
    assert_eq!((code.len(), fixes.len()), (2, 3));
    assert!(fixes[0] > code[1], "{code:?} {fixes:?}");
    let max = d.app.unit.text_max.get();
    d.keys("]c]c");
    assert_eq!(d.app.detail_offset(), code[1].min(max));
    assert_eq!(d.app.unit.fix, None);
    // On to the proposed ones: each is the current one in turn, even where
    // the text can't scroll it to the top.
    for (i, &row) in fixes.iter().enumerate() {
        d.keys("]c");
        assert_eq!(d.app.unit.fix, Some(i), "{}", d.status());
        assert_eq!(d.app.current_fix(), Some(i));
        assert_eq!(d.app.detail_offset(), row.min(max));
        let text = d.render(100, 20);
        if row <= max {
            // At the top (under the title row and the line that says
            // what is above).
            let top = text.lines().nth(4).unwrap();
            assert!(top.starts_with(" ╭ "), "{text}");
            assert!(top.contains("proposed"), "{text}");
        }
    }
    d.keys("]c");
    assert_eq!(d.status(), "No more hunks below.");
    assert_eq!(d.app.unit.fix, Some(2));
    d.keys("[c");
    assert_eq!(d.app.unit.fix, Some(1));
    d.keys("2[c");
    assert_eq!(d.app.unit.fix, None);
    assert_eq!(d.app.detail_offset(), code[1].min(max));
    // A scroll goes by what is at the top again.
    d.keys("]c");
    assert_eq!(d.app.unit.fix, Some(0));
    d.keys("gg");
    assert_eq!(d.app.unit.fix, None);
    assert_eq!(d.app.current_fix(), Some(0));
}

#[test]
fn with_the_whole_text_on_screen_hunk_jumps_still_pick_the_proposed_hunk() {
    let mut d = driver();
    propose(&mut d, "u0", three());
    open(&mut d, 120);
    assert_eq!(d.app.unit.text_max.get(), 0);
    // The code change's hunks can't scroll: passed over.
    d.keys("]c");
    assert_eq!(d.app.unit.fix, Some(0));
    d.keys("2]c");
    assert_eq!(d.app.unit.fix, Some(2));
    let text = d.render(100, 120);
    let keys = text.lines().position(|l| l.contains("p put")).unwrap();
    assert!(keys > row_of(&text, "╭ b.rs:8-10"), "{text}");
    d.keys("]c");
    assert_eq!(d.status(), "No more hunks below.");
    // Back past the first: none is the current one by a jump.
    d.keys("3[c");
    assert_eq!((d.app.unit.fix, d.app.detail_offset()), (None, 0));
    d.keys("[c");
    assert_eq!(d.status(), "No more hunks above.");
}

#[test]
fn p_asks_then_puts_the_current_hunk_in_and_shift_p_every_open_one() {
    let mut d = driver();
    let mut p = three();
    p.hunks[0].state = HunkState::Applied;
    propose(&mut d, "u0", p);
    open(&mut d, 30);
    // On the second hunk.
    d.keys("]c]c]c]c");
    assert_eq!(d.app.current_fix(), Some(1));
    assert!(d.keys("p").is_empty());
    assert_eq!(
        d.overlay(),
        &Overlay::ConfirmApply {
            id: "u0".into(),
            hunks: vec![1]
        }
    );
    let text = d.render(100, 30);
    assert!(text.contains("Put this hunk into a.rs? (+1 −1)"), "{text}");
    assert_eq!(
        d.keys("y"),
        vec![Effect::ApplyHunks {
            unit_id: "u0".into(),
            hunks: vec![1]
        }]
    );
    assert_eq!(d.overlay(), &Overlay::None);
    assert_eq!(d.status(), "Putting the hunk in…");
    // Every open one; `n` says no.
    d.keys("P");
    assert_eq!(
        d.overlay(),
        &Overlay::ConfirmApply {
            id: "u0".into(),
            hunks: vec![1, 2]
        }
    );
    let text = d.render(100, 30);
    assert!(text.contains("Put 2 hunks into 2 files?"), "{text}");
    assert!(d.keys("n").is_empty());
    assert_eq!(d.overlay(), &Overlay::None);
    assert_eq!(states(&d, "u0")[1..], [HunkState::Proposed; 2]);
    // One already in: said so.
    d.keys("[c");
    assert_eq!(d.app.current_fix(), Some(0));
    assert!(d.keys("p").is_empty());
    assert_eq!(d.overlay(), &Overlay::None);
    assert_eq!(d.status(), "This hunk is put in already.");
    // The window's prompt asks the same.
    d.keys("P");
    let view = snapshot(&d.app, None);
    match view.overlay {
        OverlayView::ConfirmApply { question } => {
            assert_eq!(question, "Put 2 hunks into 2 files?")
        }
        o => panic!("{o:?}"),
    }
}

#[test]
fn p_from_the_queue_opens_the_unit_at_its_first_open_hunk_and_asks() {
    let mut d = driver();
    let mut p = three();
    p.hunks[0].state = HunkState::Applied;
    p.hunks[2].parses = false;
    propose(&mut d, "u0", p);
    assert_eq!(d.selected().as_deref(), Some("u0"));
    d.keys("p");
    assert_eq!(d.screen(), Screen::Unit);
    assert_eq!(
        d.overlay(),
        &Overlay::ConfirmApply {
            id: "u0".into(),
            hunks: vec![1]
        }
    );
    // Drawn, the text is at that hunk.
    let text = d.render(100, 30);
    d.keys("<esc>");
    let fixes = d.app.unit.fixes.borrow().clone();
    assert_eq!(
        d.app.detail_offset(),
        fixes[1].min(d.app.unit.text_max.get()),
        "{text}"
    );
    // A hunk that doesn't parse is asked about as such.
    d.keys("P");
    let text = d.render(100, 30);
    assert!(
        text.contains("Put 2 hunks into 2 files? 1 of them don't parse."),
        "{text}"
    );
}

#[test]
fn nothing_to_put_in_says_how_to_ask_or_that_it_is_in() {
    let mut d = driver();
    d.keys("l");
    assert!(d.keys("p").is_empty());
    assert_eq!(d.status(), "No proposed change: ␣e asks for one.");
    assert!(d.keys("P").is_empty());
    assert_eq!(d.status(), "No proposed change: ␣e asks for one.");
    let mut p = three();
    for h in &mut p.hunks {
        h.state = HunkState::Applied;
    }
    propose(&mut d, "u0", p);
    d.render(100, 40);
    assert!(d.keys("P").is_empty());
    assert_eq!(d.status(), "Every proposed hunk is put in already.");
}

fn pr() -> PrInfo {
    PrInfo {
        host: "github.com".into(),
        port: None,
        base_assumed: None,
        owner: "o".into(),
        repo: "r".into(),
        number: 7,
        remote: None,
        head_sha: Some("head".into()),
        approved_head: None,
        repo_dir: None,
    }
}

#[test]
fn on_a_pull_request_they_become_suggestions_and_flag_the_unit() {
    let mut d = driver();
    d.app.review.pr = Some(pr());
    propose(&mut d, "u0", three());
    let text = open(&mut d, 120);
    assert!(text.contains("p suggest · P suggest all"), "{text}");
    // Nothing is written: the hunk is a suggestion, the unit flagged.
    assert_eq!(d.keys("p"), vec![Effect::Save]);
    assert_eq!(d.overlay(), &Overlay::None);
    assert_eq!(
        states(&d, "u0"),
        [
            HunkState::Suggested,
            HunkState::Proposed,
            HunkState::Proposed
        ]
    );
    assert_eq!(d.decision_of("u0"), Decision::Investigate);
    assert_eq!(d.status(), "Suggested: it goes into the review on :w.");
    assert_eq!(d.app.unit.id, "u0", "the view stays on the unit");
    let text = d.render(100, 120);
    assert!(text.contains("suggested ╮"), "{text}");
    assert!(d.keys("p").is_empty());
    assert_eq!(
        d.status(),
        "This hunk is suggested already: it goes into the review on :w."
    );
    // `u` takes the flag back.
    d.keys("u");
    assert_eq!(d.decision_of("u0"), Decision::Pending);
    // The rest; the unit flagged again, once.
    assert_eq!(d.keys("P"), vec![Effect::Save]);
    assert_eq!(states(&d, "u0"), [HunkState::Suggested; 3]);
    assert_eq!(d.decision_of("u0"), Decision::Investigate);
    assert!(d.keys("P").is_empty());
    assert!(
        d.status()
            .starts_with("Every proposed hunk is suggested already")
    );
    assert!(
        !d.effects
            .iter()
            .any(|e| matches!(e, Effect::ApplyHunks { .. }))
    );
}

#[test]
fn written_they_are_put_in_and_the_review_runs_again() {
    let mut d = driver();
    propose(&mut d, "u0", three());
    let effects = d.app.on_app_event(AppEvent::FixApplied {
        unit_id: "u0".into(),
        hunks: vec![0, 2],
        result: Ok(vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]),
    });
    assert_eq!(effects, vec![Effect::Save, Effect::Rerun]);
    assert_eq!(
        states(&d, "u0"),
        [HunkState::Applied, HunkState::Proposed, HunkState::Applied]
    );
    assert_eq!(d.status(), "Put into 2 files; reviewing again.");
    // The re-run says what it did: the unit is as at the base again.
    let mut again = d.app.review.clone();
    again.units.retain(|u| u.id != "u0");
    d.app.on_app_event(AppEvent::Rerun(Ok(Box::new(again))));
    assert_eq!(
        d.status(),
        "1 unit left the review by the proposed change: as at the base again."
    );
    propose(&mut d, "u1", three());
    let effects = d.app.on_app_event(AppEvent::FixApplied {
        unit_id: "u1".into(),
        hunks: vec![1],
        result: Err("a.rs changed: the text isn't there once".into()),
    });
    assert!(effects.is_empty());
    assert_eq!(states(&d, "u1")[1], HunkState::Proposed);
    assert_eq!(
        d.status(),
        "Not put in: a.rs changed: the text isn't there once"
    );
}

fn dives(effects: &[Effect]) -> Vec<(String, DiveKind)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Dive { unit_id, kind, .. } => Some((unit_id.clone(), *kind)),
            _ => None,
        })
        .collect()
}

fn finding(severity: Risk) -> Finding {
    let cite = |line| Cite {
        file: PathBuf::from("a.rs"),
        start: line,
        end: line,
    };
    Finding {
        class: Class::Authorization,
        severity,
        confidence: Risk::High,
        title: TITLE.into(),
        source: cite(2),
        path: Vec::new(),
        sink: cite(17),
        missing: "the owner check".into(),
        exploit: "tenant B reads tenant A's asset".into(),
        path_verified: true,
        second: None,
        proof: None,
    }
}

fn audit(d: &mut Driver, findings: Vec<Finding>) {
    d.app.review.unit_mut("u0").unwrap().dive.results.insert(
        DiveKind::Security,
        DiveResult {
            audit: Some(Audit {
                findings,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
}

#[test]
fn leader_e_asks_for_a_change_when_there_is_something_to_fix() {
    // No agent: said so, in the unit view.
    let mut d = driver();
    d.app.has_agent = false;
    assert!(dives(&d.keys("<space>e")).is_empty());
    assert_eq!(d.screen(), Screen::Unit);
    assert_eq!(d.status(), "No agent: proposing a change needs one.");
    // A tile opens beside the text, has the focus and says so too.
    assert_eq!(d.app.unit.tiles, [Panel::Fix]);
    assert_eq!(d.app.unit.panel, Some(Panel::Fix));
    let text = d.render(150, 40);
    assert!(text.contains("╭ ␣e Proposed change"), "{text}");
    assert!(
        text.contains("▎ No agent: proposing a change needs one."),
        "{text}"
    );

    // Nothing to fix yet: how to get something.
    agent(&mut d);
    assert!(dives(&d.keys("<space>e")).is_empty());
    assert_eq!(
        d.status(),
        "Nothing to fix yet: ␣x finds security issues, K asks what to do, i writes a note."
    );
    // The tile says what holds now; the text has no section for it.
    let text = d.render(150, 40);
    assert_eq!(text.matches("▎ Nothing to fix yet").count(), 1, "{text}");
    assert_eq!(text.matches("Proposed change").count(), 1, "{text}");
    // A finding that doesn't stand isn't one.
    let mut disputed = finding(Risk::High);
    disputed.second = Some(SecondCheck {
        agree: false,
        why: "the id is cast upstream".into(),
        evidence: None,
    });
    audit(&mut d, vec![disputed]);
    assert!(dives(&d.keys("<space>e")).is_empty());

    // A note: asked once; on its way, said so. The tile closes: the text
    // has the change.
    d.app.review.unit_mut("u0").unwrap().dive.note = "check the owner".into();
    assert_eq!(dives(&d.keys("<space>e")), [("u0".into(), DiveKind::Fix)]);
    assert!(!d.app.unit.tiles.contains(&Panel::Fix));
    assert!(
        d.app
            .review
            .unit("u0")
            .unwrap()
            .dive_pending
            .contains(&DiveKind::Fix)
    );
    assert!(dives(&d.keys("<space>e")).is_empty());
    assert!(
        d.status().starts_with("A change is on its way"),
        "{}",
        d.status()
    );

    // A finding that stands, from the queue: the unit view opens on it.
    let mut d = driver();
    audit(&mut d, vec![finding(Risk::Medium)]);
    assert_eq!(dives(&d.keys("<space>e")), [("u0".into(), DiveKind::Fix)]);
    assert_eq!((d.screen(), d.app.unit.id.as_str()), (Screen::Unit, "u0"));

    // The advice.
    let mut d = driver();
    d.app.review.unit_mut("u0").unwrap().dive.results.insert(
        DiveKind::Recommend,
        DiveResult {
            text: "Check the owner before loading.".into(),
            ..Default::default()
        },
    );
    assert_eq!(dives(&d.keys("<space>e")), [("u0".into(), DiveKind::Fix)]);
}

#[test]
fn leader_e_with_a_proposal_shows_its_first_open_hunk() {
    let mut d = driver();
    let mut p = three();
    p.hunks[0].state = HunkState::Applied;
    propose(&mut d, "u0", p);
    // From the queue: the text scrolls there once it is drawn.
    assert!(dives(&d.keys("<space>e")).is_empty());
    assert_eq!(d.screen(), Screen::Unit);
    assert_eq!(d.app.unit.fix, Some(1));
    d.render(100, 20);
    let fixes = d.app.unit.fixes.borrow().clone();
    let max = d.app.unit.text_max.get();
    assert_eq!(d.app.detail_offset(), fixes[1].min(max));
    // In the view, from a tile: the text gets the focus and scrolls now.
    d.keys("gg<space>t");
    assert!(d.app.unit.panel.is_some());
    d.render(100, 20);
    d.keys("<space>e");
    assert_eq!(d.app.unit.panel, None);
    assert_eq!(d.app.detail_offset(), fixes[1].min(max));
    assert_eq!(d.app.current_fix(), Some(1));
}

#[test]
fn the_queue_preview_says_a_change_is_proposed() {
    let mut d = driver();
    let line = "✎ a change is proposed: 3 hunks · ␣e shows it";
    assert!(!d.render(120, 40).contains("a change is proposed"));
    propose(&mut d, "u0", three());
    let text = d.render(120, 40);
    assert!(text.contains(line), "{text}");
    // Counted until put in, then gone.
    let mut p = three();
    p.hunks[0].state = HunkState::Applied;
    propose(&mut d, "u0", p);
    let text = d.render(120, 40);
    assert!(text.contains("a change is proposed: 2 hunks"), "{text}");
    let mut p = three();
    for h in &mut p.hunks {
        h.state = HunkState::Suggested;
    }
    propose(&mut d, "u0", p);
    assert!(!d.render(120, 40).contains("a change is proposed"));
}

#[test]
fn the_window_gets_the_proposal_with_its_hunks_and_the_current_one() {
    let mut d = driver();
    let view = snapshot(&d.app, None);
    assert!(view.preview.as_ref().unwrap().proposed.is_none());
    propose(&mut d, "u0", three());
    let view = snapshot(&d.app, None);
    assert_eq!(view.preview.as_ref().unwrap().proposed, Some(3));
    d.keys("l");
    let view = snapshot(&d.app, None);
    let p = view.unit.unwrap().proposal.unwrap();
    assert_eq!(p.answers.as_deref(), Some(TITLE));
    assert_eq!(p.severity, Some(Risk::High));
    assert_eq!(p.hunks.len(), 3);
    assert_eq!(p.hunks[0].place, "a.rs:4-6");
    assert_eq!(p.hunks[0].code.rows.len(), 4);
    assert_eq!((p.hunks[0].code.added, p.hunks[0].code.removed), (1, 1));
    assert_eq!(p.hunks[0].code.hunks, [1]);
    assert_eq!(p.current, Some(0));
    assert!(!p.suggest);
    // A click picks the hunk `fix.apply` puts in.
    d.app.select_fix(2);
    let view = snapshot(&d.app, None);
    assert_eq!(view.unit.unwrap().proposal.unwrap().current, Some(2));
    d.app.press(teer::keymap::Action::FixApply, None);
    assert_eq!(
        d.overlay(),
        &Overlay::ConfirmApply {
            id: "u0".into(),
            hunks: vec![2]
        }
    );
}

// ---- Asked on its own ------------------------------------------------------

/// The audit of `u0` came: `findings`, `checking` while a second model
/// checks them.
fn audited(d: &mut Driver, findings: Vec<Finding>, checking: bool) -> Vec<Effect> {
    d.app.on_app_event(AppEvent::Dive(DiveEvent::Audit {
        id: "u0".into(),
        fingerprint: "fp0".into(),
        text: "one finding".into(),
        audit: Box::new(Audit {
            findings,
            checking,
            ..Default::default()
        }),
    }))
}

#[test]
fn a_high_finding_that_stands_has_the_agent_propose_a_change_once() {
    let mut d = driver();
    let effects = audited(&mut d, vec![finding(Risk::High)], false);
    assert_eq!(dives(&effects), [("u0".into(), DiveKind::Fix)]);
    assert!(
        d.app
            .review
            .unit("u0")
            .unwrap()
            .dive_pending
            .contains(&DiveKind::Fix)
    );
    // Once: the same audit again asks nothing more.
    let effects = audited(&mut d, vec![finding(Risk::High)], false);
    assert!(dives(&effects).is_empty(), "{effects:?}");
}

#[test]
fn not_with_propose_fixes_never() {
    let mut d = driver();
    d.app.cfg.propose_fixes = ProposeFixes::Never;
    let effects = audited(&mut d, vec![finding(Risk::High)], false);
    assert!(dives(&effects).is_empty(), "{effects:?}");
}

#[test]
fn not_while_a_second_model_checks_the_findings_then_on_its_word() {
    let mut d = driver();
    let effects = audited(&mut d, vec![finding(Risk::High)], true);
    assert!(dives(&effects).is_empty(), "{effects:?}");
    // It agrees: the finding stands.
    let effects = d.app.on_app_event(AppEvent::Dive(DiveEvent::AuditChecked {
        id: "u0".into(),
        fingerprint: "fp0".into(),
        checks: Some(vec![(
            0,
            SecondCheck {
                agree: true,
                why: "the path holds".into(),
                evidence: None,
            },
        )]),
    }));
    assert_eq!(dives(&effects), [("u0".into(), DiveKind::Fix)]);
    // It disputes: nothing is asked.
    let mut d = driver();
    audited(&mut d, vec![finding(Risk::High)], true);
    let effects = d.app.on_app_event(AppEvent::Dive(DiveEvent::AuditChecked {
        id: "u0".into(),
        fingerprint: "fp0".into(),
        checks: Some(vec![(
            0,
            SecondCheck {
                agree: false,
                why: "the id is cast upstream".into(),
                evidence: None,
            },
        )]),
    }));
    assert!(dives(&effects).is_empty(), "{effects:?}");
}

#[test]
fn not_for_medium_findings_alone() {
    let mut d = driver();
    let effects = audited(
        &mut d,
        vec![finding(Risk::Medium), finding(Risk::Low)],
        false,
    );
    assert!(dives(&effects).is_empty(), "{effects:?}");
    // Nor without an agent.
    let mut d = driver();
    d.app.has_agent = false;
    let effects = audited(&mut d, vec![finding(Risk::High)], false);
    assert!(dives(&effects).is_empty(), "{effects:?}");
}

#[test]
fn a_lone_tile_shares_its_column_with_the_keys_that_look_further() {
    let mut d = driver();
    d.app.review.unit_mut("u0").unwrap().summary = Some("Casts the id.".into());
    // The queue's preview: what the agent wrote is in a box of its own.
    let text = d.render(150, 40);
    assert!(text.contains("╭ review ─"), "{text}");
    assert!(text.contains("│ Casts the id."), "{text}");

    // The unit view: the ask tile, and under it the keys that look further.
    d.keys("l");
    let text = d.render(150, 40);
    let (ask, inspect) = (
        row_of(&text, "Ask about it"),
        row_of(&text, "Inspect further"),
    );
    // Two thirds of the 38 rows under the title, then one third.
    assert_eq!(inspect - ask, 24, "{text}");
    for want in [
        "These keys look further into this code change:",
        "␣t     find the tests that reach it",
        "␣e     have the agent propose a change",
        "gd     show what it calls",
    ] {
        assert!(text.contains(want), "{want}:\n{text}");
    }
    // A second tile takes the room.
    d.keys("<space>t");
    assert!(!d.render(150, 40).contains("Inspect further"));
    // Too short for both: the tile alone.
    let mut d = driver();
    d.keys("l");
    assert!(!d.render(150, 16).contains("Inspect further"));
}
