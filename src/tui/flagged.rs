//! The Flagged screen: every unit marked investigate, one at a time. What
//! the unit is, then tiles: the code change, one per answer in the order it
//! was asked for, the chat about the unit once opened, and one with the
//! questions that bring more light into it. Each tile scrolls on its own;
//! the one in focus can fill the screen.
//! (The agent questions keep the `Dive*` names: `DiveKind`, `DiveEvent`.)
//! Everything the agent wrote is untrusted and drawn as plain text.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Pane, Target, pane_key, tile_kinds};
use super::preview::{scroll_rows, unit_explain, unit_head};
use super::slim;
use crate::config::{Action, Theme as ThemeName};
use crate::review::model::{
    Behavior, ClaimCheck, Coverage, Delta, DiveKind, Expect, FoundTest, Marker, Outcome, ProbeCase,
    Reach, Resolution, Risk, TestCheck, TestSearch, Unit, UnitStatus, Visual,
};
use crate::review::visual::{DARK, LIGHT, edge_line};
use crate::util::{truncate, wrap};

pub const ASKING: &str = "asking the agent…";
pub const PROBING: &str =
    "the agent writes a probe, then teer runs it before and after the change…";
pub const CHECKING: &str = "a second model checks the description…";
pub const SEARCHING: &str = "searching the test files…";

/// Each question: its kind, its name, the name when space is short, its
/// key, and what it brings to light.
const TABS: [(DiveKind, &str, &str, Action, &str); 5] = [
    (
        DiveKind::Recommend,
        "Recommend",
        "Rec",
        Action::DiveRecommend,
        "what to do about it, most important first",
    ),
    (
        DiveKind::Visualize,
        "Visualize",
        "Vis",
        Action::DiveVisualize,
        "a picture of what it talks to and what changes",
    ),
    (
        DiveKind::Tests,
        "Tests",
        "Tests",
        Action::DiveTests,
        "the tests that reach it, found at once; the agent can read them",
    ),
    (
        DiveKind::Behavior,
        "Behavior",
        "Run",
        Action::DiveBehavior,
        "runs the old and the new code on the same inputs",
    ),
    (
        DiveKind::Disagree,
        "Disagreement",
        "Disagree",
        Action::DiveDisagree,
        "a second model checks the description",
    ),
];

/// The first row of the tile that asks for more.
pub const MENU: &str = "Not sure yet? Bring more light into it:";

/// What the chat brings to light, in the tile that asks for more.
const CHAT: &str = "ask about it in your own words; the conversation is kept";

/// What can still be asked about `u`: each question not asked yet (its
/// kind, name, key and what it brings to light), then the chat while it
/// has no tile.
fn left_to_ask(app: &App, u: &Unit) -> Vec<(Option<DiveKind>, &'static str, Action, &'static str)> {
    let asked = tile_kinds(u);
    let mut left: Vec<_> = TABS
        .iter()
        .filter(|q| !asked.contains(&q.0))
        .map(|q| (Some(q.0), q.1, q.3, q.4))
        .collect();
    if !app.flagged_panes(u).contains(&Pane::Chat) {
        left.push((None, "Chat", Action::Ask, CHAT));
    }
    left
}

/// `kind`'s name.
fn name_of(kind: DiveKind) -> &'static str {
    TABS.iter().find(|t| t.0 == kind).map_or("", |t| t.1)
}

/// Where `kind` stands for `u`: on its way (the spinner), answered with
/// doubts (`⚠`), answered (`✓`); `None` when never asked.
fn state(app: &App, u: &Unit, kind: DiveKind) -> Option<(String, Style)> {
    let t = &app.theme;
    let doubts =
        kind == DiveKind::Disagree && app.assessment(&u.id).is_some_and(|a| !a.doubts.is_empty());
    if pending(u, kind) {
        Some((app.spinner().to_string(), t.muted()))
    } else if doubts {
        Some(("⚠".into(), t.risk(Risk::Medium)))
    } else if u.dive.results.contains_key(&kind)
        || (kind == DiveKind::Disagree && u.checks.second.is_some())
    {
        Some(("✓".into(), t.ok()))
    } else {
        None
    }
}

/// `kind`'s key.
fn key_of(app: &App, kind: DiveKind) -> String {
    TABS.iter()
        .find(|q| q.0 == kind)
        .map_or(String::new(), |q| app.keys.label(q.3))
}

/// What the tab says while its answer is on its way: the latest progress
/// when there is one.
fn waiting(app: &App, u: &Unit, kind: DiveKind) -> String {
    if let Some(p) = app.progress.get(&(u.id.clone(), kind)) {
        return p.clone();
    }
    match kind {
        DiveKind::Behavior => PROBING,
        DiveKind::Disagree => CHECKING,
        DiveKind::Tests if !u.dive.results.contains_key(&kind) => SEARCHING,
        _ => ASKING,
    }
    .to_string()
}

