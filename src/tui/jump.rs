//! The jump list: `/` finds a unit by file or name, `gd` lists what the
//! unit calls and `gr` what calls it, and `enter` goes to the one picked.

use std::collections::HashSet;

use crossterm::event::KeyEvent;

use super::app::{App, Effect, JumpKind, Overlay, typed};
use crate::keymap::{Action, Edit};

/// Longest query, in characters.
const QUERY_MAX: usize = 200;

impl App {
    /// Open the jump list of `kind`. A list that would be empty doesn't
    /// open; the status line says why.
    pub fn open_jump(&mut self, kind: JumpKind) -> Vec<Effect> {
        if self.items_for(&kind, "").is_empty() {
            self.status = self.nothing_to_jump_to(&kind);
            return Vec::new();
        }
        self.overlay = Overlay::Jump {
            kind,
            query: String::new(),
            sel: 0,
        };
        Vec::new()
    }

    /// The units the jump list shows, by index into `review.units`, best
    /// match first.
    pub fn jump_items(&self) -> Vec<usize> {
        match &self.overlay {
            Overlay::Jump { kind, query, .. } => self.items_for(kind, query),
            _ => Vec::new(),
        }
    }

    /// A key while the jump list is open. Finding, the query is a text
    /// input ([`crate::keymap::Keymap::edit`]) and `↑ ↓` move the
    /// selection; the `gd` and `gr` lists are panels
    /// ([`App::panel_action`]): `j`/`k`/`gg`/`G` move, `view.open` goes,
    /// `view.back` or `view.close` closes.
    pub(super) fn jump_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let Overlay::Jump {
            kind,
            mut query,
            sel,
        } = self.overlay.clone()
        else {
            return Vec::new();
        };
        let items = self.items_for(&kind, &query);
        let last = items.len().saturating_sub(1);
        let mut sel = sel.min(last);
        if kind == JumpKind::Find {
            match self.keys.edit(&k) {
                Edit::Submit => {
                    // Nothing matches: nothing to go to, the query stays.
                    let Some(&i) = items.get(sel) else {
                        return Vec::new();
                    };
                    return self.jump_to(i);
                }
                Edit::Cancel => {
                    self.overlay = Overlay::None;
                    return Vec::new();
                }
                Edit::Down => sel = (sel + 1).min(last),
                Edit::Up => sel = sel.saturating_sub(1),
                e => {
                    if typed(&mut query, e, QUERY_MAX) {
                        let n = self.items_for(&kind, &query).len();
                        sel = sel.min(n.saturating_sub(1));
                    }
                }
            }
        } else {
            let Some(a) = self.panel_action(&k) else {
                return Vec::new();
            };
            match a {
                Action::MoveDown => sel = (sel + 1).min(last),
                Action::MoveUp => sel = sel.saturating_sub(1),
                Action::MoveFirst => sel = 0,
                Action::MoveLast => sel = last,
                Action::ViewOpen => {
                    if let Some(&i) = items.get(sel) {
                        return self.jump_to(i);
                    }
                }
                Action::ViewBack | Action::ViewClose => {
                    self.overlay = Overlay::None;
                    return Vec::new();
                }
                _ => {}
            }
        }
        self.overlay = Overlay::Jump { kind, query, sel };
        Vec::new()
    }

    /// Close the list and go to unit `i` (an index into `review.units`):
    /// in the unit view when it is open, else in the queue.
    fn jump_to(&mut self, i: usize) -> Vec<Effect> {
        let id = self.review.units[i].id.clone();
        self.overlay = Overlay::None;
        self.go_to_unit(&id).unwrap_or_default()
    }

    fn items_for(&self, kind: &JumpKind, query: &str) -> Vec<usize> {
        match kind {
            JumpKind::Find => self.found(query),
            JumpKind::Definition { of } => self.linked(of, true),
            JumpKind::Callers { of } => self.linked(of, false),
        }
    }

    /// Listed units whose file and symbol match `query`, ignoring case:
    /// those that hold it as it is typed first, then those that hold its
    /// letters in order, each in list order. Every listed unit for an empty
    /// query.
    fn found(&self, query: &str) -> Vec<usize> {
        let query = query.to_lowercase();
        let mut ranked: Vec<(bool, usize)> = self
            .order
            .iter()
            .filter_map(|&i| {
                let u = &self.review.units[i];
                let text = format!("{} {}", u.file.display(), u.symbol).to_lowercase();
                if text.contains(&query) {
                    Some((false, i))
                } else if in_order(&query, &text) {
                    Some((true, i))
                } else {
                    None
                }
            })
            .collect();
        // Stable: list order within each kind of match.
        ranked.sort_by_key(|&(loose, _)| loose);
        ranked.into_iter().map(|(_, i)| i).collect()
    }

    /// Listed units unit `of` calls (`callees`), or that call it, in list
    /// order: from the units' own lists and the review's call graph.
    fn linked(&self, of: &str, callees: bool) -> Vec<usize> {
        let mut ids: HashSet<&str> = HashSet::new();
        if let Some(u) = self.review.unit(of) {
            let mine = if callees { &u.callees } else { &u.callers };
            ids.extend(mine.iter().map(String::as_str));
        }
        for u in &self.review.units {
            let theirs = if callees { &u.callers } else { &u.callees };
            if theirs.iter().any(|id| id == of) {
                ids.insert(&u.id);
            }
        }
        let g = &self.review.graph;
        for &(caller, callee) in &g.edges {
            let (from, to) = if callees {
                (caller, callee)
            } else {
                (callee, caller)
            };
            if g.nodes.get(from).is_some_and(|n| n.id == of)
                && let Some(n) = g.nodes.get(to)
            {
                ids.insert(&n.id);
            }
        }
        self.order
            .iter()
            .copied()
            .filter(|&i| {
                let id = self.review.units[i].id.as_str();
                id != of && ids.contains(id)
            })
            .collect()
    }

    /// Why the list of `kind` is empty, for the status line.
    fn nothing_to_jump_to(&self, kind: &JumpKind) -> String {
        let symbol = |of: &str| {
            self.review
                .unit(of)
                .map_or_else(|| of.to_string(), |u| u.symbol.clone())
        };
        match kind {
            JumpKind::Find => "Nothing is listed to find.".into(),
            JumpKind::Definition { of } => format!("{} calls nothing in this review.", symbol(of)),
            JumpKind::Callers { of } => format!("Nothing in this review calls {}.", symbol(of)),
        }
    }
}

