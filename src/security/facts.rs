//! What teer finds in a change without an agent: an authorization check
//! removed or missing where its siblings have it, a new entry point,
//! untrusted input reaching a sink, a secret, a package change, a CI
//! workflow or security config loosened. Each becomes a [`Fact`] on its
//! unit, and facts raise levels (`attention::assess`).
//!
//! All of it is tree-sitter and string matching at build time, per unit:
//! the code's checks, entry points and sinks come from the catalog
//! ([`super::catalog`]); whether a unit is exposed comes from the call
//! graph ([`entry_reachable`]). Secrets, packages, CI and config are read
//! from added lines in every file, whole-file units included.

mod ci;
mod lines;
mod packages;
mod scan;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::catalog::{self, Catalog, SinkKind};
use super::config::SecurityConfig;
use super::model::{Class, Fact, Origin, Weight};
use crate::graph::SymbolNode;
use crate::review::model::{Range, Review, Side, Unit, UnitKind, UnitStatus};
use crate::units::lang::Lang;
use lines::Added;
use scan::{Event, Scan, What};

/// How many callers up an entry point is looked for.
const REACH_DEPTH: usize = 4;
/// Largest file read beyond the changed ones (as the call graph does).
const MAX_FILE_BYTES: usize = 512 * 1024;

/// Write each unit's security facts ([`crate::review::model::Unit::security`])
/// from the change and the call graph. With the lens off, none.
pub fn annotate(review: &mut Review, cfg: &SecurityConfig) {
    for u in &mut review.units {
        u.security.clear();
    }
    if !cfg.enabled {
        return;
    }
    let found: Vec<Vec<Fact>> = {
        let mut reach = Reach::new(review, cfg);
        review.units.iter().map(|u| reach.facts(u)).collect()
    };
    for (u, f) in review.units.iter_mut().zip(found) {
        u.security = f;
    }
}

/// The entry point `unit` is, or is called from within four callers:
/// its route or name (`POST /users`, `SyncCommand`). `None` when none is
/// found. Reads `security.toml` for the user's entry points; for many
/// units, use one [`Reach`].
pub fn entry_reachable(review: &Review, unit: &Unit) -> Option<String> {
    let cfg = SecurityConfig::load();
    Reach::new(review, &cfg).reachable(unit)
}

/// Finds entry points over a review: each file read and parsed once, and
/// each unit's answer kept.
pub struct Reach<'a> {
    review: &'a Review,
    cfg: &'a SecurityConfig,
    catalogs: HashMap<Lang, Rc<Catalog>>,
    scans: HashMap<(PathBuf, bool), Option<Rc<Scan>>>,
    nodes: HashMap<&'a str, usize>,
    units: HashMap<&'a str, usize>,
    callers_of: Vec<Vec<usize>>,
    reached: HashMap<String, Option<String>>,
    /// Per changed file: its old lines (trimmed), its new lines, and the
    /// unit each new line is in (the innermost).
    lines: HashMap<&'a Path, Rc<FileLines<'a>>>,
}

/// A changed file's lines, as the line rules read them.
struct FileLines<'a> {
    old: HashSet<&'a str>,
    new: Rc<Vec<&'a str>>,
    owner: Vec<&'a str>,
}

