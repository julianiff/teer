//! v2 Phase 3, the prepare pipeline's output (P1–P8 of docs/v2-map.md §11):
//! ○ units accepted for you (`accepted_by` auto) and hidden until `za`, the
//! `auto_accept` switch, a disputed ○, `u` on a unit accepted for you, the
//! `$ ⚠ ≠` marks as written reasons, what the queue rows show, the queue
//! order and the first unit, and `--json`.
//!
//! Through the library: an [`App`] on a review made here, or on one a
//! scripted agent prepared from a fixture repo, keys typed as in
//! tests/it/common/v2.rs. `--json` runs the binary (tests/it/common/sandbox.rs).

use crate::common;

use std::collections::HashMap;
use std::sync::Arc;

use common::sandbox::Sandbox;
use common::v2::{Driver, review, unit};
use common::*;
use serde_json::{Value, json};
use teer::agent::mock::MockAdapter;
use teer::config::Config;
use teer::keymap::{Action, Keymap};
use teer::review::attention::Level;
use teer::review::intent::Brief;
use teer::review::model::{
    AcceptedBy, Cost, Decision, Range, Review, Risk, SecondOpinion, SessionRef, Unit, UnitStatus,
    Verdict,
};
use teer::review::prepare::prepare;
use teer::review::prompts::SECOND_RULES;
use teer::review::{BuildOptions, build_review};
use teer::session::app::Grouping;
use teer::tui::app::App;

// ---- The Phase 3 API these tests expect: adapt here after merging -----------

/// Accepted for the user: `decision = Accepted` with `accepted_by = auto`.
fn accepted_for_you(u: &Unit) -> bool {
    u.decision == Decision::Accepted && u.accepted_by == Some(AcceptedBy::Auto)
}

/// Not accepted for the user (pending, or decided by the user).
fn not_for_you(u: &Unit) -> bool {
    u.accepted_by != Some(AcceptedBy::Auto)
}

/// The unit's written reasons, shown under "Why this level".
fn reasons(u: &Unit) -> Vec<String> {
    u.reasons.clone()
}

/// The default config with `auto_accept = "<mode>"`.
fn auto_accept(mode: &str) -> Config {
    toml::from_str(&format!("auto_accept = \"{mode}\"\n")).expect("a valid config")
}

/// List by level, the plan's queue order (`zf` switches to the file boxes).
fn by_level(d: &mut Driver) {
    d.app.grouping = Grouping::Unit;
    d.app.rebuild_order();
}

/// Open `review` as teer does: the [`App`], then its start, without an agent.
fn open(review: Review, cfg: Config) -> Driver {
    isolate_env();
    let mut app = App::new(review, cfg, Keymap::default());
    let effects = app.start(
        false,
        Some(teer::session::app::NO_AGENT.into()),
        Vec::new(),
        None,
    );
    // The queue listed by file (`zf`); by level is tests/it/v2_queue.rs's.
    app.grouping = Grouping::File;
    app.rebuild_order();
    Driver { app, effects }
}

/// The level unit `id` is shown at.
fn level(d: &Driver, id: &str) -> Level {
    d.app.assessment(id).expect("unit exists").level
}

// ---- Units -----------------------------------------------------------------

/// `file::name`: a changed function, pending, ● (no agent asked).
fn changed(file: &str, name: &str) -> Unit {
    let mut u = unit(0, file);
    u.id = format!("{file}::{name}");
    u.symbol = name.into();
    u.fingerprint = format!("fp {file} {name}");
    u
}

/// ○: formatting only.
fn same(file: &str, name: &str) -> Unit {
    let mut u = changed(file, name);
    u.status = UnitStatus::NoBehaviorChange;
    u.nbc_reason = Some("formatting only".into());
    u
}

/// ▲: removed while something still calls it.
fn removed_but_called(file: &str, name: &str) -> Unit {
    let mut u = changed(file, name);
    u.status = UnitStatus::Removed;
    u.callers = vec!["elsewhere.rs::caller".into()];
    u
}

/// ◆: expensive to undo.
fn expensive(file: &str, name: &str) -> Unit {
    let mut u = changed(file, name);
    u.checks.cost = Some(Cost {
        why: "changes the orders table schema".into(),
        evidence: Vec::new(),
    });
    u
}