/// Whether `text` holds the characters of `query` in order, gaps allowed.
fn in_order(query: &str, text: &str) -> bool {
    let mut rest = text.chars();
    query.chars().all(|q| rest.any(|c| c == q))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::Config;
    use crate::keymap::{Keymap, parse_keys};
    use crate::review::model::{Decision, Prep, Review, Unit, UnitKind, UnitStatus};
    use crate::tui::app::Screen;

    fn unit(file: &str, symbol: &str) -> Unit {
        Unit {
            id: format!("{file}::{symbol}"),
            file: PathBuf::from(file),
            symbol: symbol.into(),
            kind: UnitKind::Function,
            status: UnitStatus::Changed,
            old_range: None,
            new_range: None,
            renamed_from: None,
            fingerprint: String::new(),
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

    fn app(units: Vec<Unit>) -> App {
        let review = Review {
            id: "t".into(),
            repo_root: PathBuf::new(),
            base: String::new(),
            head: String::new(),
            head_commit: String::new(),
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
        };
        App::new(review, Config::default(), Keymap::default())
    }

    /// `app.rs::charge`, `app.rs::refund`, `cart.rs::add`,
    /// `checkout.rs::pay`; `charge` calls `pay`.
    fn shop() -> App {
        let mut charge = unit("app.rs", "charge");
        charge.callees = vec!["checkout.rs::pay".into()];
        let mut pay = unit("checkout.rs", "pay");
        pay.callers = vec!["app.rs::charge".into()];
        app(vec![
            charge,
            unit("app.rs", "refund"),
            unit("cart.rs", "add"),
            pay,
        ])
    }

    fn press(a: &mut App, keys: &str) -> Vec<Effect> {
        let mut out = Vec::new();
        for k in parse_keys(keys).unwrap() {
            out.extend(a.on_key(k.event()));
        }
        out
    }

    fn ids(a: &App, items: &[usize]) -> Vec<String> {
        items
            .iter()
            .map(|&i| a.review.units[i].id.clone())
            .collect()
    }

    fn selected(a: &App) -> String {
        a.selected_id().unwrap()
    }

    fn query(a: &mut App, q: &str) -> Vec<String> {
        if let Overlay::Jump { query, .. } = &mut a.overlay {
            *query = q.into();
        }
        ids(a, &a.jump_items())
    }

    #[test]
    fn find_ranks_substrings_before_letters_in_order() {
        let mut a = shop();
        a.open_jump(JumpKind::Find);
        let listed: Vec<String> = ids(&a, &a.order.clone());
        // Empty: every listed unit, in list order.
        assert_eq!(query(&mut a, ""), listed);
        assert_eq!(
            query(&mut a, "CART"),
            vec!["cart.rs::add".to_string()],
            "case is ignored"
        );
        // `ar` is in `charge` and `cart`, and only its letters in order
        // in `app.rs refund`; `checkout.rs pay` has no `r` after an `a`.
        let whole: Vec<String> = listed
            .iter()
            .filter(|id| ["app.rs::charge", "cart.rs::add"].contains(&id.as_str()))
            .cloned()
            .collect();
        let r = query(&mut a, "ar");
        assert_eq!(r[..2], whole[..], "{r:?}");
        assert_eq!(r[2..], ["app.rs::refund".to_string()], "{r:?}");
        assert_eq!(query(&mut a, "zzz"), Vec::<String>::new());
    }

    #[test]
    fn definition_and_callers_come_from_the_call_graph() {
        let mut a = shop();
        a.open_jump(JumpKind::Definition {
            of: "app.rs::charge".into(),
        });
        assert_eq!(ids(&a, &a.jump_items()), vec!["checkout.rs::pay"]);
        a.open_jump(JumpKind::Callers {
            of: "checkout.rs::pay".into(),
        });
        assert_eq!(ids(&a, &a.jump_items()), vec!["app.rs::charge"]);
        // Only one side recorded is enough: the graph's edges count too.
        let mut a = app(vec![unit("a.rs", "f"), unit("a.rs", "g")]);
        a.review.graph = crate::graph::GraphData {
            nodes: ["f", "g"]
                .iter()
                .map(|s| crate::graph::SymbolNode {
                    id: format!("a.rs::{s}"),
                    file: PathBuf::from("a.rs"),
                    symbol: s.to_string(),
                    kind: UnitKind::Function,
                    range: crate::review::model::Range::new(1, 2),
                    in_review: true,
                })
                .collect(),
            edges: vec![(0, 1)],
        };
        a.open_jump(JumpKind::Callers {
            of: "a.rs::g".into(),
        });
        assert_eq!(ids(&a, &a.jump_items()), vec!["a.rs::f"]);
    }

    #[test]
    fn an_empty_list_does_not_open_and_says_why() {
        let mut a = shop();
        a.open_jump(JumpKind::Definition {
            of: "cart.rs::add".into(),
        });
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.status, "add calls nothing in this review.");
        a.open_jump(JumpKind::Callers {
            of: "app.rs::charge".into(),
        });
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.status, "Nothing in this review calls charge.");
        let mut empty = app(Vec::new());
        empty.open_jump(JumpKind::Find);
        assert_eq!(empty.overlay, Overlay::None);
        assert!(!empty.status.is_empty());
    }

    #[test]
    fn typing_narrows_and_keeps_the_selection_on_the_list() {
        let mut a = shop();
        a.open_jump(JumpKind::Find);
        press(&mut a, "<down><down><down><down>");
        assert!(matches!(a.overlay, Overlay::Jump { sel: 3, .. }));
        // Three units hold a `p`: the selection stays on the last.
        press(&mut a, "p");
        assert!(matches!(a.overlay, Overlay::Jump { sel: 2, .. }));
        press(&mut a, "ay");
        assert!(matches!(a.overlay, Overlay::Jump { sel: 0, .. }));
        // `j` and `q` are text here; `<up>` stays at the top.
        press(&mut a, "jq<up>");
        assert!(matches!(&a.overlay, Overlay::Jump { query, sel: 0, .. } if query == "payjq"));
        assert!(a.jump_items().is_empty());
        // ⏎ with nothing found keeps the list open.
        press(&mut a, "<cr>");
        assert!(matches!(a.overlay, Overlay::Jump { .. }));
        press(&mut a, "<bs><bs><c-u>");
        assert!(matches!(&a.overlay, Overlay::Jump { query, .. } if query.is_empty()));
        press(&mut a, "<esc>");
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn enter_goes_to_the_unit_in_the_list() {
        let mut a = shop();
        a.sel = 0;
        a.open_jump(JumpKind::Find);
        press(&mut a, "refund<cr>");
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.screen, Screen::Queue, "in the queue, the queue");
        assert_eq!(selected(&a), "app.rs::refund");
        // gd: a panel; `j` moves, `l` goes.
        a.open_jump(JumpKind::Definition {
            of: "app.rs::charge".into(),
        });
        press(&mut a, "jl");
        assert_eq!(selected(&a), "checkout.rs::pay");
    }

    #[test]
    fn the_gd_list_closes_with_back_or_close() {
        for key in ["h", "<esc>", "q"] {
            let mut a = shop();
            a.open_jump(JumpKind::Definition {
                of: "app.rs::charge".into(),
            });
            let at = a.sel;
            press(&mut a, key);
            assert_eq!(a.overlay, Overlay::None, "{key}");
            assert_eq!(a.sel, at, "{key}");
        }
        // Keys other screens use do nothing here.
        let mut a = shop();
        a.open_jump(JumpKind::Definition {
            of: "app.rs::charge".into(),
        });
        press(&mut a, "a");
        assert!(matches!(a.overlay, Overlay::Jump { .. }));
        assert!(
            a.review
                .units
                .iter()
                .all(|u| u.decision == Decision::Pending)
        );
    }
}
