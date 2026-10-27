//! Call graph between symbols, resolved by name. Covers the changed files,
//! the files that call into them (a few hops up, for impact), and the
//! definitions of what changed code calls.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::review::model::{Range, Unit, UnitKind, UnitStatus};
use crate::units::extract::{MODULE_SYMBOL, ParsedFile, parse_source};
use crate::units::lang::Lang;
use crate::units::{Analysis, unit_id};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolNode {
    pub id: String,
    pub file: PathBuf,
    pub symbol: String,
    pub kind: UnitKind,
    pub range: Range,
    pub in_review: bool,
}

/// Serializable graph: edges point from caller to callee.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphData {
    pub nodes: Vec<SymbolNode>,
    pub edges: Vec<(usize, usize)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Family {
    Rust,
    Python,
    Js,
    Php,
}

fn family(l: Lang) -> Family {
    match l {
        Lang::Rust => Family::Rust,
        Lang::Python => Family::Python,
        Lang::Php => Family::Php,
        _ => Family::Js,
    }
}

/// How many caller hops to follow upward from changed units.
const CALLER_DEPTH: usize = 4;
/// Most files read beyond the changed ones.
const MAX_FILES: usize = 400;
/// Largest file read beyond the changed ones.
const MAX_FILE_BYTES: usize = 512 * 1024;

struct Builder {
    files: HashMap<PathBuf, (Lang, ParsedFile)>,
}

