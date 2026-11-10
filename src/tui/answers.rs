//! The agent's answers about a unit, as the unit view shows them: the
//! advice, the second model, and the tools' panels (the tests, old
//! against new, the diagram, the security findings). (The agent
//! questions keep the `Dive*` names: `DiveKind`, `DiveEvent`.) Everything
//! the agent wrote is untrusted and drawn as plain text.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::app::App;
use crate::config::Theme as ThemeName;
use crate::keymap::Action;
use crate::review::model::{
    Behavior, Cite, ClaimCheck, Coverage, Delta, DiveKind, Expect, FoundTest, Marker, Outcome,
    ProbeCase, Reach, Risk, Side, TestCheck, TestSearch, Unit, UnitStatus, Visual,
};
use crate::review::visual::{DARK, LIGHT, edge_line};
use crate::security::{Audit, Fact, Finding, Origin, Proof, Weight, audit};
use crate::session::names;
use crate::session::tools::{self, may_read_tests, pending, tool_key};
use crate::util::{truncate, wrap};

/// The advice tile's name.
pub(super) const ADVICE: &str = "Advice";
/// The advice tile's question, after its key, until the agent is asked.
pub(super) const ASK_ADVICE: &str = "ask the agent what to do about it";

/// The Tests tile's question, after its key, when teer found tests.
pub(super) const READ_TESTS: &str = "the agent reads them";
/// The same, when teer found none.
pub(super) const LOOK_TESTS: &str = "the agent looks for tests itself";

/// The first row of what brings more light into a unit.
pub(super) const MENU: &str = "Not sure yet? Bring more light into it:";

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
            app.keys.label(Action::GotoBrowser)
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
            (false, true) => (Marker::Changed.symbol(), t.marker(Marker::Changed)),
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

/// The Tests panel before the agent has read anything: what teer's search
/// found, nearest first. With `ask`, under the count, the question whether
/// the agent reads them: what each checks and the dangerous cases none
/// covers (or, with none found, whether it looks for tests itself).
fn found_only(app: &App, s: &TestSearch, w: usize, ask: bool) -> Vec<Line<'static>> {
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
    if ask {
        let key = app.keys.labels(Action::ViewOpen);
        let what = match s.total {
            0 => LOOK_TESTS.to_string(),
            _ => format!("{READ_TESTS}: what each checks, and what none covers"),
        };
        let room = w.saturating_sub(key.width() + 2);
        for (k, l) in wrap(&what, room).into_iter().enumerate() {
            let lead = if k == 0 {
                Span::styled(format!(" {key} "), t.accent())
            } else {
                Span::raw(" ".repeat(key.width() + 2))
            };
            lines.push(Line::from(vec![lead, Span::styled(l, t.bold())]));
        }
        lines.push(Line::default());
    }
    for f in &s.tests {
        lines.extend(found_lines(app, f, w));
    }
    if s.total > s.tests.len() {
        lines.push(Line::styled(
            format!("   and {} more, farther from it", s.total - s.tests.len()),
            t.muted(),
        ));
    }
    lines
}

/// The Tests panel. Before the agent has read anything, what teer's search
/// found ([`found_only`], asking with `ask`). After: the tests that cover
/// the unit, what each checks and where; the dangerous cases no test
/// covers, most dangerous first; the agent's note; the other tests the
/// search found.
fn coverage_lines(app: &App, note: &str, c: &Coverage, w: usize, ask: bool) -> Vec<Line<'static>> {
    let t = &app.theme;
    if !c.judged {
        let none = TestSearch::default();
        return found_only(app, c.search.as_ref().unwrap_or(&none), w, ask);
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
pub(super) fn disagree_lines(app: &App, u: &Unit, w: usize) -> Vec<Line<'static>> {
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
        let why = format!("{} ({})", v.why, names::cite_text(&v.evidence));
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
                names::cite_text(&m.evidence)
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

/// The Security tile's audit of `u`, when there is one.
pub(super) fn audit_of(u: &Unit) -> Option<&Audit> {
    u.dive
        .results
        .get(&DiveKind::Security)
        .and_then(|r| r.audit.as_ref())
}

/// Is something more on its way about `u`'s findings: the second model's
/// check, or their proof.
pub(super) fn security_busy(u: &Unit) -> bool {
    audit_of(u).is_some_and(|a| a.checking || a.proving)
}

/// May `<space>p` prove `u`'s findings: there are some, and they aren't
/// being proven.
pub(super) fn may_prove(u: &Unit) -> bool {
    audit_of(u).is_some_and(|a| !a.findings.is_empty() && !a.proving)
}

/// How a severity shows: the level it would raise the unit to.
fn severity_glyph(r: Risk) -> &'static str {
    match r {
        Risk::High => "▲",
        Risk::Medium => "◆",
        Risk::Low => "●",
    }
}

