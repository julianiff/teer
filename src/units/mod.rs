//! Semantic units: parse both versions of each changed file, match symbols
//! by path, and classify what changed.

pub mod extract;
pub mod lang;
pub mod whole;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::git::ChangedFile;
use crate::review::model::{Decision, Prep, Unit, UnitKind, UnitStatus};
use crate::util::hash_hex;
use extract::{MODULE_SYMBOL, ParsedFile, Symbol, parse_source};
use lang::Lang;

/// Both parsed versions of a changed file.
#[derive(Debug, Clone)]
pub struct ParsedPair {
    pub lang: Lang,
    pub old: Option<ParsedFile>,
    pub new: Option<ParsedFile>,
}

#[derive(Debug, Default)]
pub struct Analysis {
    pub units: Vec<Unit>,
    pub parsed: HashMap<PathBuf, ParsedPair>,
}

/// Which languages to analyze; `None` means all supported ones. Any other
/// changed file is one unit for the whole file (see [`whole`]).
pub fn analyze(files: &[ChangedFile], languages: Option<&[String]>) -> Analysis {
    let mut analysis = Analysis::default();
    for f in files {
        let lang = Lang::from_path(&f.path).filter(|lang| {
            languages.is_none_or(|allowed| allowed.iter().any(|l| l == lang.config_name()))
        });
        let Some(lang) = lang else {
            analysis.units.extend(whole::unit(f));
            continue;
        };
        let old = f.old_content.as_deref().and_then(|s| parse_source(lang, s));
        let new = f.new_content.as_deref().and_then(|s| parse_source(lang, s));
        analysis
            .units
            .extend(diff_symbols(&f.path, old.as_ref(), new.as_ref()));
        analysis
            .parsed
            .insert(f.path.clone(), ParsedPair { lang, old, new });
    }
    analysis
}

pub fn unit_id(file: &std::path::Path, symbol: &str) -> String {
    format!("{}::{}", file.to_string_lossy(), symbol)
}

fn make_unit(
    file: &std::path::Path,
    sym: &Symbol,
    status: UnitStatus,
    old: Option<&Symbol>,
    new: Option<&Symbol>,
) -> Unit {
    let old_text = old.map(|s| s.own_text.as_str()).unwrap_or("");
    let new_text = new.map(|s| s.own_text.as_str()).unwrap_or("");
    Unit {
        id: unit_id(file, &sym.path),
        file: file.to_path_buf(),
        symbol: sym.path.clone(),
        kind: sym.kind,
        status,
        old_range: old.map(|s| s.range),
        new_range: new.map(|s| s.range),
        renamed_from: None,
        fingerprint: hash_hex(&[old_text.as_bytes(), new_text.as_bytes()]),
        changes: Vec::new(),
        nbc_reason: match (status, old, new) {
            (UnitStatus::NoBehaviorChange, Some(o), Some(n)) if o.norm_plain == n.norm_plain => {
                Some("formatting or comments".into())
            }
            (UnitStatus::NoBehaviorChange, _, _) => Some("local names renamed".into()),
            _ => None,
        },
        prep: if status == UnitStatus::NoBehaviorChange {
            Prep::Skipped
        } else {
            Prep::NotRequested
        },
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Pending,
        summary: None,
        dive: Default::default(),
        dive_pending: Default::default(),
        checks: Default::default(),
    }
}

/// Match old and new symbols of one file and classify each change.
pub fn diff_symbols(
    file: &std::path::Path,
    old: Option<&ParsedFile>,
    new: Option<&ParsedFile>,
) -> Vec<Unit> {
    let empty: Vec<Symbol> = Vec::new();
    let old_syms = old.map(|p| &p.symbols).unwrap_or(&empty);
    let new_syms = new.map(|p| &p.symbols).unwrap_or(&empty);
    let old_by: HashMap<&str, &Symbol> = old_syms.iter().map(|s| (s.path.as_str(), s)).collect();
    let new_by: HashMap<&str, &Symbol> = new_syms.iter().map(|s| (s.path.as_str(), s)).collect();

    let mut units = Vec::new();
    let mut removed: Vec<&Symbol> = Vec::new();
    let mut added: Vec<&Symbol> = Vec::new();

    for s in old_syms {
        match new_by.get(s.path.as_str()) {
            Some(n) => {
                if s.own_text == n.own_text {
                    continue;
                }
                let status = if s.norm == n.norm {
                    UnitStatus::NoBehaviorChange
                } else {
                    UnitStatus::Changed
                };
                // Formatting or import churn at the top level isn't worth a line.
                if s.path == MODULE_SYMBOL && status == UnitStatus::NoBehaviorChange {
                    continue;
                }
                units.push(make_unit(file, n, status, Some(s), Some(n)));
            }
            None => removed.push(s),
        }
    }
    for s in new_syms {
        if !old_by.contains_key(s.path.as_str()) {
            added.push(s);
        }
    }

    // A removed and an added symbol with the same body is a rename.
    let mut used_added = HashSet::new();
    let mut renamed = HashSet::new();
    for r in &removed {
        if r.kind == UnitKind::Module {
            continue;
        }
        if let Some((i, a)) = added.iter().enumerate().find(|(i, a)| {
            !used_added.contains(i) && a.kind == r.kind && a.norm_anon == r.norm_anon
        }) {
            used_added.insert(i);
            renamed.insert(r.path.clone());
            let mut u = make_unit(file, a, UnitStatus::NoBehaviorChange, Some(r), Some(a));
            u.renamed_from = Some(r.path.clone());
            u.nbc_reason = Some(format!("renamed from {}", r.path));
            units.push(u);
        }
    }
    for r in removed.into_iter().filter(|r| !renamed.contains(&r.path)) {
        if r.path == MODULE_SYMBOL && r.norm.is_empty() {
            continue;
        }
        units.push(make_unit(file, r, UnitStatus::Removed, Some(r), None));
    }
    for (i, a) in added.into_iter().enumerate() {
        if used_added.contains(&i) || (a.path == MODULE_SYMBOL && a.norm.is_empty()) {
            continue;
        }
        units.push(make_unit(file, a, UnitStatus::Added, None, Some(a)));
    }

    // A class whose only change is its methods is reported through them.
    units.sort_by_key(|u| (u.display_line(), u.symbol.clone()));
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(lang: Lang, old: &str, new: &str) -> Vec<(String, UnitStatus)> {
        let o = parse_source(lang, old);
        let n = parse_source(lang, new);
        diff_symbols(&PathBuf::from("f"), o.as_ref(), n.as_ref())
            .into_iter()
            .map(|u| (u.symbol, u.status))
            .collect()
    }

    #[test]
    fn detects_function_rename() {
        let got = classify(
            Lang::Rust,
            "fn total(a: u32) -> u32 { a + 1 }\n",
            "fn sum_one(a: u32) -> u32 { a + 1 }\n",
        );
        assert_eq!(got, vec![("sum_one".into(), UnitStatus::NoBehaviorChange)]);
    }

    #[test]
    fn unchanged_moves_are_ignored() {
        let got = classify(
            Lang::Python,
            "def a():\n    pass\n\ndef b():\n    pass\n",
            "def b():\n    pass\n\ndef a():\n    pass\n",
        );
        assert!(got.is_empty(), "{got:?}");
    }
}
