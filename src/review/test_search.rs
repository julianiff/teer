//! Find the tests that reach a unit without an agent: parse the repository's
//! test files with tree-sitter, list their test cases, and keep those that
//! call the unit, call one of its callers, or name it. The Tests tile shows
//! this list at once; the agent reads it only when asked.

use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use tree_sitter::Node;

use super::model::{Cite, FoundTest, Range, Reach, Review, TestSearch, Unit, UnitKind};
use crate::units::extract::{self, Call, MODULE_SYMBOL};
use crate::units::lang::Lang;

/// How many calls away a caller may be and still lead to its tests.
const MAX_HOPS: u32 = 2;
/// Most callers searched for, nearest first.
const MAX_CALLERS: usize = 30;
/// Most test files parsed per search, nearest to the unit first.
const MAX_PARSED: usize = 400;
const MAX_FILE_BYTES: usize = 512 * 1024;
/// Most tests kept, nearest first.
pub const MAX_FOUND: usize = 30;
/// Most lines of a found test shown to the agent.
pub const MAX_CODE_LINES: usize = 40;

/// A symbol that calls the unit, directly or `hops` calls away.
#[derive(Debug, Clone, PartialEq)]
pub struct Caller {
    pub symbol: String,
    pub kind: UnitKind,
    pub file: PathBuf,
    pub hops: u32,
}

fn rank(r: &Reach) -> u32 {
    match r {
        Reach::Calls { .. } => 0,
        Reach::Through { hops, .. } => *hops,
        Reach::Names => MAX_HOPS + 1,
    }
}

/// The first [`MAX_CODE_LINES`] lines of a found test, for the agent; `...`
/// marks a cut. Empty when the file can't be read any more.
pub fn code(root: &Path, t: &FoundTest) -> String {
    let Some(text) = crate::git::read_text_file(root, &t.cite.file, MAX_FILE_BYTES) else {
        return String::new();
    };
    let lines: Vec<&str> = text
        .lines()
        .skip(t.cite.start.saturating_sub(1) as usize)
        .take((t.cite.end + 1).saturating_sub(t.cite.start) as usize)
        .collect();
    let mut code: Vec<&str> = lines.iter().take(MAX_CODE_LINES).copied().collect();
    if lines.len() > MAX_CODE_LINES {
        code.push("...");
    }
    code.join("\n")
}

/// The unit's callers up to [`MAX_HOPS`] calls away, nearest first, from the
/// review's call graph.
pub fn callers(review: &Review, unit: &Unit) -> Vec<Caller> {
    let g = &review.graph;
    let Some(start) = g.nodes.iter().position(|n| n.id == unit.id) else {
        return Vec::new();
    };
    let mut hops: HashMap<usize, u32> = HashMap::from([(start, 0)]);
    let mut queue = VecDeque::from([start]);
    let mut out = Vec::new();
    while let Some(i) = queue.pop_front() {
        let h = hops[&i];
        if h == MAX_HOPS {
            continue;
        }
        for &(a, _) in g.edges.iter().filter(|(_, b)| *b == i) {
            let n = &g.nodes[a];
            // A module's top level runs on import: nothing calls it by name.
            if hops.contains_key(&a) || matches!(n.kind, UnitKind::Module | UnitKind::File) {
                continue;
            }
            hops.insert(a, h + 1);
            queue.push_back(a);
            out.push(Caller {
                symbol: n.symbol.clone(),
                kind: n.kind,
                file: n.file.clone(),
                hops: h + 1,
            });
        }
    }
    out.truncate(MAX_CALLERS);
    out
}

/// A name whose calls reach the unit.
#[derive(Clone)]
struct Target {
    /// As a call spells it: `charge_order`, or the class for a constructor.
    name: String,
    symbol: String,
    hops: u32,
    file: PathBuf,
    /// Words a file that uses it would contain: its class and its module.
    owners: Vec<String>,
}

fn short(symbol: &str) -> &str {
    let s = symbol.rsplit("::").next().unwrap_or(symbol);
    s.split('#').next().unwrap_or(s)
}