/// `u`, with a second model that disputes its description because `why`.
fn disputed(mut u: Unit, why: &str) -> Unit {
    u.checks.second = Some(SecondOpinion {
        risk: Risk::Low,
        verdicts: vec![Verdict {
            statement: "Only reformats the call.".into(),
            agree: false,
            why: why.into(),
            evidence: Vec::new(),
        }],
        missed: Vec::new(),
    });
    u
}

/// Two ○ units among two others: `f0.rs` keep_total ●, format_me ○;
/// `f1.rs` rename_vat ○, charge_more ●.
fn mixed() -> Review {
    review(vec![
        changed("f0.rs", "keep_total"),
        same("f0.rs", "format_me"),
        same("f1.rs", "rename_vat"),
        changed("f1.rs", "charge_more"),
    ])
}

/// The ids in the queue, in its order.
fn listed(d: &Driver) -> Vec<String> {
    d.app
        .order
        .iter()
        .map(|&i| d.app.review.units[i].id.clone())
        .collect()
}

fn get<'a>(d: &'a Driver, id: &str) -> &'a Unit {
    d.app.review.unit(id).expect("unit exists")
}

// ---- P1: ○ is accepted for you, hidden until `za` --------------------------

#[test]
fn no_behavior_change_is_accepted_for_you() {
    let mut d = open(mixed(), Config::default());
    assert!(accepted_for_you(get(&d, "f0.rs::format_me")));
    assert!(accepted_for_you(get(&d, "f1.rs::rename_vat")));
    for id in ["f0.rs::keep_total", "f1.rs::charge_more"] {
        assert_eq!(get(&d, id).decision, Decision::Pending, "{id}");
        assert!(not_for_you(get(&d, id)), "{id}");
    }

    // Hidden from the queue, and said so at the bottom of the list.
    assert_eq!(listed(&d), ["f0.rs::keep_total", "f1.rs::charge_more"]);
    let za = d.app.keys.label(Action::ToggleAutoAccepted);
    assert_eq!(za, "za");
    let line = format!("2 units accepted for you, {za} shows them");
    let screen = d.render(120, 30);
    assert!(screen.contains(&line), "{screen}");

    // `za` shows them, and hides them again.
    d.keys("za");
    let shown = listed(&d);
    assert_eq!(shown.len(), 4, "{shown:?}");
    assert!(shown.contains(&"f0.rs::format_me".to_string()), "{shown:?}");
    assert!(
        shown.contains(&"f1.rs::rename_vat".to_string()),
        "{shown:?}"
    );
    d.keys("za");
    assert_eq!(listed(&d), ["f0.rs::keep_total", "f1.rs::charge_more"]);
}

// ---- P2: a disputed ○ ------------------------------------------------------

#[test]
fn disputed_no_change_is_not_auto_accepted() {
    // Per Q2 no model checks ○ units; a dispute that is there anyway raises
    // the level, as it does for any unit, and the unit is the user's to review.
    let d = open(
        review(vec![
            changed("f0.rs", "keep_total"),
            disputed(
                same("f0.rs", "format_me"),
                "the retry wasn't removed, it moved to client.ts",
            ),
            same("f1.rs", "rename_vat"),
        ]),
        Config::default(),
    );
    assert!(level(&d, "f0.rs::format_me") > Level::Same, "goes up");
    assert_eq!(get(&d, "f0.rs::format_me").decision, Decision::Pending);
    assert!(not_for_you(get(&d, "f0.rs::format_me")));
    assert!(
        listed(&d).contains(&"f0.rs::format_me".to_string()),
        "in the queue without za: {:?}",
        listed(&d)
    );
    // The undisputed one still is accepted for you.
    assert!(accepted_for_you(get(&d, "f1.rs::rename_vat")));
    let screen = d.render(120, 30);
    assert!(screen.contains("1 unit accepted for you"), "{screen}");
}

// ---- P3: auto_accept = "none" ----------------------------------------------

#[test]
fn auto_accept_none_keeps_units_pending() {
    let d = open(mixed(), auto_accept("none"));
    for id in ["f0.rs::format_me", "f1.rs::rename_vat"] {
        assert_eq!(get(&d, id).decision, Decision::Pending, "{id}");
        assert!(not_for_you(get(&d, id)), "{id}");
    }
    assert_eq!(d.app.decision_counts().0, 0, "nothing accepted");
    let screen = d.render(120, 30);
    assert!(!screen.contains("accepted for you"), "{screen}");

    // The other value, which is also the default.
    let d = open(mixed(), auto_accept("no_behavior_change"));
    assert!(accepted_for_you(get(&d, "f0.rs::format_me")));
    assert!(accepted_for_you(get(&d, "f1.rs::rename_vat")));
}

