//! Semantic units: parse both versions of each changed file, match symbols
//! by path, and classify what changed.

pub mod extract;
pub mod lang;
pub mod signature;
pub mod whole;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::git::ChangedFile;
use crate::review::model::{Decision, Nested, Prep, Unit, UnitKind, UnitStatus};
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

/// The outermost symbols inside `sym` on each side (a class's methods, a
/// module's classes and functions), old and new matched by symbol path.
fn nested_in(
    old: Option<(&ParsedFile, &Symbol)>,
    new: Option<(&ParsedFile, &Symbol)>,
) -> Vec<Nested> {
    let mut out: Vec<Nested> = Vec::new();
    for (is_new, (file, sym)) in [(false, old), (true, new)]
        .into_iter()
        .filter_map(|(is_new, side)| side.map(|s| (is_new, s)))
    {
        for inner in inside(file, sym) {
            let range = Some(inner.range);
            match out.iter_mut().find(|n| n.symbol == inner.path) {
                Some(n) if is_new => n.new = range,
                Some(n) => n.old = range,
                None => out.push(Nested {
                    symbol: inner.path.clone(),
                    old: if is_new { None } else { range },
                    new: if is_new { range } else { None },
                }),
            }
        }
    }
    out.sort_by_key(|n| n.new.or(n.old).map_or(0, |r| r.start));
    out
}

/// The outermost symbols whose lines lie inside `sym`'s, in file order. A
/// symbol on a class's first line stays with the class (its header).
fn inside<'a>(file: &'a ParsedFile, sym: &Symbol) -> Vec<&'a Symbol> {
    let outer = sym.range;
    let mut kept: Vec<&Symbol> = Vec::new();
    for s in &file.symbols {
        if s.kind == UnitKind::Module || s.path == sym.path {
            continue;
        }
        let r = s.range;
        let within = r.start >= outer.start
            && r.end <= outer.end
            && (sym.kind == UnitKind::Module || r.start > outer.start);
        let in_kept = kept
            .iter()
            .any(|k| k.range.start <= r.start && r.end <= k.range.end);
        if within && !in_kept {
            kept.push(s);
        }
    }
    kept
}

fn make_unit(
    file: &std::path::Path,
    sym: &Symbol,
    status: UnitStatus,
    old: Option<(&ParsedFile, &Symbol)>,
    new: Option<(&ParsedFile, &Symbol)>,
) -> Unit {
    let nested = nested_in(old, new);
    let (old, new) = (old.map(|(_, s)| s), new.map(|(_, s)| s));
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
        nested,
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
        accepted_by: None,
        reasons: Vec::new(),
        dive_pending: Default::default(),
        checks: Default::default(),
        security: Vec::new(),
        signature: new.and_then(|n| n.signature.clone()).map(|mut s| {
            s.before = old.and_then(|o| o.signature.as_ref()).map(|o| o.params);
            s
        }),
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
                units.push(make_unit(
                    file,
                    n,
                    status,
                    old.map(|p| (p, s)),
                    new.map(|p| (p, *n)),
                ));
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
            let mut u = make_unit(
                file,
                a,
                UnitStatus::NoBehaviorChange,
                old.map(|p| (p, *r)),
                new.map(|p| (p, *a)),
            );
            u.renamed_from = Some(r.path.clone());
            u.nbc_reason = Some(format!("renamed from {}", r.path));
            units.push(u);
        }
    }
    for r in removed.into_iter().filter(|r| !renamed.contains(&r.path)) {
        if r.path == MODULE_SYMBOL && r.norm.is_empty() {
            continue;
        }
        units.push(make_unit(
            file,
            r,
            UnitStatus::Removed,
            old.map(|p| (p, r)),
            None,
        ));
    }
    for (i, a) in added.into_iter().enumerate() {
        if used_added.contains(&i) || (a.path == MODULE_SYMBOL && a.norm.is_empty()) {
            continue;
        }
        units.push(make_unit(
            file,
            a,
            UnitStatus::Added,
            None,
            new.map(|p| (p, a)),
        ));
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
    fn nested_units_are_known_to_their_container() {
        use crate::review::model::Range;
        let src = "<?php\n\nnamespace App;\n\nclass Svc\n{\n    public function __construct()\n    {\n    }\n\n    public function run(): void\n    {\n    }\n}\n";
        let n = parse_source(Lang::Php, src);
        let units = diff_symbols(&PathBuf::from("f.php"), None, n.as_ref());
        let nested = |symbol: &str| -> Vec<(String, Option<Range>)> {
            let u = units.iter().find(|u| u.symbol == symbol).expect(symbol);
            u.nested.iter().map(|n| (n.symbol.clone(), n.new)).collect()
        };
        assert_eq!(
            nested(MODULE_SYMBOL),
            vec![("Svc".to_string(), Some(Range::new(5, 14)))]
        );
        assert_eq!(
            nested("Svc"),
            vec![
                ("Svc::__construct".to_string(), Some(Range::new(7, 9))),
                ("Svc::run".to_string(), Some(Range::new(11, 13))),
            ]
        );
        assert!(nested("Svc::run").is_empty());
    }

    #[test]
    fn nested_units_keep_the_side_they_are_on() {
        use crate::review::model::Range;
        let old = "<?php\nclass A\n{\n    private int $n = 0;\n\n    public function gone(): void\n    {\n    }\n}\n";
        let new = "<?php\nclass A\n{\n    private int $n = 1;\n}\n";
        let o = parse_source(Lang::Php, old);
        let n = parse_source(Lang::Php, new);
        let units = diff_symbols(&PathBuf::from("f.php"), o.as_ref(), n.as_ref());
        let a = units.iter().find(|u| u.symbol == "A").unwrap();
        let got: Vec<(&str, Option<Range>, Option<Range>)> = a
            .nested
            .iter()
            .map(|n| (n.symbol.as_str(), n.old, n.new))
            .collect();
        assert_eq!(got, vec![("A::gone", Some(Range::new(6, 8)), None)]);
    }

    #[test]
    fn a_unit_knows_its_signature_and_how_long_it_was() {
        let old = "<?php\nfunction make(string $a, string $b, string $c, string $d, string $e, string $f): void\n{\n}\n";
        let new = "<?php\nfunction make(string $a, string $b, string $c, string $d, string $e, string $f, ?string $g = null, ?string $h = null): void\n{\n}\n\nfunction fresh(int $a): void\n{\n}\n";
        let o = parse_source(Lang::Php, old);
        let n = parse_source(Lang::Php, new);
        let units = diff_symbols(&PathBuf::from("f.php"), o.as_ref(), n.as_ref());
        let sig = |symbol: &str| {
            let u = units.iter().find(|u| u.symbol == symbol).expect(symbol);
            u.signature.clone().expect("a signature")
        };
        let make = sig("make");
        assert_eq!((make.params, make.before, make.added()), (8, Some(6), 2));
        assert!(make.grew_long());
        assert_eq!(
            make.words(),
            "8 parameters, 2 new, 2 optional, 8 of type string"
        );
        let fresh = sig("fresh");
        assert_eq!((fresh.params, fresh.before), (1, None));
        assert!(!fresh.grew_long());
        // A removed function has none: there is nothing to call.
        let gone = diff_symbols(&PathBuf::from("f.php"), o.as_ref(), None);
        assert!(gone.iter().all(|u| u.signature.is_none()));
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