/// What an import of `file` names: its stem, or its folder for `mod.rs`,
/// `index.ts` and the like.
fn module_name(file: &Path) -> String {
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if matches!(stem.as_str(), "mod" | "index" | "__init__" | "lib" | "main")
        && let Some(dir) = file.parent().and_then(|p| p.file_name())
    {
        return dir.to_string_lossy().into_owned();
    }
    stem
}

fn target(symbol: &str, kind: UnitKind, file: &Path, hops: u32) -> Option<Target> {
    if matches!(kind, UnitKind::Module | UnitKind::File) || symbol == MODULE_SYMBOL {
        return None;
    }
    let parts: Vec<&str> = symbol.split("::").map(short).collect();
    let class = match kind {
        UnitKind::Class => parts.last().copied(),
        UnitKind::Method if parts.len() > 1 => Some(parts[parts.len() - 2]),
        _ => None,
    };
    let mut name = short(symbol).to_string();
    // A constructor is called by its class's name: `new Foo`, `Foo()`.
    if matches!(name.as_str(), "__construct" | "__init__" | "constructor")
        && let Some(c) = class
    {
        name = c.to_string();
    }
    let mut owners: Vec<String> = class.map(str::to_string).into_iter().collect();
    owners.push(module_name(file));
    owners.dedup();
    owners.retain(|o| o.len() > 1);
    Some(Target {
        name,
        symbol: symbol.to_string(),
        hops,
        file: file.to_path_buf(),
        owners,
    })
}

/// Whether `path` is where tests live: a test folder, or a test file name.
/// Rust's unit tests sit in source files and are checked by content instead.
fn is_test_path(path: &Path) -> bool {
    let dirs = path.parent().into_iter().flat_map(|p| p.components());
    if dirs.into_iter().any(|c| {
        let c = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        matches!(
            c.as_str(),
            "test" | "tests" | "__tests__" | "spec" | "specs" | "e2e" | "testing"
        )
    }) {
        return true;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.contains(".test.")
        || name.contains(".spec.")
        || matches!(stem.as_str(), "test" | "tests")
        || stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_spec")
        || (stem.len() > 4 && stem.ends_with("Test"))
}

fn rust_has_tests(text: &str) -> bool {
    ["#[test]", "::test]", "::test(", "#[rstest", "#[test_case"]
        .iter()
        .any(|m| text.contains(m))
}

/// Whether a test file is named after `module`: `test_checkout.py`,
/// `checkout.test.ts`, `CheckoutTest.php` for `checkout`.
fn named_after(path: &Path, module: &str) -> bool {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    module.len() > 2 && stem != module.to_lowercase() && stem.contains(&module.to_lowercase())
}

/// Folders `a` and `b` share from the top.
fn shared_dirs(a: &Path, b: &Path) -> usize {
    let (a, b) = (a.parent(), b.parent());
    a.into_iter()
        .flat_map(|p| p.components())
        .zip(b.into_iter().flat_map(|p| p.components()))
        .take_while(|(x, y)| x == y)
        .count()
}

/// `word` in `text` as a whole identifier: not inside a longer name.
fn has_word(text: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(i, _)| {
        !text[..i].chars().next_back().is_some_and(ident)
            && !text[i + word.len()..].chars().next().is_some_and(ident)
    })
}