/// Where a cite is, short: `AssetController.php:41`, `x.go:3-9`.
fn place(c: &Cite) -> String {
    let name = c.file.file_name().map_or_else(
        || c.file.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    if c.end > c.start {
        format!("{name}:{}-{}", c.start, c.end)
    } else {
        format!("{name}:{}", c.start)
    }
}

/// `lead` and `what` on the left of a row `w` cells wide, `at` on its
/// right (muted); `what` is cut to make room. Without `what`, `at` follows
/// the lead.
fn located(app: &App, lead: &str, what: &str, style: Style, at: &str, w: usize) -> Line<'static> {
    let t = &app.theme;
    let mut spans = vec![Span::styled(lead.to_string(), style)];
    if what.is_empty() {
        spans.push(Span::styled(
            truncate(at, w.saturating_sub(lead.width())),
            t.muted(),
        ));
        return Line::from(spans);
    }
    let room = w.saturating_sub(lead.width() + at.width() + 2);
    let what = truncate(what, room.max(8));
    let pad = w.saturating_sub(lead.width() + what.width() + at.width());
    spans.push(Span::styled(what, style));
    spans.push(Span::raw(" ".repeat(pad.max(2))));
    spans.push(Span::styled(at.to_string(), t.muted()));
    Line::from(spans)
}

/// `text` wrapped under a three-cell indent.
fn indented(text: &str, w: usize, style: Style) -> Vec<Line<'static>> {
    wrap(text, w.saturating_sub(4))
        .into_iter()
        .map(|l| Line::styled(format!("   {l}"), style))
        .collect()
}

/// A spinner line saying what is on its way.
fn spinning(app: &App, text: &str, w: usize) -> Vec<Line<'static>> {
    wrap(text, w.saturating_sub(4))
        .into_iter()
        .enumerate()
        .map(|(k, l)| {
            let lead = if k == 0 { app.spinner() } else { " " };
            Line::styled(format!(" {lead} {l}"), app.theme.muted())
        })
        .collect()
}

/// A fact teer found: a glyph by its weight, the fact, its class and where.
fn fact_lines(app: &App, u: &Unit, f: &Fact, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let (glyph, style) = match f.weight {
        Weight::Attention => ("▲", t.risk(Risk::High)),
        Weight::Look => ("◆", t.risk(Risk::Medium)),
        Weight::Note => ("·", t.muted()),
    };
    let mut lines = marked(glyph, &f.text, w, style);
    let mut at = vec![f.class.word().to_string()];
    if !f.subject.is_empty() {
        at.push(f.subject.clone());
    }
    if let Some(r) = f.lines {
        let side = if f.side == Side::Old { " (old)" } else { "" };
        let name = u
            .file
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into());
        at.push(format!("{name}:{}{side}", r.start));
    }
    if let Origin::Import { tool, rule } = &f.origin {
        at.push(format!("{tool} {rule}"));
    }
    lines.extend(indented(&at.join(" · "), w, t.muted()));
    lines
}