/// Who wrote the answer on `kind`'s tab. Questions here run on Claude
/// Code's default model (no `--model`); the second opinion on `second_model`;
/// the Tests search is teer's own until the agent reads it.
fn answer_label(app: &App, u: &Unit, kind: DiveKind) -> String {
    let agent = slim::model_name("default");
    let tests = u.dive.results.get(&kind).and_then(|r| r.coverage.as_ref());
    match kind {
        DiveKind::Tests if tests.is_some_and(|c| !c.judged) => "found by teer".into(),
        DiveKind::Tests if tests.is_some_and(|c| c.search.is_some()) => {
            format!("found by teer, read by the agent ({agent})")
        }
        DiveKind::Disagree => format!(
            "second opinion ({})",
            slim::model_name(&app.cfg.second_model)
        ),
        DiveKind::Behavior => format!("probe by the agent ({agent}), run by teer"),
        _ => format!("agent ({agent})"),
    }
}

/// Is `kind`'s answer on its way? The second opinion keeps its own flag.
fn pending(u: &Unit, kind: DiveKind) -> bool {
    match kind {
        DiveKind::Disagree => u.checks.second_pending,
        _ => u.dive_pending.contains(&kind),
    }
}

fn glyph(app: &App, u: &Unit) -> Span<'static> {
    let t = &app.theme;
    match u.dive.resolution {
        Some(Resolution::Ok) => Span::styled("●", t.ok()),
        Some(Resolution::NeedsFix) => Span::styled("✗", t.needs_fix()),
        None => Span::styled("⚑", t.badge_investigate()),
    }
}

/// The questions not asked yet and the chat, each with its key, name and
/// what it brings to light; on one row where there is room, else the text
/// under the name.
fn more_lines(app: &App, u: &Unit, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines: Vec<Line<'static>> = wrap(MENU, w)
        .into_iter()
        .map(|l| Line::styled(format!(" {l}"), t.bold()))
        .collect();
    lines.push(Line::default());
    let left = left_to_ask(app, u);
    if left.is_empty() {
        let again = format!(
            "Every question is asked. {} asks the one in focus again.",
            app.keys.label(Action::DiveRerun)
        );
        lines.extend(
            wrap(&again, w)
                .into_iter()
                .map(|l| Line::styled(format!(" {l}"), t.muted())),
        );
        return lines;
    }
    let key_w = left
        .iter()
        .map(|q| app.keys.label(q.2).width())
        .max()
        .unwrap_or(1);
    let name_w = left.iter().map(|q| q.1.width() + 2).max().unwrap_or(0);
    let head = 1 + key_w + 2 + name_w + 2;
    let one_row = w >= head + 24;
    for (kind, name, action, what) in left {
        let key = app.keys.label(action);
        let mut spans = vec![
            Span::styled(format!(" {key:>key_w$}  "), t.accent()),
            Span::styled(name.to_string(), t.bold()),
        ];
        // The second opinion can be there unasked.
        let mut used = name.width();
        if let Some((glyph, style)) = kind.and_then(|k| state(app, u, k)) {
            spans.push(Span::styled(format!(" {glyph}"), style));
            used += 2;
        }
        if one_row {
            spans.push(Span::raw(" ".repeat(name_w + 2 - used)));
            spans.push(Span::styled(
                truncate(what, w.saturating_sub(head)),
                t.muted(),
            ));
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(spans));
            let pad = " ".repeat(key_w + 3);
            for l in wrap(what, w.saturating_sub(key_w + 3)) {
                lines.push(Line::styled(format!("{pad}{l}"), t.muted()));
            }
        }
    }
    lines
}

fn delta_style(app: &App, d: Delta) -> Style {
    let t = &app.theme;
    match d {
        Delta::Added => t.marker(Marker::Added),
        Delta::Changed => t.marker(Marker::Changed),
        Delta::Removed => t.marker(Marker::Removed),
        Delta::Unchanged => t.muted(),
    }
}

/// A Visualize answer: the claim, one line per edge, where the page is.
fn visual_lines(app: &App, v: &Visual, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines: Vec<Line<'static>> = wrap(&v.claim, w)
        .into_iter()
        .map(|l| Line::raw(format!(" {l}")))
        .collect();
    lines.push(Line::default());
    for e in &v.edges {
        let tag = format!("[{}]", e.status.word());
        let text = truncate(&edge_line(v, e), w.saturating_sub(tag.width() + 3));
        lines.push(Line::from(vec![
            Span::raw(format!(" {text}  ")),
            Span::styled(tag, delta_style(app, e.status)),
        ]));
    }
    lines.push(Line::default());
    let saved = format!("saved to {}", v.path.display());
    for l in wrap(&saved, w) {
        lines.push(Line::styled(format!(" {l}"), t.muted()));
    }
    lines.push(Line::styled(
        format!(
            " {} opens it in the browser",
            app.keys.label(Action::OpenVisual)
        ),
        t.muted(),
    ));
    lines
}