/// Search the test files under `root` for tests that reach `unit`: those
/// that call it, call one of `callers`, or name it. Matching is by name, so
/// a found test may call something else that happens to share it; the agent
/// judges that. Tests in files that never mention the unit's class or module
/// are left out.
pub fn find(root: &Path, unit: &Unit, callers: &[Caller]) -> TestSearch {
    let mut targets: Vec<Target> = target(&unit.symbol, unit.kind, &unit.file, 0)
        .into_iter()
        .collect();
    targets.extend(
        callers
            .iter()
            .filter_map(|c| target(&c.symbol, c.kind, &c.file, c.hops)),
    );
    let module = module_name(&unit.file);
    let whole = targets.first().is_none_or(|t| t.hops > 0);
    let Ok(tracked) = crate::git::tracked_files(root) else {
        return TestSearch::default();
    };
    let mut files: Vec<(PathBuf, Lang)> = tracked
        .into_iter()
        .filter_map(|p| Lang::from_path(&p).map(|l| (p, l)))
        .filter(|(p, l)| *l == Lang::Rust || is_test_path(p))
        .collect();
    // Nearest first, so the parse cap leaves out far files.
    files.sort_by_cached_key(|(p, _)| {
        (
            !named_after(p, &module),
            Reverse(shared_dirs(p, &unit.file)),
            p.clone(),
        )
    });
    let mut found = Vec::new();
    let mut parsed = 0;
    for (path, lang) in files {
        if parsed >= MAX_PARSED {
            break;
        }
        let named = named_after(&path, &module);
        let Some(text) = crate::git::read_text_file(root, &path, MAX_FILE_BYTES) else {
            continue;
        };
        if !named && !targets.iter().any(|t| text.contains(t.name.as_str())) {
            continue;
        }
        if lang == Lang::Rust && !is_test_path(&path) && !rust_has_tests(&text) {
            continue;
        }
        let Some(tree) = extract::parse_tree(lang, &text) else {
            continue;
        };
        parsed += 1;
        let src = text.as_bytes();
        let mut calls = extract::calls_under(lang, tree.root_node(), src);
        if lang == Lang::Rust {
            macro_calls(tree.root_node(), src, &mut calls);
        }
        // Which names this file can mean: it is the target's own file, or
        // mentions its class or module (an import, a `use`).
        let related: Vec<bool> = targets
            .iter()
            .map(|t| t.file == path || t.owners.iter().any(|o| has_word(&text, o)))
            .collect();
        let cases = cases_in(lang, tree.root_node(), src);
        let mut here: Vec<Target> = targets
            .iter()
            .zip(&related)
            .filter(|(_, ok)| **ok)
            .map(|(t, _)| t.clone())
            .collect();
        here.extend(helpers(lang, &text, &path, &cases, &calls, &here));
        let lines: Vec<&str> = text.lines().collect();
        for case in cases {
            let body = || {
                let (a, b) = (case.range.start as usize - 1, case.range.end as usize);
                lines[a.min(lines.len())..b.min(lines.len())].join("\n")
            };
            let reach = match best_call(case.range, &calls, &here) {
                Some((t, line)) if t.hops == 0 => Reach::Calls { line },
                Some((t, line)) => Reach::Through {
                    via: t.symbol.clone(),
                    hops: t.hops,
                    line,
                },
                None if whole && named => Reach::Names,
                None if !whole && (related[0] || named) && has_word(&body(), &targets[0].name) => {
                    Reach::Names
                }
                None => continue,
            };
            found.push((
                named,
                FoundTest {
                    name: case.name,
                    cite: Cite {
                        file: path.clone(),
                        start: case.range.start,
                        end: case.range.end,
                    },
                    reach,
                },
            ));
        }
    }
    found.sort_by_key(|(named, t)| (rank(&t.reach), !named, t.cite.file.clone(), t.cite.start));
    let total = found.len();
    let mut tests: Vec<FoundTest> = found.into_iter().map(|(_, t)| t).collect();
    tests.truncate(MAX_FOUND);
    TestSearch { tests, total }
}

/// The nearest target a call inside `range` reaches, and where.
fn best_call<'t>(range: Range, calls: &[Call], targets: &'t [Target]) -> Option<(&'t Target, u32)> {
    let mut best: Option<(&Target, u32)> = None;
    for c in calls.iter().filter(|c| range.contains(c.line)) {
        for t in targets.iter().filter(|t| t.name == c.name) {
            if best.is_none_or(|(b, _)| t.hops < b.hops) {
                best = Some((t, c.line));
            }
        }
    }
    best
}

