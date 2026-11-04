//! The questions a unit view's tiles ask beyond what prepare wrote: the
//! advice and the leader tools (Tests, Behavior, Visualize), their keys,
//! names, and whether one may be asked now.

use super::app::App;
use crate::keymap::Action;
use crate::review::model::{DiveKind, Unit};

/// Each tool: its kind, the key that opens its panel, its name, and what
/// it brings to light, in the order the ask panel suggests them.
pub const TOOLS: [(DiveKind, Action, &str, &str); 3] = [
    (
        DiveKind::Tests,
        Action::ToolTests,
        "Tests",
        "the tests that reach it, found at once; then the agent may read them",
    ),
    (
        DiveKind::Behavior,
        Action::ToolRunOldNew,
        "Behavior",
        "runs the old and the new code on the same inputs",
    ),
    (
        DiveKind::Visualize,
        Action::ToolDiagram,
        "Visualize",
        "a picture of what it talks to and what changes",
    ),
];

/// May the agent be asked for advice on `u`: there is an agent, and it
/// hasn't answered and isn't answering.
pub fn may_advise(app: &App, u: &Unit) -> bool {
    let kind = DiveKind::Recommend;
    app.can_ask() && !u.dive.results.contains_key(&kind) && !pending(u, kind)
}

/// May the agent read what teer's test search found for `u`: there is an
/// agent, the search is done, and the agent hasn't read it or isn't
/// reading it.
pub fn may_read_tests(app: &App, u: &Unit) -> bool {
    let kind = DiveKind::Tests;
    let searched = u
        .dive
        .results
        .get(&kind)
        .and_then(|r| r.coverage.as_ref())
        .is_some_and(|c| c.search.is_some() && !c.judged);
    app.can_ask() && searched && !pending(u, kind)
}

/// The tool `a` opens.
pub fn tool_kind(a: Action) -> Option<DiveKind> {
    TOOLS.iter().find(|t| t.1 == a).map(|t| t.0)
}

/// The key that opens tool `kind`; empty for the questions no key asks.
pub fn tool_key(app: &App, kind: DiveKind) -> String {
    TOOLS
        .iter()
        .find(|t| t.0 == kind)
        .map_or(String::new(), |t| app.keys.label(t.1))
}

/// Tool `kind`'s name and what it brings to light.
pub fn light(kind: DiveKind) -> (&'static str, &'static str) {
    TOOLS
        .iter()
        .find(|t| t.0 == kind)
        .map_or(("", ""), |t| (t.2, t.3))
}

/// Is `kind`'s answer on its way? The second opinion keeps its own flag.
pub fn pending(u: &Unit, kind: DiveKind) -> bool {
    match kind {
        DiveKind::Disagree => u.checks.second_pending,
        _ => u.dive_pending.contains(&kind),
    }
}
