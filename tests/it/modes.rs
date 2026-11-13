//! Modes (`teer review`, `teer security`, `<space>m`): the
//! mode a review opens in, what each mode shows and leaves out, and the
//! picker.

use crate::common;

use clap::Parser;
use common::v2::{Driver, units};
use common::*;
use teer::cli::{Args, Screens, run_on};
use teer::config::Config;
use teer::keymap::Keymap;
use teer::mode::Mode;
use teer::review::model::{DiveKind, SessionRef};
use teer::session::suggest::Offer;
use teer::session::unit::Panel;
use teer::session::{Effect, Overlay, Screen};
use teer::start::{Launch, mode_for};
use teer::tui::setup::{Ended, Opened};

#[derive(Default)]
struct Fake {
    launched: std::cell::RefCell<Option<Launch>>,
}

#[async_trait::async_trait(?Send)]
impl Screens for Fake {
    async fn setup(
        &self,
        _cfg: Config,
        _path: &std::path::Path,
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

/// Start teer with `args` in a repository with one changed file; what it
/// launched.
async fn launch(args: &[&str]) -> Launch {
    isolate_env();
    let r = FixtureRepo::new();
    r.write("shop.py", "def price(q):\n    return q * 10\n")
        .commit("init");
    r.write("shop.py", "def price(q):\n    return q * 11\n");
    let d = tempfile::tempdir().unwrap();
    let dir = r.path();
    let args = Args::try_parse_from(
        ["teer", "-C", dir.to_str().unwrap(), "--no-agent"]
            .into_iter()
            .chain(args.iter().copied()),
    )
    .unwrap();
    let fake = Fake::default();
    run_on(args, &d.path().join("config.toml"), &fake)
        .await
        .unwrap();
    fake.launched.into_inner().expect("a review")
}

#[tokio::test]
async fn a_mode_command_names_the_mode_and_plain_teer_leaves_it_open() {
    assert_eq!(launch(&[]).await.mode, None);
    assert_eq!(launch(&["review"]).await.mode, Some(Mode::Review));
    let l = launch(&["security"]).await;
    assert_eq!(l.mode, Some(Mode::Security));
    assert_eq!(l.review.files.len(), 1);
}

#[tokio::test]
async fn a_review_opens_in_the_chosen_mode_else_the_configs_else_by_what_it_is() {
    let l = launch(&[]).await;
    let mut review = l.review;
    let mut cfg = Config::default();
    // Nothing names a mode: a review.
    assert_eq!(mode_for(&review, &cfg), Mode::Review);
    cfg.mode = Some(Mode::All);
    assert_eq!(mode_for(&review, &cfg), Mode::All);
    // What the reviewer chose for this review wins, and is saved with it.
    review.mode = Some(Mode::Security);
    assert_eq!(mode_for(&review, &cfg), Mode::Security);
    let json = serde_json::to_string(&review).unwrap();
    assert!(json.contains(r#""mode":"security""#), "{json}");
    let cfg: Config = toml::from_str("mode = \"review\"\n").unwrap();
    assert_eq!(cfg.mode, Some(Mode::Review));
}

/// A review of three units with an agent, in `mode`.
fn driver(mode: Mode) -> Driver {
    let mut d = Driver::new(3);
    d.app.has_agent = true;
    d.app.review.agent_session = Some(SessionRef::default());
    d.app.security.enabled = true;
    d.app.set_mode(mode);
    d
}

#[test]
fn the_header_names_the_mode_but_not_all() {
    let first = |d: &Driver| d.render(100, 20).lines().next().unwrap().to_string();
    assert!(first(&driver(Mode::Review)).starts_with(" review ␣m · "));
    assert!(first(&driver(Mode::Security)).starts_with(" security ␣m · "));
    let all = first(&driver(Mode::All));
    assert!(!all.contains("all ·"), "{all}");
}

#[test]
fn the_help_and_the_keys_tile_list_only_what_the_mode_has() {
    // What each mode's help of the unit view says, and doesn't.
    let cases: [(Mode, &[&str], &[&str]); 2] = [
        (
            Mode::Review,
            &["ask about the unit", "tests that cover", "propose a change"],
            &["security: how", "prove the security"],
        ),
        (
            Mode::Security,
            &["ask about the unit", "security: how", "propose a change"],
            &["tests that cover", "commits and PR description"],
        ),
    ];
    for (mode, has, lacks) in cases {
        let mut d = driver(mode);
        d.keys("l");
        // The keys tile, beside the text.
        let tile = d.render(160, 50);
        d.keys("?");
        let help = d.render(160, 60);
        for word in has {
            assert!(help.contains(word), "{mode:?} help lacks {word}:\n{help}");
        }
        for word in lacks {
            assert!(!help.contains(word), "{mode:?} help has {word}:\n{help}");
        }
        assert!(help.contains("mode: review or security"), "{help}");
        assert!(tile.contains("␣m mode"), "{mode:?}:\n{tile}");
        let tests = tile.contains("␣t tests");
        assert_eq!(tests, mode == Mode::Review, "{mode:?}:\n{tile}");
    }
}

#[test]
fn a_tool_of_another_mode_does_nothing_and_the_prefix_hint_leaves_it_out() {
    let mut d = driver(Mode::Review);
    let e = d.keys("<space>x");
    assert!(e.is_empty(), "{e:?}");
    assert_eq!(d.screen(), Screen::Queue);
    assert!(
        d.status().starts_with("Not in review mode"),
        "{}",
        d.status()
    );
    // After the prefix, the bar lists what can follow.
    d.keys("<space>");
    let bar = d.bar(200, 20);
    assert!(bar.contains("tests that cover"), "{bar}");
    assert!(!bar.contains("security: how"), "{bar}");
    assert!(!bar.contains("prove the security"), "{bar}");
}

/// A possible risk on unit `i`, with the checks that follow it up.
fn suspect(d: &mut Driver, i: usize, checks: &[&str]) {
    use teer::review::model::{LineRef, Prep, Risk, Side};
    use teer::security::{Class, Suspect};
    let u = &mut d.app.review.units[i];
    u.prep = Prep::Ready;
    let line = u.new_range.map_or(1, |r| r.start);
    u.checks.suspects = vec![Suspect {
        class: Class::Authorization,
        severity: Risk::Medium,
        text: "A caller can pass any asset id.".into(),
        evidence: vec![LineRef {
            side: Side::New,
            line,
        }],
        checks: checks.iter().map(|c| c.to_string()).collect(),
    }];
}

#[test]
fn security_mode_opens_units_on_the_ask_tile_and_offers_the_checks() {
    let mut d = driver(Mode::Security);
    assert!(d.app.security.by_exposure);
    let i = d.app.order[d.app.sel];
    suspect(
        &mut d,
        i,
        &["Which callers check that the user owns the asset?"],
    );
    let e = d.keys("l");
    // The same tile as in a review, and nothing asked yet.
    assert_eq!(d.app.unit.tiles, vec![Panel::Ask]);
    assert!(!e.iter().any(|x| matches!(x, Effect::Dive { .. })), "{e:?}");
    let offered: Vec<_> = d.app.suggestions().into_iter().map(|s| s.offer).collect();
    assert_eq!(offered, [Offer::Question, Offer::Audit]);
    let shown = d.render(160, 40);
    // No advice here: the check that follows the risk up, with what it
    // is about, then the full check.
    assert!(!shown.contains("What should I do"), "{shown}");
    assert!(
        shown.contains("1 Which callers check that the user owns the asset? (auth · medium)"),
        "{shown}"
    );
    assert!(
        shown.contains("2 Check every path into this unit"),
        "{shown}"
    );
    // `enter` on an empty question asks the first, as the reviewer's own.
    // (The first question of a session asks whether it may use the web.)
    d.keys("K<cr>");
    assert_eq!(d.overlay(), &Overlay::ConfirmWeb);
    let e = d.keys("n");
    assert!(e.iter().any(|x| matches!(x, Effect::Ask { .. })), "{e:?}");
    let asked = d.app.review.questions.last().unwrap();
    assert_eq!(
        asked.text,
        "Which callers check that the user owns the asset?"
    );
    // Asked: no longer offered. What its answer leads to is, first.
    assert_eq!(d.app.suggestions().len(), 1);
    let n = d.app.review.questions.len() - 1;
    d.app.review.questions[n].answer = Some(teer::review::model::Answer {
        text: "Open: AssetController::show calls it without a check.".into(),
        next: vec!["Does a test cover a user without access?".into()],
        ..Default::default()
    });
    let next = d.app.suggestions();
    assert_eq!(next[0].text, "Does a test cover a user without access?");
    assert_eq!(next[0].about, "the last answer");
    // The full check takes its number: a bare enter doesn't start it.
    let e = d.keys("K2<cr>");
    assert!(
        e.iter().any(|x| matches!(
            x,
            Effect::Dive {
                kind: DiveKind::Security,
                ..
            }
        )),
        "{e:?}"
    );
    assert_eq!(d.app.unit.panel, Some(Panel::Tool(DiveKind::Security)));
}

#[test]
fn the_code_has_the_risks_in_security_and_what_changed_in_a_review() {
    let mut d = driver(Mode::Security);
    let i = d.app.order[d.app.sel];
    suspect(&mut d, i, &[]);
    let said = |d: &Driver| {
        let u = &d.app.review.units[i];
        teer::review::comments::comments(u, d.app.mode)
            .into_iter()
            .map(|c| c.title)
            .collect::<Vec<_>>()
    };
    assert_eq!(said(&d), ["possible · auth · medium"]);
    d.app.set_mode(Mode::Review);
    assert!(!said(&d).iter().any(|t| t.starts_with("possible")));
}

#[test]
fn the_picker_lists_the_modes_and_enter_takes_one() {
    let mut d = driver(Mode::Review);
    d.keys("<space>m");
    assert_eq!(d.overlay(), &Overlay::Mode { sel: 0 });
    let shown = d.render(120, 30);
    for m in Mode::EVERY {
        assert!(shown.contains(m.name()), "{}:\n{shown}", m.name());
        assert!(shown.contains(m.about()), "{}:\n{shown}", m.name());
    }
    assert!(shown.contains("(now)"), "{shown}");
    let e = d.keys("j<cr>");
    assert_eq!(e, vec![Effect::Save]);
    assert_eq!(d.app.mode, Mode::Security);
    assert_eq!(d.app.review.mode, Some(Mode::Security));
    assert_eq!(d.overlay(), &Overlay::None);
    assert!(d.render(120, 30).starts_with(" security ␣m · "));
    // A review of units only: every unit still listed.
    assert_eq!(d.app.order.len(), units(3).len());
}

// ---- `?` lists only what does something now -----------------------------------

#[test]
fn the_help_lists_only_what_does_something_right_now() {
    let help = |d: &mut Driver| {
        d.keys("?");
        let s = d.render(160, 60);
        d.keys("<esc>");
        s
    };
    let mut d = driver(Mode::All);
    let s = help(&mut d);
    // Nothing decided, noted or flagged yet: nothing to undo, repeat,
    // reset or list.
    for gone in [
        "undo the last decision",
        "repeat the last decision",
        "re-run and reset every decision",
        "every note",
        "flagged units, one after another",
        "show units accepted for you",
        "commits and PR description",
    ] {
        assert!(!s.contains(gone), "{gone}:\n{s}");
    }
    for there in ["accept / flag the unit", "edit the note", "finish"] {
        assert!(s.contains(there), "{there}:\n{s}");
    }
    // A flag and a note later, they are there.
    d.keys("f");
    d.keys("ggia note<cr>");
    let s = help(&mut d);
    for there in [
        "undo the last decision",
        "repeat the last decision",
        "re-run and reset every decision",
        "every note",
        "flagged units, one after another",
    ] {
        assert!(s.contains(there), "{there}:\n{s}");
    }
    // The unit view: no hunk to put in while none is proposed, and the
    // tile keys only with a tile open.
    d.keys("l");
    let s = help(&mut d);
    assert!(!s.contains("put the proposed hunk"), "{s}");
    assert!(s.contains("next / previous tile"), "{s}");
    assert!(s.contains("zoom the tile in focus"), "{s}");
    // Without an agent no tile is open, and what needs one is not listed.
    let mut d = Driver::new(3);
    d.keys("l");
    let s = help(&mut d);
    for gone in ["next / previous tile", "ask about the unit", "advice"] {
        assert!(!s.contains(gone), "{gone}:\n{s}");
    }
    assert!(s.contains("tests that cover this unit"), "{s}");
}