/// Functions of a test file that aren't tests but reach the unit through
/// `targets` (`assertSideEffects()` calling it): tests that call them reach
/// it too.
fn helpers(
    lang: Lang,
    text: &str,
    path: &Path,
    cases: &[Case],
    calls: &[Call],
    targets: &[Target],
) -> Vec<Target> {
    let Some(parsed) = extract::parse_source(lang, text) else {
        return Vec::new();
    };
    let defs: Vec<&extract::Symbol> = parsed
        .symbols
        .iter()
        .filter(|s| matches!(s.kind, UnitKind::Function | UnitKind::Method))
        .filter(|s| {
            !cases
                .iter()
                .any(|c| s.range.start <= c.range.end && c.range.start <= s.range.end)
        })
        .collect();
    let mut out: Vec<Target> = Vec::new();
    loop {
        let known: Vec<Target> = targets.iter().chain(&out).cloned().collect();
        let more: Vec<Target> = defs
            .iter()
            .filter(|d| !out.iter().any(|t| t.symbol == d.path))
            .filter_map(|d| {
                let (t, _) = best_call(d.range, calls, &known)?;
                (t.hops < MAX_HOPS).then(|| Target {
                    name: d.name.clone(),
                    symbol: d.path.clone(),
                    hops: t.hops + 1,
                    file: path.to_path_buf(),
                    owners: Vec::new(),
                })
            })
            .collect();
        if more.is_empty() {
            return out;
        }
        out.extend(more);
    }
}

/// Calls inside Rust macro arguments (`assert_eq!(charge(1), 1)`), which
/// tree-sitter leaves as plain tokens: a name followed by `(...)`.
fn macro_calls(n: Node, src: &[u8], out: &mut Vec<Call>) {
    let mut cursor = n.walk();
    let kids: Vec<Node> = n.children(&mut cursor).collect();
    if n.kind() == "token_tree" {
        for w in kids.windows(2) {
            if w[0].kind() == "identifier"
                && w[1].kind() == "token_tree"
                && text(w[1], src).starts_with('(')
            {
                out.push(Call {
                    name: text(w[0], src).to_string(),
                    method: false,
                    line: w[0].start_position().row as u32 + 1,
                });
            }
        }
    }
    for k in kids {
        macro_calls(k, src, out);
    }
}

/// A test case: its name and its lines.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub name: String,
    pub range: Range,
}

fn cases_in(lang: Lang, root: Node, src: &[u8]) -> Vec<Case> {
    let mut out = Vec::new();
    walk(lang, root, src, &mut Vec::new(), &mut out);
    out
}

fn text<'s>(n: Node, src: &'s [u8]) -> &'s str {
    n.utf8_text(src).unwrap_or("")
}

fn walk(lang: Lang, n: Node, src: &[u8], scope: &mut Vec<String>, out: &mut Vec<Case>) {
    if let Some((name, start)) = case_at(lang, n, src) {
        let sep = if lang.is_js_like() { " › " } else { "::" };
        let mut full = scope.clone();
        full.push(name);
        let end = extract::node_range(n).end;
        out.push(Case {
            name: super::validate::one_line(&full.join(sep), 120),
            range: Range::new(start.row as u32 + 1, end),
        });
        // What a test defines inside itself isn't another test.
        return;
    }
    let pushed = scope_at(lang, n, src).map(|s| scope.push(s)).is_some();
    let mut cursor = n.walk();
    for child in n.children(&mut cursor) {
        walk(lang, child, src, scope, out);
    }
    if pushed {
        scope.pop();
    }
}

/// A Rust attribute that makes a test: `#[test]`, `#[tokio::test(..)]`,
/// `#[rstest]`, `#[test_case(..)]`.
fn rust_test_attr(attr: &str) -> bool {
    let inner = attr.trim().trim_start_matches("#[").trim_end_matches(']');
    let path = inner.split(['(', ' ']).next().unwrap_or("").trim();
    path == "test" || path.ends_with("::test") || path == "rstest" || path == "test_case"
}