fn outcome_text(o: &Outcome) -> String {
    match o {
        Outcome::Value(v) => v.clone(),
        Outcome::Error(e) => format!("error: {e}"),
        Outcome::Missing => "— (not printed)".into(),
    }
}

/// A line of `text` after a marker, wrapped and indented under it.
fn marked(glyph: &str, text: &str, w: usize, style: Style) -> Vec<Line<'static>> {
    wrap(text, w.saturating_sub(4))
        .into_iter()
        .enumerate()
        .map(|(k, l)| {
            let lead = if k == 0 {
                format!(" {glyph} ")
            } else {
                "   ".into()
            };
            Line::styled(format!("{lead}{l}"), style)
        })
        .collect()
}

/// A Behavior answer: the summary; the statements checked against the runs;
/// each case (surprises first, then the ones that changed, then the same);
/// whether the project's tests notice the change; what ran.
fn behavior_lines(app: &App, u: &Unit, b: &Behavior, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let new_unit = u.status == UnitStatus::Added;
    let summary = if new_unit {
        let n = b.cases.len();
        format!(
            "{n} case{} ran on the new code (it doesn't exist before the change).",
            if n == 1 { "" } else { "s" }
        )
    } else {
        b.summary()
    };
    let mut lines: Vec<Line<'static>> = wrap(&summary, w)
        .into_iter()
        .map(|l| Line::raw(format!(" {l}")))
        .collect();
    lines.push(Line::default());
    let claims = b.claim_checks();
    if !claims.is_empty() && !new_unit {
        lines.push(Line::styled(
            " The statements, checked by running the code",
            t.bold(),
        ));
        for (text, check) in claims {
            let (glyph, style, result) = match check {
                ClaimCheck::Holds(n) => (
                    "✓",
                    t.ok(),
                    format!("{n} case{} agree", if n == 1 { "" } else { "s" }),
                ),
                ClaimCheck::Fails { wrong, of } => (
                    "✗",
                    t.needs_fix(),
                    format!("{wrong} of {of} cases disagree"),
                ),
                ClaimCheck::Untested => ("?", t.muted(), "no case tests it".to_string()),
            };
            lines.extend(marked(glyph, &format!("{text} ({result})"), w, style));
        }
        lines.push(Line::default());
    }
    let mut cases: Vec<&ProbeCase> = b.cases.iter().collect();
    cases.sort_by_key(|c| (!c.surprise(), !c.changed()));
    let name_w = cases
        .iter()
        .map(|c| c.name.width())
        .max()
        .unwrap_or(0)
        .min(18);
    for c in cases {
        let (glyph, style) = match (c.surprise() && !new_unit, c.changed()) {
            _ if new_unit => ("•", t.muted()),
            (true, _) => ("!", t.needs_fix()),
            (false, true) => ("≠", t.marker(Marker::Changed)),
            (false, false) => ("=", t.muted()),
        };
        let name = crate::util::truncate_pad(&c.name, name_w);
        let room = w.saturating_sub(name_w + 6);
        let mut head = vec![
            Span::styled(format!(" {glyph} "), style),
            Span::styled(name, t.bold()),
            Span::raw("  "),
            Span::styled(truncate(&c.input, room), t.muted()),
        ];
        if !c.changed() || new_unit {
            let used: usize = head.iter().map(|s| s.content.width()).sum();
            let value = format!("  {}", outcome_text(&c.after));
            head.push(Span::raw(truncate(&value, w.saturating_sub(used))));
        }
        lines.push(Line::from(head));
        let pad = " ".repeat(5);
        if c.changed() && !new_unit {
            for (side, o) in [("before", &c.before), ("after ", &c.after)] {
                let text = truncate(&outcome_text(o), w.saturating_sub(13));
                lines.push(Line::from(vec![
                    Span::styled(format!("{pad}{side}  "), t.muted()),
                    Span::raw(text),
                ]));
            }
        }
        if c.surprise() && !new_unit {
            let predicted = match (c.claim > 0, c.expect) {
                (true, Expect::Same) => "its statement implies no change",
                (true, Expect::Different) => "its statement implies a change",
                (false, Expect::Same) => "the agent predicted no change",
                (false, Expect::Different) => "the agent predicted a change",
            };
            let text = format!("{predicted}: {}", c.why);
            for l in wrap(&text, w.saturating_sub(5)) {
                lines.push(Line::styled(format!("{pad}{l}"), t.needs_fix()));
            }
        }
    }
    if let Some(tests) = &b.tests {
        lines.push(Line::default());
        let (glyph, style, text) = match tests {
            TestCheck::Guarded => (
                "✓",
                t.ok(),
                "The project's tests pass with the change and fail with this unit put back: they notice it."
                    .to_string(),
            ),
            TestCheck::Unguarded => (
                "⚠",
                t.risk(Risk::Medium),
                "The project's tests pass with or without this change: no test notices it."
                    .to_string(),
            ),
            TestCheck::FailWithChange(status) => (
                "✗",
                t.needs_fix(),
                format!("The project's tests already fail with the change applied ({status})."),
            ),
            TestCheck::NoTests(why) => ("·", t.muted(), format!("No existing test: {why}")),
            TestCheck::NotRun(why) => ("·", t.muted(), format!("Tests not run: {why}")),
        };
        lines.extend(marked(glyph, &text, w, style));
        if !b.test_command.is_empty() {
            let cmd = format!("`{}`", b.test_command.join(" "));
            for l in wrap(&cmd, w.saturating_sub(4)) {
                lines.push(Line::styled(format!("   {l}"), t.muted()));
            }
        }
    }
    lines.push(Line::default());
    let ran = format!(
        "ran `{}` in a sandbox: no network, nothing from the home folder",
        b.command.join(" ")
    );
    for l in wrap(&ran, w) {
        lines.push(Line::styled(format!(" {l}"), t.muted()));
    }
    for l in wrap(&format!("probe and raw output: {}", b.dir.display()), w) {
        lines.push(Line::styled(format!(" {l}"), t.muted()));
    }
    lines
}