// ---- P4: they count for finishing; `u` makes one pending -------------------

#[test]
fn auto_accepted_count_and_undo() {
    let mut d = open(mixed(), Config::default());
    assert_eq!(
        d.app.decision_counts(),
        (2, 0, 2),
        "(accepted, flagged, pending): accepted for you counts as accepted"
    );
    d.select_id("f0.rs::keep_total");
    d.keys("a");
    assert!(!d.app.all_reviewed());
    d.select_id("f1.rs::charge_more");
    d.keys("a");
    assert!(
        d.app.all_reviewed(),
        "nothing left but units accepted for you"
    );

    // Only ○ units: nothing to review.
    let d = open(
        review(vec![
            same("f0.rs", "format_me"),
            same("f1.rs", "rename_vat"),
        ]),
        Config::default(),
    );
    assert!(d.app.all_reviewed());

    // `u` on a unit accepted for you makes it pending: the user's to review.
    let mut d = open(mixed(), Config::default());
    d.keys("za");
    d.select_id("f0.rs::format_me");
    d.keys("u");
    let u = get(&d, "f0.rs::format_me");
    assert_eq!(u.decision, Decision::Pending);
    assert!(not_for_you(u));
    assert!(
        accepted_for_you(get(&d, "f1.rs::rename_vat")),
        "only that one"
    );
    assert_eq!(d.app.decision_counts(), (1, 0, 3));
    d.keys("za");
    assert!(
        listed(&d).contains(&"f0.rs::format_me".to_string()),
        "a pending unit stays in the queue: {:?}",
        listed(&d)
    );
}

// ---- P5, P6: marks become reasons; rows keep ⚠ only ------------------------

/// The first new line of each unit in a prompt, by id.
fn units_in(prompt: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut id = None;
    for line in prompt.lines() {
        if let Some(rest) = line.strip_prefix("### Unit ") {
            id = Some(rest.to_string());
        }
        if let (Some(uid), Some(r)) = (&id, line.strip_prefix("new lines: "))
            && let Some((a, _)) = r.split_once('-')
        {
            out.push((uid.clone(), a.parse().unwrap()));
            id = None;
        }
    }
    out
}

const COST: &str = "Charges the card again on retry.";
const DISPUTE: &str = "It calls charge_order twice.";
const GAP: &str = "Also counts the items.";

/// Describes every unit with one low-risk statement: `charge_order` is
/// expensive to undo (`$`), `Cart.add` does something the PR description
/// doesn't mention (`≠`). As the second model: disputes `new_helper` (`⚠`).
fn responder(prompt: &str, _schema: &Value) -> anyhow::Result<Value> {
    let second = prompt.starts_with(&SECOND_RULES[..40]);
    let units: Vec<Value> = units_in(prompt)
        .into_iter()
        .map(|(id, line)| {
            let ev = json!([{"side": "new", "line": line}]);
            let charge = id.ends_with("charge_order");
            if second {
                let dispute = id.ends_with("new_helper");
                json!({
                    "id": id,
                    "risk": "low",
                    "verdicts": [{
                        "n": 1,
                        "verdict": if dispute { "dispute" } else { "agree" },
                        "why": if dispute { DISPUTE } else { "Right." },
                        "evidence": ev
                    }],
                    "missed": []
                })
            } else {
                let mut item = json!({
                    "id": id,
                    "summary": "Changes it.",
                    "changes": [{"marker": "changed", "statement": "Does something new.", "risk": "low", "evidence": ev}],
                    "undo": {
                        "expensive": charge,
                        "why": if charge { COST } else { "" },
                        "evidence": if charge { ev.clone() } else { json!([]) }
                    }
                });
                if id.ends_with("add") {
                    item["unmentioned"] = json!({"text": GAP, "evidence": ev});
                }
                item
            }
        })
        .collect();
    Ok(json!({ "units": units }))
}