/// The name of the test `n` is and the position its lines start at
/// (attributes and decorators included), if it is one.
fn case_at(lang: Lang, n: Node, src: &[u8]) -> Option<(String, tree_sitter::Point)> {
    let name = |n: Node| {
        n.child_by_field_name("name")
            .map(|x| text(x, src).to_string())
    };
    match lang {
        Lang::Rust => {
            if n.kind() != "function_item" {
                return None;
            }
            let (mut start, mut test) = (n.start_position(), false);
            let mut prev = n.prev_sibling();
            while let Some(p) = prev {
                match p.kind() {
                    "attribute_item" => {
                        test |= rust_test_attr(text(p, src));
                        start = p.start_position();
                    }
                    "line_comment" | "block_comment" => {}
                    _ => break,
                }
                prev = p.prev_sibling();
            }
            test.then(|| Some((name(n)?, start))).flatten()
        }
        Lang::Python => {
            if n.kind() != "function_definition" {
                return None;
            }
            let fname = name(n)?;
            if !fname.starts_with("test") {
                return None;
            }
            let outer = match n.parent() {
                Some(p) if p.kind() == "decorated_definition" => p,
                _ => n,
            };
            Some((fname, outer.start_position()))
        }
        Lang::Php => match n.kind() {
            "method_declaration" => {
                let mname = name(n)?;
                let mut cursor = n.walk();
                let attr = n
                    .children(&mut cursor)
                    .any(|c| c.kind() == "attribute_list" && has_word(text(c, src), "Test"));
                let doc = n
                    .prev_named_sibling()
                    .is_some_and(|p| p.kind() == "comment" && text(p, src).contains("@test"));
                (mname.starts_with("test") || attr || doc).then(|| (mname, n.start_position()))
            }
            // Pest: `it('charges once', function () { ... })`.
            "function_call_expression" => {
                let f = text(n.child_by_field_name("function")?, src);
                matches!(f, "it" | "test")
                    .then(|| first_string(n, src))
                    .flatten()
                    .map(|s| (s, n.start_position()))
            }
            _ => None,
        },
        _ => {
            if n.kind() != "call_expression" {
                return None;
            }
            let (base, member) = js_callee(n.child_by_field_name("function")?, src)?;
            let test = matches!(base, "it" | "test")
                && matches!(
                    member,
                    "" | "only" | "skip" | "concurrent" | "failing" | "todo" | "each()"
                );
            test.then(|| first_string(n, src))
                .flatten()
                .map(|s| (s, n.start_position()))
        }
    }
}

/// The name a test inside `n` gets in front of its own: a Rust `mod`, a
/// class, a `describe` block.
fn scope_at(lang: Lang, n: Node, src: &[u8]) -> Option<String> {
    let name = || {
        n.child_by_field_name("name")
            .map(|x| text(x, src).to_string())
    };
    match (lang, n.kind()) {
        (Lang::Rust, "mod_item") => name(),
        (Lang::Python, "class_definition") => name(),
        (Lang::Php, "class_declaration") => name(),
        (Lang::Php, "function_call_expression") => {
            let f = text(n.child_by_field_name("function")?, src);
            (f == "describe").then(|| first_string(n, src)).flatten()
        }
        (l, "call_expression") if l.is_js_like() => {
            let (base, member) = js_callee(n.child_by_field_name("function")?, src)?;
            let scope = (matches!(base, "describe" | "context" | "suite")
                && matches!(member, "" | "only" | "skip" | "each()"))
                || (base == "test" && member == "describe");
            scope.then(|| first_string(n, src)).flatten()
        }
        (l, "class_declaration") if l.is_js_like() => name(),
        _ => None,
    }
}

/// `it` → (`it`, ""), `it.only` → (`it`, `only`), `it.each(rows)` → (`it`, `each()`).
fn js_callee<'s>(f: Node, src: &'s [u8]) -> Option<(&'s str, &'s str)> {
    match f.kind() {
        "identifier" => Some((text(f, src), "")),
        "member_expression" => {
            let obj = f.child_by_field_name("object")?;
            let prop = f.child_by_field_name("property")?;
            (obj.kind() == "identifier").then(|| (text(obj, src), text(prop, src)))
        }
        "call_expression" => {
            let (base, member) = js_callee(f.child_by_field_name("function")?, src)?;
            (member == "each").then_some((base, "each()"))
        }
        _ => None,
    }
}