fn cite_text(c: &crate::review::model::Cite) -> String {
    format!("{}:{}-{}", c.file.display(), c.start, c.end)
}

/// How a found test reaches the unit, in a few words.
fn reach_words(r: &Reach) -> String {
    match r {
        Reach::Calls { line } => format!("calls it · line {line}"),
        Reach::Through { via, hops: 1, line } => format!("via {via} · line {line}"),
        Reach::Through { via, hops, line } => {
            format!("via {via}, {hops} calls away · line {line}")
        }
        Reach::Names => "names it".into(),
    }
}

/// A test teer found: its name, then how it reaches the unit and where.
fn found_lines(app: &App, f: &FoundTest, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines = marked("·", &f.name, w, Style::default());
    let how = format!("{} · {}", reach_words(&f.reach), cite_text(&f.cite));
    for l in wrap(&how, w.saturating_sub(4)) {
        lines.push(Line::styled(format!("   {l}"), t.muted()));
    }
    lines
}

/// The Tests tile before the agent has read anything: what teer's search
/// found, nearest first, and the key that has the agent read it.
fn found_only(app: &App, s: &TestSearch, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let head = match s.total {
        0 => "No test calls it, calls a caller, or names it.".to_string(),
        1 => "1 test reaches it".to_string(),
        n => format!("{n} tests reach it"),
    };
    let mut lines: Vec<Line<'static>> = wrap(&head, w.saturating_sub(1))
        .into_iter()
        .map(|l| Line::styled(format!(" {l}"), t.bold()))
        .collect();
    for f in &s.tests {
        lines.extend(found_lines(app, f, w));
    }
    if s.total > s.tests.len() {
        lines.push(Line::styled(
            format!("   and {} more, farther from it", s.total - s.tests.len()),
            t.muted(),
        ));
    }
    lines.push(Line::default());
    let key = app.keys.label(Action::InterpretTests);
    let what = if s.total == 0 {
        "the agent looks for tests teer can't see, and the risky cases none covers"
    } else {
        "the agent reads them: which really cover it, and the risky cases they miss"
    };
    let lead = key.width() + 3;
    for (i, l) in wrap(what, w.saturating_sub(lead)).into_iter().enumerate() {
        let k = if i == 0 {
            key.clone()
        } else {
            " ".repeat(key.width())
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {k}  "), t.accent()),
            Span::styled(l, t.muted()),
        ]));
    }
    lines
}

/// The Tests tile. Before the agent has read anything, what teer's search
/// found ([`found_only`]). After: the tests that cover the unit, what each
/// checks and where; the dangerous cases no test covers, most dangerous
/// first; the agent's note; the other tests the search found.
fn coverage_lines(app: &App, note: &str, c: &Coverage, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    if !c.judged {
        return found_only(app, c.search.as_ref().unwrap_or(&TestSearch::default()), w);
    }
    let mut lines = Vec::new();
    let n = c.covered.len();
    let head = match n {
        0 => "No existing test covers it.".to_string(),
        1 => "1 test covers it".to_string(),
        n => format!("{n} tests cover it"),
    };
    lines.push(Line::styled(format!(" {head}"), t.bold()));
    let read = c.search.as_ref().map(|s| match s.total {
        0 => "teer found no test that reaches it; the agent looked.".to_string(),
        1 => "The agent read the 1 test teer found.".to_string(),
        n => format!("The agent read the {n} tests teer found."),
    });
    for l in read.iter().flat_map(|r| wrap(r, w.saturating_sub(1))) {
        lines.push(Line::styled(format!(" {l}"), t.muted()));
    }
    for test in &c.covered {
        lines.extend(marked("✓", &test.name, w, t.ok()));
        let what = format!("{} · {}", test.checks, cite_text(&test.cite));
        for l in wrap(&what, w.saturating_sub(4)) {
            lines.push(Line::styled(format!("   {l}"), t.muted()));
        }
    }
    lines.push(Line::default());
    if c.gaps.is_empty() {
        lines.push(Line::styled(
            " No dangerous case found that the tests miss.",
            t.muted(),
        ));
    } else {
        lines.push(Line::styled(" Dangerous cases no test covers", t.bold()));
        for g in &c.gaps {
            let word = format!("{:?}", g.risk).to_lowercase();
            let style = t.risk(g.risk);
            lines.extend(marked("!", &format!("{} ({word} risk)", g.case), w, style));
            let at: Vec<String> = g.evidence.iter().map(cite_text).collect();
            let why = format!("{} · {}", g.danger, at.join(", "));
            for l in wrap(&why, w.saturating_sub(4)) {
                lines.push(Line::styled(format!("   {l}"), t.muted()));
            }
        }
    }
    if !note.is_empty() {
        lines.push(Line::default());
        for l in wrap(note, w) {
            lines.push(Line::styled(format!(" {l}"), t.muted()));
        }
    }
    let rest: Vec<&FoundTest> = c
        .search
        .iter()
        .flat_map(|s| &s.tests)
        .filter(|f| !c.covered.iter().any(|x| x.cite == f.cite))
        .collect();
    if !rest.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(
            " Also reach it, not counted as covering it",
            t.bold(),
        ));
        for f in rest {
            lines.extend(found_lines(app, f, w));
        }
    }
    lines
}