impl<'a> Reach<'a> {
    pub fn new(review: &'a Review, cfg: &'a SecurityConfig) -> Reach<'a> {
        let g = &review.graph;
        let nodes = g
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.as_str(), i))
            .collect();
        let units = review
            .units
            .iter()
            .enumerate()
            .map(|(i, u)| (u.id.as_str(), i))
            .collect();
        let mut callers_of = vec![Vec::new(); g.nodes.len()];
        for &(a, b) in &g.edges {
            if a < g.nodes.len() && b < g.nodes.len() {
                callers_of[b].push(a);
            }
        }
        Reach {
            review,
            cfg,
            catalogs: HashMap::new(),
            scans: HashMap::new(),
            nodes,
            units,
            callers_of,
            reached: HashMap::new(),
            lines: HashMap::new(),
        }
    }

    fn unit(&self, id: &str) -> Option<&'a Unit> {
        let review = self.review;
        self.units.get(id).map(|&i| &review.units[i])
    }

    /// The lines of changed file `path`, read once.
    fn file_lines(&mut self, path: &'a Path) -> Option<Rc<FileLines<'a>>> {
        if let Some(l) = self.lines.get(path) {
            return Some(l.clone());
        }
        let review = self.review;
        let f = review.files.iter().find(|f| f.path == path)?;
        let old = f
            .old_content
            .as_deref()
            .unwrap_or("")
            .lines()
            .map(str::trim)
            .collect();
        let new: Vec<&str> = f.new_content.as_deref().unwrap_or("").lines().collect();
        // Each line's unit: the smallest that holds it, painted over the
        // larger ones.
        let mut owner = vec![""; new.len()];
        let mut here: Vec<(&Range, &str)> = review
            .units
            .iter()
            .filter(|u| u.file == path && u.kind != UnitKind::File)
            .filter_map(|u| u.new_range.as_ref().map(|r| (r, u.symbol.as_str())))
            .collect();
        here.sort_by_key(|(r, _)| std::cmp::Reverse(r.len()));
        for (r, sym) in here {
            for l in r.start.max(1)..=r.end.min(new.len() as u32) {
                owner[l as usize - 1] = sym;
            }
        }
        let lines = Rc::new(FileLines {
            old,
            new: Rc::new(new),
            owner,
        });
        self.lines.insert(path, lines.clone());
        Some(lines)
    }

    /// The catalog for `lang`, with the user's names.
    pub fn catalog(&mut self, lang: Lang) -> Rc<Catalog> {
        let cfg = self.cfg;
        self.catalogs
            .entry(lang)
            .or_insert_with(|| Rc::new(catalog::for_lang(lang, cfg)))
            .clone()
    }

    /// One side of a file, read: from the review for a changed file, else
    /// from the repository (the new side only).
    fn scan(&mut self, path: &Path, new: bool) -> Option<Rc<Scan>> {
        let key = (path.to_path_buf(), new);
        if let Some(s) = self.scans.get(&key) {
            return s.clone();
        }
        // Only languages with a catalog have checks, entry points or sinks
        // to find.
        let lang = Lang::from_path(path).filter(|l| {
            matches!(
                l,
                Lang::Php | Lang::Go | Lang::JavaScript | Lang::TypeScript | Lang::Tsx
            )
        });
        if lang.is_none() {
            self.scans.insert(key, None);
            return None;
        }
        let text: Option<String> = match self.review.files.iter().find(|f| f.path == path) {
            Some(f) if new => f.new_content.clone(),
            Some(f) => f.old_content.clone(),
            None if new => crate::git::read_text_file(&self.review.repo_root, path, MAX_FILE_BYTES),
            None => None,
        };
        let scanned = match (lang, text) {
            (Some(lang), Some(text)) => {
                let cat = self.catalog(lang);
                scan::scan(lang, path, &text, &cat).map(Rc::new)
            }
            _ => None,
        };
        self.scans.insert(key, scanned.clone());
        scanned
    }

    /// The entry point `unit` is itself (or the first route it
    /// registers).
    pub fn entry_of(&mut self, unit: &Unit) -> Option<String> {
        if unit.kind == UnitKind::File {
            return None;
        }
        let new = unit.status != UnitStatus::Removed;
        let s = self.scan(&unit.file, new)?;
        s.entry_name(s.sym(&unit.symbol)?)
    }

    fn node_entry(&mut self, node: &SymbolNode) -> Option<String> {
        if let Some(u) = self.unit(&node.id) {
            return self.entry_of(u);
        }
        let s = self.scan(&node.file, true)?;
        s.entry_name(s.sym(&node.symbol)?)
    }

    /// The entry point `unit` is, or the nearest one among its callers,
    /// up to four calls away.
    pub fn reachable(&mut self, unit: &Unit) -> Option<String> {
        if let Some(r) = self.reached.get(&unit.id) {
            return r.clone();
        }
        let r = self.find_reachable(unit);
        self.reached.insert(unit.id.clone(), r.clone());
        r
    }

    fn find_reachable(&mut self, unit: &Unit) -> Option<String> {
        if let Some(e) = self.entry_of(unit) {
            return Some(e);
        }
        let review = self.review;
        let start = *self.nodes.get(unit.id.as_str())?;
        let mut seen: HashSet<usize> = HashSet::from([start]);
        let mut frontier = vec![start];
        for _ in 0..REACH_DEPTH {
            let mut next = Vec::new();
            for i in frontier {
                for &c in &self.callers_of[i] {
                    if seen.insert(c) {
                        next.push(c);
                    }
                }
            }
            next.sort_unstable();
            for &c in &next {
                if let Some(e) = self.node_entry(&review.graph.nodes[c]) {
                    return Some(e);
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        None
    }

    /// Unit `u`'s facts.
    fn facts(&mut self, u: &'a Unit) -> Vec<Fact> {
        if matches!(u.status, UnitStatus::NoBehaviorChange | UnitStatus::Removed) {
            return Vec::new();
        }
        let review = self.review;
        let Some(f) = review.file_of(u) else {
            return Vec::new();
        };
        let Some(fl) = self.file_lines(&u.file) else {
            return Vec::new();
        };
        let old_text = f.old_content.as_deref().unwrap_or("");
        let new_text = f.new_content.as_deref().unwrap_or("");
        let whole = u.kind == UnitKind::File;
        let range = if whole {
            None
        } else {
            u.new_range.map(|r| (r.start, r.end))
        };
        // A unit's lines are the ones no smaller unit inside it holds.
        let mut added = Added::new(
            &u.file,
            old_text,
            new_text,
            &fl.old,
            fl.new.clone(),
            range,
            |l| whole || fl.owner.get(l as usize - 1) == Some(&u.symbol.as_str()),
        );
        // Rust's `mod tests` is test code whatever its file.
        added.test_code = u.symbol.starts_with("tests::") || u.symbol.contains("::tests::");
        let mut out = Vec::new();
        // A lock file or a snapshot is written by a tool, not a person.
        if crate::units::whole::generated(&u.file).is_none() {
            out.extend(lines::secrets(&added));
            out.extend(lines::config(&added));
            out.extend(lines::crypto(&added));
        }
        if whole {
            if ci::applies(&u.file) {
                out.extend(ci::facts(&added));
            }
            if packages::applies(&u.file) {
                out.extend(packages::facts(review, f));
            }
            return dedupe(out);
        }
        let ns = self.scan(&u.file, true);
        if let Some((ns, ni)) = ns.and_then(|s| s.sym(&u.symbol).map(|i| (s, i))) {
            let os = if u.status == UnitStatus::Changed {
                self.scan(&u.file, false)
            } else {
                None
            };
            let oi = os.as_ref().and_then(|s| s.sym(&u.symbol));
            let added_lines: HashSet<u32> = added.lines.iter().copied().collect();
            let unit = Code {
                u,
                ns: &ns,
                ni,
                os: os.as_deref().zip(oi),
                old_text,
                new_text,
                added: &added_lines,
            };
            self.code_facts(&unit, &mut out);
        }
        dedupe(out)
    }

    fn code_facts(&mut self, c: &Code, out: &mut Vec<Fact>) {
        let removed = self.removed_checks(c, out);
        if !removed {
            sibling_gap(c, out);
        }
        entry_points(c, out);
        self.injection(c, out);
        exposure(c, out);
        llm(c, out);
    }

    /// An authorization check in the unit's old code that its new code
    /// lacks. Whether any was found.
    fn removed_checks(&mut self, c: &Code, out: &mut Vec<Fact>) -> bool {
        let Some((os, oi)) = c.os else {
            return false;
        };
        // By lowercase name: the name as written, and each (line, target).
        type Checks = BTreeMap<String, (String, Vec<(u32, usize)>)>;
        let checks = |s: &Scan, i: usize| -> Checks {
            let mut m = Checks::new();
            for e in s.events.iter().filter(|e| e.owner == i) {
                if let What::Check(name) = &e.what {
                    let entry = m
                        .entry(name.to_ascii_lowercase())
                        .or_insert_with(|| (name.clone(), Vec::new()));
                    entry.1.push((e.line, e.target));
                }
            }
            m
        };
        let old = checks(os, oi);
        let new = checks(c.ns, c.ni);
        let total = |m: &Checks| -> usize { m.values().map(|v| v.1.len()).sum() };
        // One check swapped for another isn't one removed.
        if total(&new) >= total(&old) {
            return false;
        }
        let mut found = false;
        for (key, (name, occurrences)) in &old {
            let have = new.get(key).map(|v| v.1.len()).unwrap_or(0);
            if have >= occurrences.len() {
                continue;
            }
            // The ones gone: about a method that no longer has it, then on a
            // line the new code lacks, then the last ones.
            let gone = occurrences.len() - have;
            let target_keeps = |target: usize| {
                c.ns.sym(&os.syms[target].path).is_some_and(|t| {
                    c.ns.events.iter().any(|e| {
                        e.target == t
                            && matches!(&e.what, What::Check(n) if n.eq_ignore_ascii_case(key))
                    })
                })
            };
            let mut picked: Vec<(u32, usize)> = occurrences.clone();
            picked.sort_by_key(|&(line, target)| {
                (
                    target_keeps(target),
                    c.still_there(line),
                    std::cmp::Reverse(line),
                )
            });
            picked.truncate(gone);
            if self.moved_to_callee(c.u, key) {
                continue;
            }
            for (line, target) in picked {
                // A decorator or doc block about a method that is gone:
                // the method went, not its check.
                let tsym = &os.syms[target];
                if target != oi && c.ns.sym(&tsym.path).is_none() {
                    continue;
                }
                let exposed = os.entry_name(target).is_some()
                    || (tsym.kind == UnitKind::Class
                        && os
                            .syms
                            .iter()
                            .any(|s| s.parent == Some(target) && s.entry.is_some()))
                    || !c.u.callers.is_empty()
                    || self.reachable(c.u).is_some();
                let about = if target != oi {
                    format!(" from {}", tsym.path)
                } else {
                    String::new()
                };
                out.push(Fact {
                    class: Class::Authorization,
                    weight: if exposed {
                        Weight::Attention
                    } else {
                        Weight::Look
                    },
                    text: format!("{name} is gone{about} (it was on line {line})"),
                    side: Side::Old,
                    lines: Some(Range::new(line, line)),
                    subject: name.clone(),
                    origin: Origin::Teer,
                });
                found = true;
            }
        }
        found
    }

    /// A callee of `u` in this change gained check `key`: it moved there.
    fn moved_to_callee(&mut self, u: &Unit, key: &str) -> bool {
        let review = self.review;
        for id in &u.callees {
            let Some(callee) = review.unit(id) else {
                continue;
            };
            if !matches!(callee.status, UnitStatus::Added | UnitStatus::Changed) {
                continue;
            }
            let count = |s: Option<Rc<Scan>>| -> usize {
                let Some(s) = s else { return 0 };
                let Some(i) = s.sym(&callee.symbol) else {
                    return 0;
                };
                s.events
                    .iter()
                    .filter(|e| e.owner == i)
                    .filter(|e| matches!(&e.what, What::Check(n) if n.eq_ignore_ascii_case(key)))
                    .count()
            };
            let now = count(self.scan(&callee.file, true));
            let before = if callee.status == UnitStatus::Changed {
                count(self.scan(&callee.file, false))
            } else {
                0
            };
            if now > before {
                return true;
            }
        }
        false
    }

    /// A sink in the new code that the old code didn't have (or had
    /// fewer of), given something that isn't a plain literal.
    fn injection(&mut self, c: &Code, out: &mut Vec<Fact>) {
        let mut new: BTreeMap<(SinkKind, String), Vec<(u32, bool)>> = BTreeMap::new();
        for e in c.events() {
            if let What::Sink {
                kind,
                name,
                request,
            } = &e.what
            {
                new.entry((*kind, name.clone()))
                    .or_default()
                    .push((e.line, *request));
            }
        }
        let mut old: HashMap<(SinkKind, String), usize> = HashMap::new();
        for e in c.old_events() {
            if let What::Sink { kind, name, .. } = &e.what {
                *old.entry((*kind, name.clone())).or_default() += 1;
            }
        }
        let open_redirect = self
            .cfg
            .include
            .iter()
            .any(|i| i == "open_redirect" || i == "redirect");
        new.retain(|k, v| v.len() > old.get(k).copied().unwrap_or(0));
        if new.is_empty() {
            return;
        }
        let reach = self.reachable(c.u);
        let reach = reach.as_deref();
        let itself = self.entry_of(c.u).is_some();
        for ((kind, name), mut occurrences) in new {
            let had = old.get(&(kind, name.clone())).copied().unwrap_or(0);
            let extra = occurrences.len() - had;
            occurrences.sort_by_key(|(line, _)| (!c.added.contains(line), *line));
            for (line, request) in occurrences.into_iter().take(extra) {
                let src = if request {
                    "request input"
                } else {
                    "a variable"
                };
                let what = match kind {
                    SinkKind::Sql => format!("raw SQL built from {src}"),
                    SinkKind::Shell => format!("a shell command from {src}"),
                    SinkKind::Eval => format!("code evaluated from {src}"),
                    SinkKind::Deserialization => format!("deserialization of {src}"),
                    SinkKind::FilePath => format!("a file path from {src}"),
                    SinkKind::Http => format!("an outgoing request to a URL from {src}"),
                    SinkKind::Template => format!("unescaped HTML from {src}"),
                    SinkKind::Redirect => format!("a redirect to a URL from {src}"),
                };
                let place = match reach {
                    Some(e) if itself => format!(", in {e}"),
                    Some(e) => format!(", reachable from {e}"),
                    None => String::new(),
                };
                let weight = if reach.is_some() && (kind != SinkKind::Redirect || open_redirect) {
                    Weight::Look
                } else {
                    Weight::Note
                };
                out.push(fact(
                    Class::Injection,
                    weight,
                    format!("new {what} via {name}{place}"),
                    line,
                    kind.word(),
                ));
            }
        }
    }
}

/// A code unit's two sides, read.
struct Code<'c> {
    u: &'c Unit,
    ns: &'c Scan,
    ni: usize,
    os: Option<(&'c Scan, usize)>,
    old_text: &'c str,
    new_text: &'c str,
    /// The unit's added lines (new side).
    added: &'c HashSet<u32>,
}

impl Code<'_> {
    fn events(&self) -> impl Iterator<Item = &Event> {
        self.ns.events.iter().filter(move |e| e.owner == self.ni)
    }

    fn old_events(&self) -> impl Iterator<Item = &Event> {
        self.os
            .into_iter()
            .flat_map(|(s, i)| s.events.iter().filter(move |e| e.owner == i))
    }

    /// Old line `line` is still among the unit's new lines.
    fn still_there(&self, line: u32) -> bool {
        let Some(old) = self.old_text.lines().nth(line as usize - 1) else {
            return false;
        };
        let Some(r) = self.u.new_range else {
            return false;
        };
        self.new_text
            .lines()
            .skip(r.start as usize - 1)
            .take(r.len() as usize)
            .any(|l| l.trim() == old.trim())
    }
}

/// A new or changed entry point without a check, where at least two of
/// its siblings (the other actions of its class, the other handlers or
/// routes of its file) all have one.
fn sibling_gap(c: &Code, out: &mut Vec<Fact>) {
    let s = c.ns;
    let me = &s.syms[c.ni];
    if me.entry.is_some() && s.checks_on(c.ni).is_empty() {
        let class_checked = me.parent.is_some_and(|p| {
            s.events
                .iter()
                .any(|e| e.target == p && matches!(e.what, What::Check(_)))
        });
        let siblings: Vec<usize> = (0..s.syms.len())
            .filter(|&j| {
                j != c.ni
                    && s.syms[j].parent == me.parent
                    && s.syms[j].entry.is_some()
                    && matches!(s.syms[j].kind, UnitKind::Function | UnitKind::Method)
            })
            .collect();
        let checks: Vec<Vec<String>> = siblings.iter().map(|&j| s.checks_on(j)).collect();
        if !class_checked && siblings.len() >= 2 && checks.iter().all(|c| !c.is_empty()) {
            let check = most_common(checks.iter().flatten());
            let (what, place) = match me.parent {
                Some(p) => ("actions", s.syms[p].name.clone()),
                None => ("handlers", "this file".to_string()),
            };
            out.push(fact(
                Class::Authorization,
                Weight::Look,
                format!(
                    "no {check} here; the {} other {what} in {place} have it",
                    siblings.len()
                ),
                me.range.start,
                &check,
            ));
        }
    }
    // Routes registered in this unit, next to routes that all have a check.
    if s.routes.iter().any(|r| r.is_use) {
        return;
    }
    let old_routes: HashSet<&str> =
        c.os.map(|(os, oi)| {
            os.routes
                .iter()
                .filter(|r| r.owner == oi)
                .map(|r| r.route.as_str())
                .collect()
        })
        .unwrap_or_default();
    let routes: Vec<&scan::RouteReg> = s.routes.iter().filter(|r| !r.is_use).collect();
    for (k, r) in routes.iter().enumerate() {
        if r.owner != c.ni || !r.checks.is_empty() || old_routes.contains(r.route.as_str()) {
            continue;
        }
        let others: Vec<&&scan::RouteReg> = routes
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != k)
            .map(|(_, o)| o)
            .collect();
        if others.len() >= 2 && others.iter().all(|o| !o.checks.is_empty()) {
            let check = most_common(others.iter().flat_map(|o| o.checks.iter()));
            out.push(fact(
                Class::Authorization,
                Weight::Look,
                format!(
                    "no {check} on {}; the {} other routes in this file have it",
                    r.route,
                    others.len()
                ),
                r.line,
                &check,
            ));
        }
    }
}

fn most_common<'s>(names: impl Iterator<Item = &'s String>) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for n in names {
        *counts.entry(n.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(c, _)| c.to_string())
        .unwrap_or_default()
}