/// The Python fixture, described by [`responder`] with a second opinion and
/// a PR description to compare against.
async fn prepared() -> (FixtureRepo, Review) {
    isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD)
        .write("api.py", "def handle(req):\n    return charge_order(req)\n")
        .commit("init");
    repo.write("checkout.py", PY_NEW);
    let mut review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let brief = Brief {
        pr: Some((
            "Charge in one place".into(),
            "Moves charging into charge_order.".into(),
        )),
        commits: HashMap::new(),
        untrusted: false,
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    prepare(
        Arc::new(MockAdapter::new(responder)),
        SessionRef::fresh("mock", repo.path()),
        review.to_prepare(None),
        2,
        true,
        Arc::new(brief),
        tx,
    )
    .await;
    while let Ok(ev) = rx.try_recv() {
        review.apply(ev);
    }
    (repo, review)
}

/// The id of the unit whose id ends with `name`.
fn id_of(d: &Driver, name: &str) -> String {
    d.app
        .review
        .units
        .iter()
        .find(|u| u.id.ends_with(name))
        .unwrap_or_else(|| panic!("a unit {name}"))
        .id
        .clone()
}

#[tokio::test]
async fn marks_become_reasons() {
    let (_repo, review) = prepared().await;
    let mut d = open(review, Config::default());
    let has = |id: &str, f: &dyn Fn(&str) -> bool| {
        let r = reasons(get(&d, id));
        assert!(r.iter().any(|s| f(s)), "{id}: {r:?}");
    };

    // `$`: "Expensive to undo: <why>".
    let charge = id_of(&d, "charge_order");
    has(&charge, &|s| {
        s.to_lowercase().starts_with("expensive to undo") && s.contains(COST.trim_end_matches('.'))
    });
    // `⚠`: the second model's words, quoted.
    let helper = id_of(&d, "new_helper");
    has(&helper, &|s| {
        s.to_lowercase().contains("second model disputes")
            && s.contains(DISPUTE.trim_end_matches('.'))
            && (s.contains('"') || s.contains('“'))
    });
    // `≠`: "Not mentioned in the commits or PR description".
    let add = id_of(&d, "add");
    has(&add, &|s| {
        s.to_lowercase()
            .starts_with("not mentioned in the commits or pr description")
    });
    // Each only where it applies.
    let none = |id: &str, word: &str| {
        let r = reasons(get(&d, id));
        assert!(
            !r.iter().any(|s| s.to_lowercase().contains(word)),
            "{id}: {r:?}"
        );
    };
    none(&charge, "dispute");
    none(&charge, "not mentioned");
    none(&helper, "expensive");
    none(&add, "expensive");
    none(&add, "dispute");

    // The unit's reasons show with it.
    for (id, text) in [
        (&charge, "expensive to undo"),
        (&helper, "it calls charge_order twice"),
        (&add, "not mentioned in the commits or pr description"),
    ] {
        d.select_id(id);
        let screen = d.render(160, 50);
        assert!(screen.to_lowercase().contains(text), "{id}: {screen}");
    }
}

#[tokio::test]
async fn queue_rows_show_only_dispute_glyph() {
    let (_repo, review) = prepared().await;
    let mut d = open(review, Config::default());
    // The preview, below the list at this width, shows a unit with no marks.
    let plain = id_of(&d, "old_helper");
    d.select_id(&plain);
    let screen = d.render(100, 60);
    assert!(!screen.contains('$'), "no $ in the queue: {screen}");
    assert!(!screen.contains('≠'), "no ≠ in the queue: {screen}");
    let row = |name: &str| {
        screen
            .lines()
            .find(|l| l.contains(name))
            .unwrap_or_else(|| panic!("a row for {name}: {screen}"))
            .to_string()
    };
    assert!(row("new_helper").contains('⚠'), "{screen}");
    assert!(!row("charge_order").contains('⚠'), "{screen}");
}

// ---- P7: queue order and the first unit ------------------------------------