/// One finding: a glyph by severity and its title (severity · class);
/// where the input enters, each call, where it does harm; what's missing;
/// the exploit; whether the path is verified; the second model's word;
/// and what proving it showed.
fn finding_lines(app: &App, f: &Finding, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let style = if f.stands() {
        t.risk(f.severity)
    } else {
        t.muted()
    };
    let title = format!("{} ({} · {})", f.title, f.severity.word(), f.class.word());
    let mut lines: Vec<Line<'static>> = wrap(&title, w.saturating_sub(4))
        .into_iter()
        .enumerate()
        .map(|(k, l)| {
            let lead = if k == 0 {
                Span::styled(format!(" {} ", severity_glyph(f.severity)), style)
            } else {
                Span::raw("   ")
            };
            Line::from(vec![lead, Span::styled(l, t.bold())])
        })
        .collect();
    let g = &app.review.graph;
    let name = |c: &Cite| audit::symbol_at(g, c).unwrap_or("").to_string();
    let plain = Style::default();
    lines.push(located(
        app,
        "   source  ",
        &name(&f.source),
        plain,
        &place(&f.source),
        w,
    ));
    for h in &f.path {
        lines.push(located(app, "   → ", &h.symbol, plain, &place(&h.cite), w));
    }
    lines.push(located(
        app,
        "   sink    ",
        &name(&f.sink),
        plain,
        &place(&f.sink),
        w,
    ));
    if !f.missing.is_empty() {
        for (k, l) in wrap(&f.missing, w.saturating_sub(6))
            .into_iter()
            .enumerate()
        {
            let lead = if k == 0 { "   ✗ " } else { "     " };
            lines.push(Line::styled(format!("{lead}{l}"), t.needs_fix()));
        }
    }
    if !f.exploit.is_empty() {
        lines.extend(indented(&f.exploit, w, t.muted()));
    }
    if !f.path_verified {
        lines.extend(indented(
            "path not verified: teer's call graph doesn't confirm every call",
            w,
            t.risk(Risk::Medium),
        ));
    }
    match &f.second {
        Some(s) if s.agree => lines.push(Line::styled("   second model agrees", t.ok())),
        Some(s) => {
            let at = s
                .evidence
                .as_ref()
                .map(|c| format!(" ({})", place(c)))
                .unwrap_or_default();
            let text = format!("⚠ second model disputes: {}{at}", s.why);
            lines.extend(indented(&text, w, t.needs_fix()));
        }
        None => {}
    }
    match &f.proof {
        Some(Proof::Proven {
            case,
            before,
            after,
        }) => {
            lines.extend(indented(&format!("proven: {case}"), w, t.needs_fix()));
            lines.extend(indented(
                &format!("before: {before} · after: {after}"),
                w,
                t.muted(),
            ));
        }
        Some(Proof::NotShown {
            case,
            before,
            after,
        }) => {
            lines.extend(indented(&format!("not shown: {case}"), w, t.muted()));
            lines.extend(indented(
                &format!("before: {before} · after: {after}"),
                w,
                t.muted(),
            ));
        }
        Some(Proof::Unproven { why }) => {
            lines.extend(indented(&format!("unproven: {why}"), w, t.muted()));
        }
        None => {}
    }
    lines
}

/// The agent's audit: the head, its summary, each finding (those that
/// stand first, most severe first), what is on its way, the controls in
/// place, then what was checked and what not, and what was dropped.
fn audit_lines(app: &App, a: &Audit, note: &str, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines = vec![Line::styled(format!(" {}", audit::head(a)), t.bold())];
    for l in wrap(note, w.saturating_sub(1)) {
        lines.push(Line::raw(format!(" {l}")));
    }
    let mut findings: Vec<&Finding> = a.findings.iter().collect();
    findings.sort_by_key(|f| (!f.stands(), std::cmp::Reverse(f.severity)));
    for f in findings {
        lines.push(Line::default());
        lines.extend(finding_lines(app, f, w));
    }
    if a.checking || a.proving {
        lines.push(Line::default());
    }
    if a.checking {
        lines.extend(spinning(app, audit::CHECKING, w));
    }
    if a.proving {
        lines.extend(spinning(app, audit::PROVING, w));
    }
    if !a.holds.is_empty() {
        lines.push(Line::default());
        for h in &a.holds {
            lines.push(located(app, " ✓ ", &h.what, t.ok(), &place(&h.cite), w));
        }
    }
    lines.push(Line::default());
    let checked: Vec<&str> = a.checked.iter().map(|c| c.word()).collect();
    let mut footer = vec![format!("checked: {}", checked.join(" · "))];
    if !a.not_checked.is_empty() {
        footer.push(format!("not: {}", a.not_checked.join(" · ")));
    }
    for l in footer.iter().flat_map(|f| wrap(f, w.saturating_sub(1))) {
        lines.push(Line::styled(format!(" {l}"), t.muted()));
    }
    if a.dropped > 0 {
        let n = a.dropped;
        let text = format!(
            "{n} finding{} dropped: {} lines didn't check out (teer.log says which)",
            if n == 1 { "" } else { "s" },
            if n == 1 { "its" } else { "their" }
        );
        for l in wrap(&text, w.saturating_sub(1)) {
            lines.push(Line::styled(format!(" {l}"), t.muted()));
        }
    }
    lines
}