/// The second opinion: its risk against the description's, then each
/// statement it disputes or agrees with, then what it says was missed.
fn disagree_lines(app: &App, u: &Unit, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines = Vec::new();
    let Some(o) = &u.checks.second else {
        if let Some(e) = &u.checks.second_failed {
            lines.extend(marked(
                "·",
                &format!("No second opinion: {e}"),
                w,
                t.muted(),
            ));
        }
        return lines;
    };
    let word = |r: Risk| format!("{r:?}").to_lowercase();
    let first = u.highest_risk().map(word).unwrap_or_else(|| "none".into());
    let doubts = o.disagreements(&u.changes);
    let head = if doubts.is_empty() {
        format!(
            "A second model agrees with the description. Its risk: {}, the description's: {first}.",
            word(o.risk)
        )
    } else {
        format!(
            "A second model disagrees on {} point{}. Its risk: {}, the description's: {first}.",
            doubts.len(),
            if doubts.len() == 1 { "" } else { "s" },
            word(o.risk)
        )
    };
    for l in wrap(&head, w) {
        lines.push(Line::raw(format!(" {l}")));
    }
    lines.push(Line::default());
    let mut verdicts: Vec<_> = o.verdicts.iter().collect();
    verdicts.sort_by_key(|v| v.agree);
    for v in verdicts {
        let (glyph, style) = if v.agree {
            ("✓", t.ok())
        } else {
            ("✗", t.needs_fix())
        };
        lines.extend(marked(glyph, &format!("\"{}\"", v.statement), w, style));
        let why = format!("{} ({})", v.why, slim::cite_text(&v.evidence));
        for l in wrap(&why, w.saturating_sub(4)) {
            lines.push(Line::styled(format!("   {l}"), t.muted()));
        }
    }
    for m in &o.missed {
        lines.extend(marked(
            "+",
            &format!(
                "missed [{} risk]: {} ({})",
                word(m.risk),
                m.statement,
                slim::cite_text(&m.evidence)
            ),
            w,
            if m.risk >= Risk::Medium {
                t.risk(Risk::Medium)
            } else {
                t.muted()
            },
        ));
    }
    lines
}

/// Rows the picture needs at least; with fewer the text list is shown.
const PICTURE_MIN_ROWS: usize = 4;

/// What `kind`'s part of the report says: its answer, that it is on its
/// way, or that there is none.
fn section_body(app: &App, u: &Unit, kind: DiveKind, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines = Vec::new();
    if kind == DiveKind::Disagree {
        lines = disagree_lines(app, u, w);
    } else if let Some(res) = u.dive.results.get(&kind) {
        let drawn = res
            .visual
            .as_ref()
            .map(|v| visual_lines(app, v, w))
            .or_else(|| res.behavior.as_ref().map(|b| behavior_lines(app, u, b, w)))
            .or_else(|| {
                res.coverage
                    .as_ref()
                    .map(|c| coverage_lines(app, &res.text, c, w))
            });
        if let Some(drawn) = drawn {
            lines = drawn;
        } else {
            for l in wrap(&res.text, w) {
                lines.push(Line::raw(format!(" {l}")));
            }
            if !res.sources.is_empty() {
                lines.push(Line::default());
                lines.push(Line::styled(" Sources", t.bold()));
                for s in &res.sources {
                    for (k, l) in wrap(s, w.saturating_sub(2)).into_iter().enumerate() {
                        let lead = if k == 0 { " - " } else { "   " };
                        lines.push(Line::styled(format!("{lead}{l}"), t.muted()));
                    }
                }
            }
        }
    }
    if pending(u, kind) {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        let wait = waiting(app, u, kind);
        for (k, l) in wrap(&wait, w.saturating_sub(2)).into_iter().enumerate() {
            let lead = if k == 0 { app.spinner() } else { " " };
            lines.push(Line::styled(format!(" {lead} {l}"), t.muted()));
        }
    }
    if lines.is_empty() {
        let key = key_of(app, kind);
        lines.push(Line::styled(
            format!(" No answer. {key} asks again."),
            t.muted(),
        ));
    }
    lines
}