/// A new entry point, one whose route or signature changed, and routes
/// newly registered.
fn entry_points(c: &Code, out: &mut Vec<Fact>) {
    let me = &c.ns.syms[c.ni];
    let before = c.os.map(|(os, oi)| &os.syms[oi]);
    if let Some(e) = &me.entry {
        let line = me.range.start;
        match before.and_then(|b| b.entry.as_ref().map(|o| (b, o))) {
            None => out.push(fact(
                Class::EntryPoint,
                Weight::Look,
                format!("new {}", e.words()),
                line,
                &e.name,
            )),
            Some((_, o)) if o.name != e.name => out.push(fact(
                Class::EntryPoint,
                Weight::Look,
                format!("{} changed: {} → {}", e.how, o.name, e.name),
                line,
                &e.name,
            )),
            Some((b, _)) if b.sig != me.sig => out.push(fact(
                Class::EntryPoint,
                Weight::Look,
                format!("signature of {} changed", e.words()),
                line,
                &e.name,
            )),
            _ => {}
        }
    }
    let old_routes: HashSet<&str> =
        c.os.map(|(os, oi)| {
            os.routes
                .iter()
                .filter(|r| r.owner == oi && !r.is_use)
                .map(|r| r.route.as_str())
                .collect()
        })
        .unwrap_or_default();
    let own = me.entry.as_ref().map(|e| e.name.as_str());
    for r in c.ns.routes.iter().filter(|r| r.owner == c.ni && !r.is_use) {
        if !old_routes.contains(r.route.as_str()) && own != Some(r.route.as_str()) {
            out.push(fact(
                Class::EntryPoint,
                Weight::Look,
                format!("new route {}", r.route),
                r.line,
                &r.route,
            ));
        }
    }
}