/// The Security tile: what teer found in the change (its facts, there
/// before the agent answers, and without one), then the agent's audit, or
/// that it is on its way, or that findings need an agent.
fn security_lines(app: &App, u: &Unit, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let kind = DiveKind::Security;
    let mut lines = Vec::new();
    if !u.security.is_empty() {
        lines.push(Line::styled(" teer found", t.bold()));
        for f in &u.security {
            lines.extend(fact_lines(app, u, f, w));
        }
        lines.push(Line::default());
    }
    let result = u.dive.results.get(&kind);
    match result.and_then(|r| r.audit.as_ref()) {
        Some(a) => {
            let note = result.map_or("", |r| r.text.as_str());
            lines.extend(audit_lines(app, a, note, w));
        }
        None if pending(u, kind) => {}
        None if !app.can_ask() => lines.extend(marked("·", audit::NO_AGENT, w, t.muted())),
        None => {
            let key = tool_key(app, kind);
            lines.push(Line::styled(
                format!(" No answer. {key} asks again."),
                t.muted(),
            ));
        }
    }
    if pending(u, kind) {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.extend(spinning(app, &tools::waiting(app, u, kind), w));
    }
    lines
}

/// Rows the picture needs at least; with fewer the text list is shown.
const PICTURE_MIN_ROWS: usize = 4;

/// What the answer to `kind` says: the answer, that it is on its way, or
/// that there is none and how to ask again.
pub(super) fn section_body(app: &App, u: &Unit, kind: DiveKind, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut lines = Vec::new();
    if kind == DiveKind::Security {
        return security_lines(app, u, w);
    }
    if kind == DiveKind::Disagree {
        lines = disagree_lines(app, u, w);
    } else if let Some(res) = u.dive.results.get(&kind) {
        let drawn = res
            .visual
            .as_ref()
            .map(|v| visual_lines(app, v, w))
            .or_else(|| res.behavior.as_ref().map(|b| behavior_lines(app, u, b, w)))
            .or_else(|| {
                let ask = kind == DiveKind::Tests && may_read_tests(app, u);
                res.coverage
                    .as_ref()
                    .map(|c| coverage_lines(app, &res.text, c, w, ask))
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
        let wait = tools::waiting(app, u, kind);
        for (k, l) in wrap(&wait, w.saturating_sub(2)).into_iter().enumerate() {
            let lead = if k == 0 { app.spinner() } else { " " };
            lines.push(Line::styled(format!(" {lead} {l}"), t.muted()));
        }
    }
    if lines.is_empty() && kind == DiveKind::Recommend {
        // Nothing is asked before the reviewer says so.
        let key = app.keys.label(Action::ToolAdvice);
        let what = "concrete steps, most important first, each with its reason";
        let mut first = vec![
            Span::styled(format!(" {key} "), t.accent()),
            Span::styled(ASK_ADVICE, t.bold()),
        ];
        first.push(Span::styled(":", t.bold()));
        lines.push(Line::from(first));
        for l in wrap(what, w.saturating_sub(2)) {
            lines.push(Line::styled(format!(" {l}"), t.muted()));
        }
        return lines;
    }
    if lines.is_empty() {
        // A tool asks again with its key; the rest when the unit opens.
        let again = match tool_key(app, kind) {
            key if key.is_empty() => "Opening the unit again asks again.".to_string(),
            key => format!("{key} asks again."),
        };
        lines.push(Line::styled(format!(" No answer. {again}"), t.muted()));
    }
    lines
}

/// Draw `u`'s Visualize picture in `area` (a panel's inside): the claim,
/// the picture, and how to open the page. False to show the text instead.
pub(super) fn draw_picture(f: &mut Frame, app: &App, u: &Unit, area: Rect, w: usize) -> bool {
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
        app.keys.label(Action::GotoBrowser),
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