/// Draw `u`'s Visualize picture in `area` (a tile's inside): the claim, the
/// picture, and how to open the page. False to show the text instead.
fn draw_picture(f: &mut Frame, app: &App, u: &Unit, area: Rect, w: usize) -> bool {
    let Some(pictures) = &app.pictures else {
        return false;
    };
    let Some(v) = u
        .dive
        .results
        .get(&DiveKind::Visualize)
        .and_then(|r| r.visual.as_ref())
    else {
        return false;
    };
    if pending(u, DiveKind::Visualize) {
        return false;
    }
    let claim: Vec<Line<'static>> = wrap(&v.claim, w)
        .into_iter()
        .map(|l| Line::raw(format!(" {l}")))
        .collect();
    let claim_h = claim.len() as u16;
    if (area.height as usize) < claim.len() + 1 + PICTURE_MIN_ROWS + 1 || area.width < 4 {
        return false;
    }
    let [top, _, pic, foot] = Layout::vertical([
        Constraint::Length(claim_h),
        Constraint::Length(1),
        Constraint::Min(PICTURE_MIN_ROWS as u16),
        Constraint::Length(1),
    ])
    .areas(area);
    // One column in, like the text.
    let pic = Rect {
        x: pic.x + 1,
        width: pic.width - 1,
        ..pic
    };
    let palette = if app.theme.name == ThemeName::Light {
        &LIGHT
    } else {
        &DARK
    };
    f.render_widget(Clear, pic);
    if !pictures.draw(f, pic, v, palette) {
        return false;
    }
    f.render_widget(Paragraph::new(claim), top);
    let open = format!(
        "{} opens it in the browser · {}",
        app.keys.label(Action::OpenVisual),
        v.path.display()
    );
    f.render_widget(
        Paragraph::new(Line::styled(
            format!(" {}", truncate(&open, w)),
            app.theme.muted(),
        )),
        foot,
    );
    true
}

/// A tile `width` wide's title: the key that asks for it and its name, or
/// what it holds.
fn tile_title(app: &App, u: &Unit, pane: Pane, width: usize) -> Line<'static> {
    let t = &app.theme;
    match pane {
        Pane::Code => Line::styled(crate::tui::diff::TITLE, t.bold()),
        Pane::Answer(kind) => {
            let mut spans = vec![
                Span::styled(format!(" {}", key_of(app, kind)), t.accent()),
                Span::styled(format!(" {} ", name_of(kind)), t.bold()),
            ];
            if let Some((glyph, style)) = state(app, u, kind) {
                spans.push(Span::styled(format!("{glyph} "), style));
            }
            Line::from(spans)
        }
        Pane::Chat => {
            let mut spans = vec![
                Span::styled(format!(" {}", app.keys.label(Action::Ask)), t.accent()),
                Span::styled(" Chat ", t.bold()),
            ];
            let chat = app.chat_of(&u.id);
            if chat.iter().any(|n| app.ask.pending.contains(n)) {
                spans.push(Span::styled(format!("{} ", app.spinner()), t.muted()));
            }
            Line::from(spans)
        }
        // The keys that ask for another tile, named where they fit.
        Pane::More => {
            let left = left_to_ask(app, u);
            if left.is_empty() {
                return Line::styled(" more light ", t.bold());
            }
            let named = left
                .iter()
                .map(|q| app.keys.label(q.2).width() + q.1.width() + 4)
                .sum::<usize>();
            let mut spans = vec![Span::styled(" +", t.bold())];
            for (i, q) in left.iter().enumerate() {
                if named + 4 <= width {
                    if i > 0 {
                        spans.push(Span::styled(" ·", t.muted()));
                    }
                    spans.push(Span::styled(
                        format!(" {}", app.keys.label(q.2)),
                        t.accent(),
                    ));
                    spans.push(Span::styled(format!(" {}", q.1), t.bold()));
                } else {
                    spans.push(Span::styled(
                        format!(" {}", app.keys.label(q.2)),
                        t.accent(),
                    ));
                }
            }
            spans.push(Span::raw(" "));
            Line::from(spans)
        }
    }
}