/// The first argument of a call, when it is a string: the test's title.
fn first_string(call: Node, src: &[u8]) -> Option<String> {
    let args = call.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    let mut first = args.named_children(&mut cursor).next()?;
    // PHP wraps each argument.
    if first.kind() == "argument" {
        first = first.named_child(0)?;
    }
    if !matches!(
        first.kind(),
        "string" | "template_string" | "encapsed_string" | "string_literal"
    ) {
        return None;
    }
    let s = text(first, src).trim_matches(['\'', '"', '`']).trim();
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(lang: Lang, src: &str) -> Vec<(String, u32, u32)> {
        let tree = extract::parse_tree(lang, src).unwrap();
        cases_in(lang, tree.root_node(), src.as_bytes())
            .into_iter()
            .map(|c| (c.name, c.range.start, c.range.end))
            .collect()
    }

    #[test]
    fn rust_tests_by_attribute() {
        let src = "fn helper() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn one() {\n        helper();\n    }\n\n    /// Doc.\n    #[tokio::test(flavor = \"multi_thread\")]\n    async fn two() {}\n\n    #[cfg(test)]\n    fn not_a_test() {}\n}\n";
        assert_eq!(
            names(Lang::Rust, src),
            [("tests::one".into(), 5, 8), ("tests::two".into(), 11, 12)]
        );
    }

    #[test]
    fn python_test_functions_and_methods() {
        let src = "import pytest\n\n@pytest.mark.parametrize('x', [1])\ndef test_a(x):\n    def test_inner():\n        pass\n\nclass TestB:\n    def test_c(self):\n        pass\n\n    def helper(self):\n        pass\n\ndef make():\n    pass\n";
        assert_eq!(
            names(Lang::Python, src),
            [("test_a".into(), 3, 6), ("TestB::test_c".into(), 9, 10)]
        );
    }

    #[test]
    fn js_its_inside_describes() {
        let src = "describe('Cart', () => {\n  it('adds', () => {\n    add(1);\n  });\n  it.each([1, 2])('takes %d', (n) => {\n    add(n);\n  });\n  describe.skip('empty', () => {\n    test(`is empty`, () => {});\n  });\n  beforeEach(() => reset());\n});\ntest.only('alone', () => {});\n";
        assert_eq!(
            names(Lang::TypeScript, src),
            [
                ("Cart › adds".into(), 2, 4),
                ("Cart › takes %d".into(), 5, 7),
                ("Cart › empty › is empty".into(), 9, 9),
                ("alone".into(), 13, 13)
            ]
        );
    }

    #[test]
    fn php_phpunit_methods_and_pest() {
        let src = "<?php\nclass CartTest extends TestCase\n{\n    public function testAdds(): void\n    {\n        $this->cart->add(1);\n    }\n\n    /** @test */\n    public function it_empties(): void {}\n\n    #[Test]\n    public function removes(): void {}\n\n    private function helper(): void {}\n}\n\nit('works alone', function () {\n    expect(true)->toBeTrue();\n});\n";
        assert_eq!(
            names(Lang::Php, src),
            [
                ("CartTest::testAdds".into(), 4, 7),
                ("CartTest::it_empties".into(), 10, 10),
                ("CartTest::removes".into(), 12, 13),
                ("works alone".into(), 18, 20)
            ]
        );
    }

    #[test]
    fn test_paths() {
        for p in [
            "tests/a.rs",
            "app/__tests__/cart.ts",
            "src/cart.test.ts",
            "src/cart.spec.js",
            "test_cart.py",
            "cart_test.py",
            "src/CartTest.php",
            "Tests/Unit/Cart.php",
        ] {
            assert!(is_test_path(Path::new(p)), "{p}");
        }
        for p in [
            "src/cart.ts",
            "src/testing_utils_x.py",
            "Test.php",
            "src/latest.py",
        ] {
            assert!(!is_test_path(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn words_are_whole_identifiers() {
        assert!(has_word("charge(order)", "charge"));
        assert!(has_word("patch('app.billing.charge')", "charge"));
        assert!(!has_word("recharge(order)", "charge"));
        assert!(!has_word("charge_order()", "charge"));
        assert!(!has_word("x", ""));
    }

    #[test]
    fn module_names_skip_index_files() {
        assert_eq!(module_name(Path::new("src/billing/mod.rs")), "billing");
        assert_eq!(module_name(Path::new("web/cart/index.ts")), "cart");
        assert_eq!(module_name(Path::new("checkout.py")), "checkout");
    }

    #[test]
    fn constructors_are_called_by_their_class() {
        let t = target(
            "Cart::__construct",
            UnitKind::Method,
            Path::new("src/Cart.php"),
            0,
        )
        .unwrap();
        assert_eq!(t.name, "Cart");
        assert_eq!(t.owners, ["Cart"]);
        assert!(target(MODULE_SYMBOL, UnitKind::Module, Path::new("a.py"), 0).is_none());
    }
}