/// Build the graph and fill each unit's `callers` and `callees`.
pub fn build(repo: &Path, analysis: &Analysis, units: &mut [Unit]) -> GraphData {
    let mut b = Builder {
        files: HashMap::new(),
    };
    let mut removed: Vec<(PathBuf, Lang, crate::units::extract::Symbol)> = Vec::new();
    for (path, pair) in &analysis.parsed {
        if let Some(new) = &pair.new {
            b.files.insert(path.clone(), (pair.lang, new.clone()));
        }
        // Removed symbols still matter: callers may still call them.
        if let Some(old) = &pair.old {
            for u in units
                .iter()
                .filter(|u| &u.file == path && u.status == UnitStatus::Removed)
            {
                if let Some(s) = old.get(&u.symbol) {
                    removed.push((path.clone(), pair.lang, s.clone()));
                }
            }
        }
    }

    // Candidate files for name search: tracked source files we haven't parsed.
    let mut candidates: Vec<(PathBuf, Lang, String)> = Vec::new();
    if let Ok(tracked) = crate::git::tracked_files(repo) {
        for p in tracked {
            if b.files.contains_key(&p) {
                continue;
            }
            let Some(lang) = Lang::from_path(&p) else {
                continue;
            };
            // Plain files inside the repo only: a PR may track a symlink to
            // a secret, to /dev/zero or a FIFO.
            if let Some(src) = crate::git::read_text_file(repo, &p, MAX_FILE_BYTES) {
                candidates.push((p, lang, src));
            }
        }
    }
    let mut parsed_extra = 0usize;
    let mut parse_matching =
        |b: &mut Builder,
         names: &HashSet<String>,
         candidates: &mut Vec<(PathBuf, Lang, String)>| {
            let mut i = 0;
            while i < candidates.len() {
                if parsed_extra >= MAX_FILES {
                    break;
                }
                if names.iter().any(|n| candidates[i].2.contains(n.as_str())) {
                    let (p, lang, src) = candidates.swap_remove(i);
                    if let Some(pf) = parse_source(lang, &src) {
                        b.files.insert(p, (lang, pf));
                        parsed_extra += 1;
                    }
                } else {
                    i += 1;
                }
            }
        };

    // Definitions of what the changed units call.
    let changed: HashSet<String> = units.iter().map(|u| u.id.clone()).collect();
    let mut callee_names = HashSet::new();
    for (path, (_, pf)) in &b.files {
        for s in &pf.symbols {
            if changed.contains(&unit_id(path, &s.path)) {
                callee_names.extend(s.calls.iter().map(|c| c.name.clone()));
            }
        }
    }
    parse_matching(&mut b, &callee_names, &mut candidates);

    // Callers, hop by hop.
    // Whole-file units have no symbols: nothing calls them, they call nothing.
    let mut interest: HashSet<String> = units
        .iter()
        .filter(|u| u.kind != UnitKind::File)
        .map(|u| u.short_name().to_string())
        .collect();
    interest.retain(|n| n != MODULE_SYMBOL);
    let mut seen_interest = interest.clone();
    for _ in 0..CALLER_DEPTH {
        if interest.is_empty() {
            break;
        }
        parse_matching(&mut b, &interest, &mut candidates);
        let mut next = HashSet::new();
        for (_, pf) in b.files.values() {
            for s in &pf.symbols {
                if s.calls.iter().any(|c| interest.contains(&c.name))
                    && s.path != MODULE_SYMBOL
                    && seen_interest.insert(s.name.clone())
                {
                    next.insert(s.name.clone());
                }
            }
        }
        interest = next;
    }

    // Nodes for every symbol we parsed.
    let mut nodes: Vec<SymbolNode> = Vec::new();
    let mut node_lang: Vec<Lang> = Vec::new();
    let mut calls_of: Vec<Vec<crate::units::extract::Call>> = Vec::new();
    let mut paths: Vec<&PathBuf> = b.files.keys().collect();
    paths.sort();
    for path in paths {
        let (lang, pf) = &b.files[path];
        for s in &pf.symbols {
            let id = unit_id(path, &s.path);
            nodes.push(SymbolNode {
                in_review: changed.contains(&id),
                id,
                file: path.clone(),
                symbol: s.path.clone(),
                kind: s.kind,
                range: s.range,
            });
            node_lang.push(*lang);
            calls_of.push(s.calls.clone());
        }
    }
    for (path, lang, s) in removed {
        let id = unit_id(&path, &s.path);
        nodes.push(SymbolNode {
            in_review: true,
            id,
            file: path,
            symbol: s.path.clone(),
            kind: s.kind,
            range: s.range,
        });
        node_lang.push(lang);
        calls_of.push(Vec::new());
    }

    // Resolve calls by short name within a language family.
    let mut defs: HashMap<(Family, &str), Vec<usize>> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        if n.kind == UnitKind::Module {
            continue;
        }
        let short = n.symbol.rsplit("::").next().unwrap_or(&n.symbol);
        let short = short.split('#').next().unwrap_or(short);
        defs.entry((family(node_lang[i]), short))
            .or_default()
            .push(i);
    }
    let mut edges = HashSet::new();
    for (i, calls) in calls_of.iter().enumerate() {
        for c in calls {
            let Some(targets) = defs.get(&(family(node_lang[i]), c.name.as_str())) else {
                continue;
            };
            let targets: Vec<usize> = targets
                .iter()
                .copied()
                .filter(|&t| t != i)
                .filter(|&t| match nodes[t].kind {
                    UnitKind::Method => c.method,
                    UnitKind::Function | UnitKind::Class => !c.method || node_lang[i] == Lang::Rust,
                    UnitKind::Module | UnitKind::File => false,
                })
                .collect();
            // Very common names resolve to too many places to be useful.
            if targets.len() > 6 {
                continue;
            }
            for t in targets {
                edges.insert((i, t));
            }
        }
    }

    // Keep changed units, their callers (transitively) and direct callees.
    let mut keep = vec![false; nodes.len()];
    let mut queue: VecDeque<usize> = VecDeque::new();
    for (i, n) in nodes.iter().enumerate() {
        if n.in_review {
            keep[i] = true;
            queue.push_back(i);
        }
    }
    let mut callers_of = vec![Vec::new(); nodes.len()];
    for &(a, b) in &edges {
        callers_of[b].push(a);
    }
    while let Some(i) = queue.pop_front() {
        for &c in &callers_of[i] {
            if !keep[c] {
                keep[c] = true;
                queue.push_back(c);
            }
        }
    }
    for &(a, t) in &edges {
        if nodes[a].in_review {
            keep[t] = true;
        }
    }
    let mut remap = vec![usize::MAX; nodes.len()];
    let mut kept = Vec::new();
    for (i, n) in nodes.into_iter().enumerate() {
        if keep[i] {
            remap[i] = kept.len();
            kept.push(n);
        }
    }
    let mut edges: Vec<(usize, usize)> = edges
        .into_iter()
        .filter(|(a, b)| keep[*a] && keep[*b])
        .map(|(a, b)| (remap[a], remap[b]))
        .collect();
    edges.sort();
    let data = GraphData { nodes: kept, edges };
    fill_units(&data, units);
    data
}

pub fn fill_units(g: &GraphData, units: &mut [Unit]) {
    let mut callers = vec![Vec::new(); g.nodes.len()];
    let mut callees = vec![Vec::new(); g.nodes.len()];
    for &(a, b) in &g.edges {
        callers[b].push(g.nodes[a].id.clone());
        callees[a].push(g.nodes[b].id.clone());
    }
    let index: HashMap<&str, usize> = g
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    for u in units.iter_mut() {
        let Some(&i) = index.get(u.id.as_str()) else {
            continue;
        };
        let ids = |v: &[String]| {
            let mut v = v.to_vec();
            v.sort();
            v.dedup();
            v
        };
        u.callers = ids(&callers[i]);
        u.callees = ids(&callees[i]);
    }
}