/// One tile: its border (in the accent while in focus), its title, and
/// what it holds, scrolled to where it was left.
fn draw_tile(f: &mut Frame, app: &App, u: &Unit, pane: Pane, area: Rect, focus: bool) {
    let t = &app.theme;
    if area.width < 8 || area.height < 3 {
        return;
    }
    app.targets.borrow_mut().push((area, Target::Tile(pane)));
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(if focus { t.accent() } else { t.muted() })
        .title(tile_title(app, u, pane, area.width as usize));
    let mut blocks = Vec::new();
    // The code sits a column in from the border on both sides; the answers
    // indent themselves.
    let mut inner = block.inner(area);
    let mut gutter = 0;
    match pane {
        Pane::Code => {
            inner = Rect::new(
                inner.x + 1,
                inner.y,
                inner.width.saturating_sub(2),
                inner.height,
            );
            let (code, added, removed, g) = crate::tui::diff::code_change(
                app,
                u,
                inner.width as usize,
                crate::tui::diff::Notes::Inline,
            );
            blocks = code;
            gutter = g;
            block = block
                .title(Line::from(crate::tui::diff::counts(app, added, removed)).right_aligned());
        }
        Pane::Answer(kind) => {
            let answered = u.dive.results.contains_key(&kind)
                || (kind == DiveKind::Disagree && u.checks.second.is_some());
            if answered {
                let label = truncate(
                    &answer_label(app, u, kind),
                    (area.width as usize).saturating_sub(6),
                );
                block = block
                    .title_bottom(Line::styled(format!(" {label} "), t.muted()).right_aligned());
            }
        }
        Pane::Chat => {
            let keys = super::ask::chat_keys(app, focus);
            block = block.title_bottom(Line::styled(keys, t.muted()).right_aligned());
        }
        Pane::More => {}
    }
    let w = (inner.width as usize).saturating_sub(2);
    f.render_widget(block, area);
    super::ui::text_area(app, inner, gutter);
    let h = inner.height as usize;
    let key = pane_key(&u.id, pane);
    if focus {
        app.detail_max_scroll.set(0);
        app.detail_rows.set(h);
    }
    match pane {
        Pane::Chat => return super::ask::draw_chat(f, app, u, inner, focus),
        Pane::Answer(DiveKind::Visualize) if draw_picture(f, app, u, inner, w) => return,
        Pane::Answer(kind) => {
            blocks = section_body(app, u, kind, w)
                .into_iter()
                .map(|l| vec![l])
                .collect();
        }
        Pane::More => blocks = more_lines(app, u, w).into_iter().map(|l| vec![l]).collect(),
        Pane::Code => {}
    }
    let at = app.pane_scroll.borrow().get(&key).copied().unwrap_or(0);
    let indent = usize::from(pane != Pane::Code);
    let s = scroll_rows(app, blocks, w.max(1), h, indent, pane == Pane::Code, at);
    if s.offset != at {
        app.pane_scroll.borrow_mut().insert(key, s.offset);
    }
    if focus {
        app.detail_max_scroll.set(s.max);
    }
    f.render_widget(Paragraph::new(s.lines), inner);
    // A click on a question left to ask asks it.
    if pane == Pane::More {
        for (_, name, action, _) in left_to_ask(app, u) {
            let shown = format!("{}  {name}", app.keys.label(action));
            if let Some(r) = super::select::find(f.buffer_mut(), inner, &shown) {
                let row = Rect::new(inner.x, r.y, inner.width, 1);
                app.targets.borrow_mut().push((row, Target::Action(action)));
            }
        }
    }
}

/// Where each tile goes in `area`. The code change takes a tall column on
/// the left (on top when narrow); the others tile the rest, two to a row
/// where there is room, a last one alone as wide as the row. The chat has
/// a row to itself, twice as tall as the others. Alone on the last row
/// under answers, the tile that asks for more is as tall as `more_h` says
/// it needs at its width.
fn tile_areas(panes: &[Pane], area: Rect, more_h: impl Fn(u16) -> u16) -> Vec<(Pane, Rect)> {
    let mut out = Vec::new();
    let (code, rest): (Vec<Pane>, Vec<Pane>) = panes.iter().partition(|p| **p == Pane::Code);
    let mut rest_area = area;
    if !code.is_empty() {
        // Before anything is asked the code gets more of the room.
        let share = if rest.len() <= 1 { 3 } else { 2 };
        if area.width >= 90 {
            let [c, r] =
                Layout::horizontal([Constraint::Ratio(share, 5), Constraint::Ratio(5 - share, 5)])
                    .areas(area);
            out.push((Pane::Code, c));
            rest_area = r;
        } else {
            let [c, r] =
                Layout::vertical([Constraint::Ratio(share, 5), Constraint::Ratio(5 - share, 5)])
                    .areas(area);
            out.push((Pane::Code, c));
            rest_area = r;
        }
    }
    if rest.is_empty() {
        return out;
    }
    let cols = if rest.len() == 1 || rest_area.width < 50 {
        1
    } else {
        2
    };
    let mut rows: Vec<Vec<Pane>> = Vec::new();
    for p in rest {
        match rows.last_mut() {
            Some(row) if row.len() < cols && p != Pane::Chat && row[0] != Pane::Chat => row.push(p),
            _ => rows.push(vec![p]),
        }
    }
    let mut heights: Vec<Constraint> = rows
        .iter()
        .map(|r| Constraint::Fill(if r[0] == Pane::Chat { 2 } else { 1 }))
        .collect();
    let n = rows.len();
    if n > 1 && cols > 1 && rows[n - 1] == [Pane::More] {
        heights[n - 1] = Constraint::Length(more_h(rest_area.width).min(rest_area.height / 2));
    }
    let row_areas = Layout::vertical(heights).split(rest_area);
    for (r, chunk) in rows.iter().enumerate() {
        let cells = Layout::horizontal(vec![Constraint::Ratio(1, chunk.len() as u32); chunk.len()])
            .split(row_areas[r]);
        for (p, c) in chunk.iter().zip(cells.iter()) {
            out.push((*p, *c));
        }
    }
    out
}