/// A sensitive value newly logged, or put into a response.
fn exposure(c: &Code, out: &mut Vec<Fact>) {
    for e in c.events().filter(|e| c.added.contains(&e.line)) {
        let (label, strong, log) = match &e.what {
            What::Log { label, strong } => (*label, *strong, true),
            What::Respond { label, strong } => (*label, *strong, false),
            _ => continue,
        };
        let a = if label.starts_with(['a', 'e', 'i', 'o', 'u']) {
            "an"
        } else {
            "a"
        };
        let text = if log {
            format!("logs {a} {label}")
        } else {
            format!("puts {a} {label} into the response")
        };
        let weight = if strong { Weight::Look } else { Weight::Note };
        out.push(fact(Class::Exposure, weight, text, e.line, label));
    }
}

/// A model's output in a unit that also runs SQL, a shell, eval, files
/// or outgoing requests on something not literal.
fn llm(c: &Code, out: &mut Vec<Fact>) {
    let risky = |k: &SinkKind| {
        matches!(
            k,
            SinkKind::Sql | SinkKind::Shell | SinkKind::Eval | SinkKind::FilePath | SinkKind::Http
        )
    };
    let models: Vec<(&String, u32)> = c
        .events()
        .filter_map(|e| match &e.what {
            What::Model(m) => Some((m, e.line)),
            _ => None,
        })
        .collect();
    let sinks: Vec<(SinkKind, &String, u32)> = c
        .events()
        .filter_map(|e| match &e.what {
            What::Sink { kind, name, .. } if risky(kind) => Some((*kind, name, e.line)),
            _ => None,
        })
        .collect();
    let (Some((model, line)), Some((kind, sink, _))) = (models.first(), sinks.first()) else {
        return;
    };
    let old_models = c
        .old_events()
        .filter(|e| matches!(e.what, What::Model(_)))
        .count();
    let old_sinks = c
        .old_events()
        .filter(|e| matches!(&e.what, What::Sink { kind, .. } if risky(kind)))
        .count();
    if models.len() > old_models || sinks.len() > old_sinks {
        out.push(fact(
            Class::Llm,
            Weight::Look,
            format!(
                "model output can reach {}: {model} and {sink} in one unit",
                kind.word()
            ),
            *line,
            model,
        ));
    }
}

/// A fact on the new side, at `line` (none when 0).
fn fact(class: Class, weight: Weight, text: String, line: u32, subject: &str) -> Fact {
    fact_at(class, weight, text, (line > 0).then_some(line), subject)
}

fn fact_at(class: Class, weight: Weight, text: String, line: Option<u32>, subject: &str) -> Fact {
    Fact {
        class,
        weight,
        text,
        side: Side::New,
        lines: line.map(|l| Range::new(l, l)),
        subject: subject.to_string(),
        origin: Origin::Teer,
    }
}

/// One fact per class, subject and line: the first.
fn dedupe(facts: Vec<Fact>) -> Vec<Fact> {
    let mut seen = HashSet::new();
    facts
        .into_iter()
        .filter(|f| seen.insert((f.class, f.subject.clone(), f.lines.map(|r| r.start))))
        .collect()
}