#[test]
fn queue_order_and_first_unit() {
    // a.rs: a_callee ● (line 1), a_caller ● (line 50, calls a_callee),
    // a_look ◆ (line 100); b.rs: b_top ▲ (accepted already), b_low ●;
    // c.rs: c_top ▲.
    let mut callee = changed("a.rs", "a_callee");
    callee.new_range = Some(Range::new(1, 5));
    callee.callers = vec!["a.rs::a_caller".into()];
    let mut caller = changed("a.rs", "a_caller");
    caller.new_range = Some(Range::new(50, 60));
    caller.callees = vec!["a.rs::a_callee".into()];
    let mut look = expensive("a.rs", "a_look");
    look.new_range = Some(Range::new(100, 110));
    let mut b_top = removed_but_called("b.rs", "b_top");
    b_top.decision = Decision::Accepted;
    let units = vec![
        callee,
        caller,
        look,
        b_top,
        changed("b.rs", "b_low"),
        removed_but_called("c.rs", "c_top"),
    ];
    let mut d = open(review(units), Config::default());

    // The first unit shown is the first pending ▲.
    assert_eq!(d.selected().as_deref(), Some("c.rs::c_top"));

    // Level, then file, callers before callees, then path.
    let by_levels = [
        "b.rs::b_top",
        "c.rs::c_top",
        "a.rs::a_look",
        "a.rs::a_caller",
        "a.rs::a_callee",
        "b.rs::b_low",
    ];
    by_level(&mut d);
    assert_eq!(listed(&d), by_levels);
    let levels: Vec<Level> = listed(&d).iter().map(|id| level(&d, id)).collect();
    use Level::*;
    assert_eq!(
        levels,
        [Attention, Attention, Look, Routine, Routine, Routine]
    );
    assert_eq!(
        d.selected().as_deref(),
        Some("c.rs::c_top"),
        "the selection stays"
    );

    // `zf`: each file in the box of its most critical unit, and back.
    d.keys("zf");
    assert_eq!(
        listed(&d),
        [
            "b.rs::b_top",
            "b.rs::b_low",
            "c.rs::c_top",
            "a.rs::a_look",
            "a.rs::a_caller",
            "a.rs::a_callee",
        ]
    );
    d.keys("zf");
    assert_eq!(listed(&d), by_levels);
}

// ---- P8: --json ------------------------------------------------------------

#[test]
fn json_has_reasons_and_old_fields() {
    let sb = Sandbox::new();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD)
        .write(
            "db/migrations/add_flag.py",
            "def up(db):\n    db.add_column(\"orders\", \"flag\")\n",
        )
        .commit("init");
    repo.write("checkout.py", PY_NEW).write(
        "db/migrations/add_flag.py",
        "def up(db):\n    db.add_column(\"orders\", \"flag\", default=False)\n",
    );
    let v = sb.json(&repo.path(), &["--json", "--no-agent"]);

    // The old fields stay.
    for k in [
        "id",
        "repo_root",
        "base",
        "head",
        "head_commit",
        "created_at",
        "files",
        "units",
    ] {
        assert!(v.get(k).is_some(), "{k}");
    }
    let units = v["units"].as_array().unwrap();
    for u in units {
        for k in [
            "id",
            "file",
            "symbol",
            "kind",
            "status",
            "old_range",
            "new_range",
            "fingerprint",
            "changes",
            "prep",
            "callers",
            "callees",
            "decision",
        ] {
            assert!(u.get(k).is_some(), "{k} in {u}");
        }
    }

    // `accepted_by`: auto on every ○ unit, on no other.
    let by = |u: &Value| u["accepted_by"].as_str().map(str::to_lowercase);
    let same: Vec<&Value> = units
        .iter()
        .filter(|u| u["status"] == "NoBehaviorChange")
        .collect();
    assert!(same.iter().any(|u| u["symbol"] == "format_me"), "{same:?}");
    for u in &same {
        assert_eq!(u["decision"], "Accepted", "{u}");
        assert_eq!(by(u).as_deref(), Some("auto"), "{u}");
        assert!(u["nbc_reason"].is_string(), "{u}");
    }
    for u in units.iter().filter(|u| u["status"] != "NoBehaviorChange") {
        assert_eq!(u["decision"], "Pending", "{u}");
        assert_ne!(by(u).as_deref(), Some("auto"), "{u}");
    }

    // `reasons`: the migration is expensive to undo, found from the path.
    let up = units
        .iter()
        .find(|u| u["symbol"] == "up")
        .expect("the migration's unit");
    let reasons: Vec<String> = up["reasons"]
        .as_array()
        .unwrap_or_else(|| panic!("reasons in {up}"))
        .iter()
        .map(|r| {
            r.as_str()
                .or_else(|| r["text"].as_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(
        reasons
            .iter()
            .any(|r| r.to_lowercase().starts_with("expensive to undo")),
        "{reasons:?}"
    );
}