/// How the unit was settled, and the note.
fn foot_line(app: &App, u: &Unit, w: usize) -> Line<'static> {
    let t = &app.theme;
    let res = match u.dive.resolution {
        Some(Resolution::Ok) => Span::styled("● resolved: ok", t.ok()),
        Some(Resolution::NeedsFix) => Span::styled("✗ resolved: needs fix", t.needs_fix()),
        None => Span::styled(
            format!(
                "⚑ unresolved ({} ok · {} needs fix)",
                app.keys.label(Action::ResolveOk),
                app.keys.label(Action::ResolveFix)
            ),
            t.muted(),
        ),
    };
    let used = res.content.width() + 12;
    let note = if u.dive.note.trim().is_empty() {
        Span::styled(
            format!("press {} to add a note", app.keys.label(Action::EditNote)),
            t.muted(),
        )
    } else {
        Span::raw(truncate(&u.dive.note, w.saturating_sub(used)))
    };
    Line::from(vec![
        Span::raw(" "),
        res,
        Span::styled("   note: ", t.muted()),
        note,
    ])
}

/// Why the unit matters, the checks and what changed; a blank row; the
/// tiles; how it was settled.
fn draw_detail(f: &mut Frame, app: &App, u: &Unit, area: Rect) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    app.targets.borrow_mut().push((area, Target::Detail));
    super::ui::text_area(app, area, 0);
    app.detail_max_scroll.set(0);
    let w = (area.width as usize).saturating_sub(2);
    let panes = app.flagged_panes(u);
    // The statements are in the code; without it they are listed here.
    let mut top = unit_explain(app, u, w, !panes.contains(&Pane::Code));
    if area.height >= 20 && !top.is_empty() {
        top.push(Line::default());
    }
    let foot_h = u16::from(area.height >= 8);
    // What the unit is takes at most a quarter of the height.
    let top_h = (top.len() as u16).min(area.height / 4);
    let [top_area, grid, foot] = Layout::vertical([
        Constraint::Length(top_h),
        Constraint::Min(0),
        Constraint::Length(foot_h),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(super::preview::cut(app, top, top_h as usize)),
        top_area,
    );
    let focus = app.flagged_focus(u);
    // Zoomed, or no room for more than one: the tile in focus alone.
    let alone = app.flagged.zoom || grid.width < 40 || grid.height < 8;
    if alone {
        draw_tile(f, app, u, focus, grid, true);
    } else {
        // Its rows at that width, and the borders.
        let more_h = |width: u16| {
            let w = (width as usize).saturating_sub(4);
            (more_lines(app, u, w).len() + 2).min(u16::MAX as usize) as u16
        };
        for (pane, r) in tile_areas(&panes, grid, more_h) {
            draw_tile(f, app, u, pane, r, pane == focus);
        }
    }
    if foot_h > 0 {
        f.render_widget(Paragraph::new(foot_line(app, u, w)), foot);
    }
}

/// The unit in a header row (where it stands, its name and place, which
/// of the flagged units it is), the rest below.
pub fn draw_flagged(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let flagged = app.flagged_units();
    if flagged.is_empty() {
        f.render_widget(
            Paragraph::new(Line::styled(
                format!(
                    " Nothing is flagged. Press {} to go back.",
                    app.keys.label(Action::Back)
                ),
                t.muted(),
            )),
            area,
        );
        return;
    }
    let sel = app.flagged.sel.min(flagged.len() - 1);
    let u = flagged[sel];
    let [head, _, detail] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(area.height >= 20)),
        Constraint::Min(0),
    ])
    .areas(area);
    let count = if flagged.len() > 1 {
        format!(
            " {}/{} · {} {} other units ",
            sel + 1,
            flagged.len(),
            app.keys.label(Action::Down),
            app.keys.label(Action::Up)
        )
    } else {
        String::new()
    };
    let cw = count.width();
    let w = (head.width as usize).saturating_sub(cw + 3);
    let mut line = unit_head(app, u, false, w);
    line.spans.insert(0, glyph(app, u));
    line.spans.insert(0, Span::raw(" "));
    let used = line.width();
    line.spans.push(Span::raw(
        " ".repeat((head.width as usize).saturating_sub(used + cw)),
    ));
    line.spans.push(Span::styled(count, t.muted()));
    f.render_widget(Paragraph::new(line), head);
    draw_detail(f, app, u, detail);
}
