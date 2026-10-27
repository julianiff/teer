//! Symbol extraction with tree-sitter: functions, methods, classes and the
//! module top level, each with a normalized token stream for detecting
//! changes that don't change behavior, and the calls it makes.

use std::collections::{HashMap, HashSet};
use std::ops::Range as ByteRange;

use tree_sitter::{Node, Parser, Tree};

use super::lang::Lang;
use crate::review::model::{Range, UnitKind};

pub const MODULE_SYMBOL: &str = "<module>";

#[derive(Debug, Clone)]
pub struct Call {
    pub name: String,
    /// Called through a receiver (`x.foo()`, `Foo::new()`): resolves to methods.
    pub method: bool,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    /// Symbol path, e.g. `PaymentService::charge_order`.
    pub path: String,
    pub name: String,
    pub kind: UnitKind,
    pub range: Range,
    /// Source of the symbol with nested symbols cut out.
    pub own_text: String,
    /// Tokens without comments and formatting, local names canonicalized.
    pub norm: Vec<String>,
    /// `norm` with the symbol's own name replaced, for rename detection.
    pub norm_anon: Vec<String>,
    /// `norm` without canonicalizing local names: equal means only
    /// formatting or comments changed.
    pub norm_plain: Vec<String>,
    pub calls: Vec<Call>,
}

#[derive(Debug, Clone)]
pub struct ParsedFile {
    pub symbols: Vec<Symbol>,
}

impl ParsedFile {
    pub fn get(&self, path: &str) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.path == path)
    }
}

pub fn parse_tree(lang: Lang, src: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&lang.ts_language()).ok()?;
    parser.parse(src, None)
}

pub fn node_range(n: Node) -> Range {
    let start = n.start_position().row as u32 + 1;
    let end_pos = n.end_position();
    let mut end = end_pos.row as u32 + 1;
    if end_pos.column == 0 && end > start {
        end -= 1;
    }
    Range::new(start, end)
}

struct Def<'t> {
    outer: Node<'t>,
    name: String,
    path: String,
    kind: UnitKind,
}

/// Parse a source file and extract its symbols. The first symbol is always
/// the module top level.
pub fn parse_source(lang: Lang, src: &str) -> Option<ParsedFile> {
    let tree = parse_tree(lang, src)?;
    let root = tree.root_node();
    let bytes = src.as_bytes();

    let mut defs: Vec<Def> = Vec::new();
    let mut stack: Vec<(String, bool)> = Vec::new();
    collect_defs(lang, root, bytes, &mut stack, &mut defs);

    // Disambiguate duplicate paths (overloads, conditional definitions).
    let mut seen: HashMap<String, usize> = HashMap::new();
    for d in defs.iter_mut() {
        let n = seen.entry(d.path.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            d.path = format!("{}#{}", d.path, n);
        }
    }

    let outer_ids: HashSet<usize> = defs.iter().map(|d| d.outer.id()).collect();
    let mut symbols = Vec::with_capacity(defs.len() + 1);

    // Module top level: everything that isn't a symbol or an import.
    {
        let skip = |n: Node| {
            outer_ids.contains(&n.id())
                || (n.parent().is_some_and(|p| p.id() == root.id()) && lang.is_import(n.kind()))
        };
        let mut norm = Vec::new();
        emit_tokens(
            lang,
            root,
            bytes,
            &skip,
            &HashSet::new(),
            &mut HashMap::new(),
            &mut norm,
        );
        let norm = strip_trailing_commas(norm);
        let mut calls = Vec::new();
        collect_calls(lang, root, bytes, &skip, &mut calls);
        let full = Range::new(1, (src.lines().count() as u32).max(1));
        symbols.push(Symbol {
            path: MODULE_SYMBOL.into(),
            name: MODULE_SYMBOL.into(),
            kind: UnitKind::Module,
            range: full,
            own_text: splice_out(src, 0..src.len(), defs.iter().map(|d| d.outer.byte_range())),
            norm_anon: norm.clone(),
            norm_plain: norm.clone(),
            norm,
            calls,
        });
    }

    for d in &defs {
        let me = d.outer.id();
        let skip = |n: Node| n.id() != me && outer_ids.contains(&n.id());
        let mut bindings = HashSet::new();
        if matches!(d.kind, UnitKind::Function | UnitKind::Method) {
            collect_bindings(lang, d.outer, bytes, &skip, &mut bindings);
        }
        let mut norm = Vec::new();
        emit_tokens(
            lang,
            d.outer,
            bytes,
            &skip,
            &bindings,
            &mut HashMap::new(),
            &mut norm,
        );
        let norm = strip_trailing_commas(norm);
        let mut plain = Vec::new();
        emit_tokens(
            lang,
            d.outer,
            bytes,
            &skip,
            &HashSet::new(),
            &mut HashMap::new(),
            &mut plain,
        );
        let norm_plain = strip_trailing_commas(plain);
        let norm_anon = norm
            .iter()
            .map(|t| {
                if *t == d.name {
                    "$self".to_string()
                } else {
                    t.clone()
                }
            })
            .collect();
        let mut calls = Vec::new();
        collect_calls(lang, d.outer, bytes, &skip, &mut calls);
        let range = d.outer.byte_range();
        let nested = defs
            .iter()
            .filter(|o| o.outer.id() != me)
            .map(|o| o.outer.byte_range());
        symbols.push(Symbol {
            path: d.path.clone(),
            name: d.name.clone(),
            kind: d.kind,
            range: node_range(d.outer),
            own_text: splice_out(src, range, nested),
            norm,
            norm_anon,
            norm_plain,
            calls,
        });
    }
    Some(ParsedFile { symbols })
}

/// Return `src[outer]` with every range strictly inside it removed.
fn splice_out(
    src: &str,
    outer: ByteRange<usize>,
    inner: impl Iterator<Item = ByteRange<usize>>,
) -> String {
    let mut cuts: Vec<ByteRange<usize>> = inner
        .filter(|r| r.start >= outer.start && r.end <= outer.end && *r != outer)
        .collect();
    cuts.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    let mut out = String::new();
    let mut pos = outer.start;
    for c in cuts {
        if c.start < pos {
            continue; // contained in a previous cut
        }
        out.push_str(&src[pos..c.start]);
        pos = c.end;
    }
    out.push_str(&src[pos..outer.end]);
    out
}

fn text<'a>(n: Node, src: &'a [u8]) -> &'a str {
    n.utf8_text(src).unwrap_or("")
}

fn field_text(n: Node, field: &str, src: &[u8]) -> Option<String> {
    n.child_by_field_name(field)
        .map(|c| text(c, src).to_string())
}

/// Name of a Rust type node without generics: `Foo<T>` -> `Foo`.
fn rust_type_name(n: Node, src: &[u8]) -> String {
    match n.kind() {
        "generic_type" => n
            .child_by_field_name("type")
            .map(|t| rust_type_name(t, src))
            .unwrap_or_default(),
        "scoped_type_identifier" => field_text(n, "name", src).unwrap_or_default(),
        "reference_type" => n
            .child_by_field_name("type")
            .map(|t| rust_type_name(t, src))
            .unwrap_or_default(),
        _ => text(n, src).to_string(),
    }
}

fn wrap_export(n: Node) -> Node {
    match n.parent() {
        Some(p) if p.kind() == "export_statement" => p,
        _ => n,
    }
}

/// What a node defines, if anything: (name, kind, outer node, opens a class scope).
fn classify<'t>(
    lang: Lang,
    n: Node<'t>,
    src: &[u8],
    in_class: bool,
) -> Option<(String, UnitKind, Node<'t>, bool)> {
    let fn_kind = if in_class {
        UnitKind::Method
    } else {
        UnitKind::Function
    };
    match lang {
        Lang::Rust => match n.kind() {
            "function_item" => Some((field_text(n, "name", src)?, fn_kind, n, false)),
            "struct_item" | "enum_item" | "union_item" => {
                Some((field_text(n, "name", src)?, UnitKind::Class, n, true))
            }
            "trait_item" => Some((field_text(n, "name", src)?, UnitKind::Class, n, true)),
            _ => None,
        },
        Lang::Php => match n.kind() {
            "function_definition" => {
                Some((field_text(n, "name", src)?, UnitKind::Function, n, false))
            }
            "method_declaration" => Some((field_text(n, "name", src)?, UnitKind::Method, n, false)),
            "class_declaration"
            | "trait_declaration"
            | "interface_declaration"
            | "enum_declaration" => Some((field_text(n, "name", src)?, UnitKind::Class, n, true)),
            _ => None,
        },
        Lang::Python => match n.kind() {
            "function_definition" | "class_definition" => {
                let outer = match n.parent() {
                    Some(p) if p.kind() == "decorated_definition" => p,
                    _ => n,
                };
                let name = field_text(n, "name", src)?;
                if n.kind() == "class_definition" {
                    Some((name, UnitKind::Class, outer, true))
                } else {
                    Some((name, fn_kind, outer, false))
                }
            }
            _ => None,
        },
        _ => match n.kind() {
            "function_declaration" | "generator_function_declaration" => Some((
                field_text(n, "name", src)?,
                UnitKind::Function,
                wrap_export(n),
                false,
            )),
            "class_declaration" | "abstract_class_declaration" => Some((
                field_text(n, "name", src)?,
                UnitKind::Class,
                wrap_export(n),
                true,
            )),
            "interface_declaration" | "enum_declaration" | "type_alias_declaration" => Some((
                field_text(n, "name", src)?,
                UnitKind::Class,
                wrap_export(n),
                false,
            )),
            "method_definition" => Some((field_text(n, "name", src)?, UnitKind::Method, n, false)),
            "public_field_definition" | "field_definition" => {
                let value = n.child_by_field_name("value")?;
                if !matches!(
                    value.kind(),
                    "arrow_function" | "function_expression" | "function"
                ) {
                    return None;
                }
                let name = field_text(n, "name", src).or_else(|| field_text(n, "property", src))?;
                Some((name, UnitKind::Method, n, false))
            }
            "variable_declarator" => {
                let value = n.child_by_field_name("value")?;
                if !matches!(
                    value.kind(),
                    "arrow_function" | "function_expression" | "function" | "generator_function"
                ) {
                    return None;
                }
                let name_node = n.child_by_field_name("name")?;
                if name_node.kind() != "identifier" {
                    return None;
                }
                let mut outer = n;
                if let Some(p) = n.parent()
                    && matches!(p.kind(), "lexical_declaration" | "variable_declaration")
                    && p.named_child_count() == 1
                {
                    outer = wrap_export(p);
                }
                Some((text(name_node, src).to_string(), fn_kind, outer, false))
            }
            _ => None,
        },
    }
}

fn collect_defs<'t>(
    lang: Lang,
    n: Node<'t>,
    src: &[u8],
    stack: &mut Vec<(String, bool)>,
    out: &mut Vec<Def<'t>>,
) {
    let in_class = stack.last().is_some_and(|(_, class)| *class);
    let mut pushed = false;
    if let Some((name, kind, outer, opens_class)) = classify(lang, n, src, in_class) {
        let mut path: Vec<&str> = stack.iter().map(|(s, _)| s.as_str()).collect();
        path.push(&name);
        out.push(Def {
            outer,
            path: path.join("::"),
            name: name.clone(),
            kind,
        });
        stack.push((name, opens_class));
        pushed = true;
    } else if lang == Lang::Rust {
        // Containers that aren't units themselves.
        let container = match n.kind() {
            "impl_item" => n
                .child_by_field_name("type")
                .map(|t| (rust_type_name(t, src), true)),
            "mod_item" if n.child_by_field_name("body").is_some() => {
                field_text(n, "name", src).map(|m| (m, false))
            }
            _ => None,
        };
        if let Some(c) = container {
            stack.push(c);
            pushed = true;
        }
    }
    let mut cursor = n.walk();
    for child in n.children(&mut cursor) {
        collect_defs(lang, child, src, stack, out);
    }
    if pushed {
        stack.pop();
    }
}

fn is_comment(kind: &str) -> bool {
    kind.contains("comment")
}

/// A Python docstring: a lone string as the first statement of a body.
fn is_docstring(lang: Lang, n: Node) -> bool {
    if lang != Lang::Python || n.kind() != "expression_statement" || n.named_child_count() != 1 {
        return false;
    }
    if n.named_child(0).map(|c| c.kind()) != Some("string") {
        return false;
    }
    let Some(parent) = n.parent() else {
        return false;
    };
    let first = parent.named_child(0).is_some_and(|c| c.id() == n.id());
    first && matches!(parent.kind(), "block" | "module")
}

fn emit_tokens(
    lang: Lang,
    n: Node,
    src: &[u8],
    skip: &dyn Fn(Node) -> bool,
    bindings: &HashSet<String>,
    canon: &mut HashMap<String, usize>,
    out: &mut Vec<String>,
) {
    if skip(n) || is_comment(n.kind()) || is_docstring(lang, n) {
        return;
    }
    if n.child_count() == 0 {
        let t = text(n, src);
        if t.trim().is_empty() {
            return;
        }
        if lang.is_js_like() && t == ";" {
            return;
        }
        let kind = n.kind();
        // In PHP, single and double quotes mean different things (interpolation).
        if lang != Lang::Php && matches!(kind, "string_start" | "string_end" | "\"" | "'" | "`") {
            out.push(t.replace('\'', "\""));
            return;
        }
        // PHP variables are `variable_name` nodes wrapping `$` and a `name`.
        let is_var = kind == "identifier"
            || (lang == Lang::Php
                && kind == "name"
                && n.parent().is_some_and(|p| p.kind() == "variable_name"));
        if is_var && bindings.contains(t) {
            let next = canon.len();
            let idx = *canon.entry(t.to_string()).or_insert(next);
            out.push(format!("$v{idx}"));
            return;
        }
        out.push(t.to_string());
        return;
    }
    let mut cursor = n.walk();
    for child in n.children(&mut cursor) {
        emit_tokens(lang, child, src, skip, bindings, canon, out);
    }
}

fn strip_trailing_commas(tokens: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        if t == ","
            && tokens
                .get(i + 1)
                .is_some_and(|n| matches!(n.as_str(), ")" | "]" | "}"))
        {
            continue;
        }
        out.push(t.clone());
    }
    out
}

/// Identifiers bound by patterns. Field names, types and paths inside a
/// pattern are not bindings.
fn pattern_idents(n: Node, src: &[u8], out: &mut HashSet<String>) {
    match n.kind() {
        "identifier" => {
            out.insert(text(n, src).to_string());
            return;
        }
        "scoped_identifier"
        | "field_pattern"
        | "attribute"
        | "subscript"
        | "member_expression"
        | "subscript_expression"
        | "type_annotation"
        | "shorthand_property_identifier_pattern" => return,
        _ => {}
    }
    for i in 0..n.child_count() {
        if matches!(
            n.field_name_for_child(i),
            Some("type" | "path" | "function" | "key")
        ) {
            continue;
        }
        if let Some(c) = n.child(i) {
            pattern_idents(c, src, out);
        }
    }
}

/// Variables bound by a PHP assignment target: `$a`, `[$a, $b]`, `list($a)`.
/// Property and array element targets aren't local bindings.
fn php_vars(n: Node, src: &[u8], out: &mut HashSet<String>) {
    match n.kind() {
        "variable_name" => {
            if let Some(name) = n.named_child(0) {
                let t = text(name, src);
                if t != "this" {
                    out.insert(t.to_string());
                }
            }
        }
        "member_access_expression"
        | "nullsafe_member_access_expression"
        | "scoped_property_access_expression"
        | "subscript_expression"
        | "function_call_expression"
        | "member_call_expression" => {}
        _ => {
            let mut c = n.walk();
            for child in n.named_children(&mut c) {
                php_vars(child, src, out);
            }
        }
    }
}

fn collect_bindings(
    lang: Lang,
    n: Node,
    src: &[u8],
    skip: &dyn Fn(Node) -> bool,
    out: &mut HashSet<String>,
) {
    if skip(n) {
        return;
    }
    let field = |f: &str| n.child_by_field_name(f);
    match lang {
        // Parameters stay as they are: PHP 8 calls can name them.
        Lang::Php => match n.kind() {
            "assignment_expression" | "augmented_assignment_expression" => {
                if let Some(p) = field("left") {
                    php_vars(p, src, out);
                }
            }
            "catch_clause" => {
                if let Some(p) = field("name") {
                    php_vars(p, src, out);
                }
            }
            "foreach_statement" => {
                // `foreach ($items as $k => $v)`: everything after the subject.
                let mut c = n.walk();
                let body = field("body").map(|b| b.id());
                for child in n.named_children(&mut c).skip(1) {
                    if Some(child.id()) != body {
                        php_vars(child, src, out);
                    }
                }
            }
            _ => {}
        },
        Lang::Rust => match n.kind() {
            "let_declaration" | "parameter" | "for_expression" | "match_arm" | "let_condition" => {
                if let Some(p) = field("pattern") {
                    pattern_idents(p, src, out);
                }
            }
            "closure_parameters" => pattern_idents(n, src, out),
            _ => {}
        },
        Lang::Python => match n.kind() {
            "assignment" | "augmented_assignment" | "for_statement" | "for_in_clause" => {
                if let Some(p) = field("left") {
                    pattern_idents(p, src, out);
                }
            }
            "as_pattern" => {
                if let Some(p) = field("alias") {
                    pattern_idents(p, src, out);
                }
            }
            "named_expression" => {
                if let Some(p) = field("name") {
                    pattern_idents(p, src, out);
                }
            }
            _ => {}
        },
        _ => match n.kind() {
            "variable_declarator" => {
                // A declarator that defines a unit binds the unit's own name.
                if let (Some(p), Some(v)) = (field("name"), field("value"))
                    && !matches!(
                        v.kind(),
                        "arrow_function" | "function_expression" | "function"
                    )
                {
                    pattern_idents(p, src, out);
                } else if field("value").is_none()
                    && let Some(p) = field("name")
                {
                    pattern_idents(p, src, out);
                }
            }
            "formal_parameters" => {
                let mut c = n.walk();
                for p in n.named_children(&mut c) {
                    match p.kind() {
                        "required_parameter" | "optional_parameter" => {
                            if let Some(pat) = p.child_by_field_name("pattern") {
                                pattern_idents(pat, src, out);
                            }
                        }
                        "assignment_pattern" => {
                            if let Some(l) = p.child_by_field_name("left") {
                                pattern_idents(l, src, out);
                            }
                        }
                        _ => pattern_idents(p, src, out),
                    }
                }
            }
            "arrow_function" => {
                if let Some(p) = field("parameter") {
                    pattern_idents(p, src, out);
                }
            }
            "catch_clause" => {
                if let Some(p) = field("parameter") {
                    pattern_idents(p, src, out);
                }
            }
            "for_in_statement" => {
                if let Some(p) = field("left") {
                    pattern_idents(p, src, out);
                }
            }
            _ => {}
        },
    }
    let mut cursor = n.walk();
    for child in n.children(&mut cursor) {
        collect_bindings(lang, child, src, skip, out);
    }
}

fn callee_name(lang: Lang, f: Node, src: &[u8]) -> Option<(String, bool)> {
    let t = |n: Node| text(n, src).to_string();
    match (lang, f.kind()) {
        (_, "identifier") => Some((t(f), false)),
        (Lang::Rust, "field_expression") => f.child_by_field_name("field").map(|x| (t(x), true)),
        (Lang::Rust, "scoped_identifier") => f.child_by_field_name("name").map(|x| (t(x), true)),
        (Lang::Rust, "generic_function") => f
            .child_by_field_name("function")
            .and_then(|x| callee_name(lang, x, src)),
        (Lang::Python, "attribute") => f.child_by_field_name("attribute").map(|x| (t(x), true)),
        (_, "member_expression") => f.child_by_field_name("property").map(|x| (t(x), true)),
        _ => None,
    }
}

/// Last segment of a PHP name: `App\\Billing\\charge` -> `charge`.
fn php_name(n: Node, src: &[u8]) -> Option<String> {
    match n.kind() {
        "name" => Some(text(n, src).to_string()),
        "qualified_name" => text(n, src)
            .rsplit('\\')
            .next()
            .map(|s| s.trim().to_string()),
        _ => None,
    }
}

fn php_callee(n: Node, src: &[u8]) -> Option<(String, bool)> {
    match n.kind() {
        "function_call_expression" => {
            php_name(n.child_by_field_name("function")?, src).map(|s| (s, false))
        }
        "member_call_expression" | "nullsafe_member_call_expression" | "scoped_call_expression" => {
            php_name(n.child_by_field_name("name")?, src).map(|s| (s, true))
        }
        "object_creation_expression" => {
            let mut c = n.walk();
            let class = n
                .named_children(&mut c)
                .find(|k| matches!(k.kind(), "name" | "qualified_name"))?;
            php_name(class, src).map(|s| (s, false))
        }
        _ => None,
    }
}

/// Every call under `n`, nested symbols included.
pub fn calls_under(lang: Lang, n: Node, src: &[u8]) -> Vec<Call> {
    let mut out = Vec::new();
    collect_calls(lang, n, src, &|_| false, &mut out);
    out
}

fn collect_calls(
    lang: Lang,
    n: Node,
    src: &[u8],
    skip: &dyn Fn(Node) -> bool,
    out: &mut Vec<Call>,
) {
    if skip(n) {
        return;
    }
    let callee = if lang == Lang::Php {
        php_callee(n, src)
    } else {
        let target = match n.kind() {
            "call_expression" | "call" => n.child_by_field_name("function"),
            "new_expression" => n.child_by_field_name("constructor"),
            _ => None,
        };
        target.and_then(|f| callee_name(lang, f, src))
    };
    if let Some((name, method)) = callee {
        out.push(Call {
            name,
            method,
            line: n.start_position().row as u32 + 1,
        });
    }
    let mut cursor = n.walk();
    for child in n.children(&mut cursor) {
        collect_calls(lang, child, src, skip, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(lang: Lang, src: &str) -> Vec<(String, UnitKind)> {
        parse_source(lang, src)
            .unwrap()
            .symbols
            .into_iter()
            .skip(1)
            .map(|s| (s.path, s.kind))
            .collect()
    }

    #[test]
    fn rust_symbols() {
        let src = "struct A { x: u32 }\nimpl A {\n    fn new() -> Self { A { x: 1 } }\n}\nfn free() {}\nmod m { fn inner() {} }\n";
        assert_eq!(
            paths(Lang::Rust, src),
            vec![
                ("A".into(), UnitKind::Class),
                ("A::new".into(), UnitKind::Method),
                ("free".into(), UnitKind::Function),
                ("m::inner".into(), UnitKind::Function),
            ]
        );
    }

    #[test]
    fn python_symbols_with_decorators() {
        let src = "class S:\n    @property\n    def a(self):\n        return 1\n\ndef f():\n    def g():\n        pass\n";
        let parsed = parse_source(Lang::Python, src).unwrap();
        let a = parsed.get("S::a").unwrap();
        assert_eq!(a.kind, UnitKind::Method);
        assert_eq!(a.range, Range::new(2, 4));
        assert!(parsed.get("f::g").is_some());
        assert_eq!(parsed.get("f::g").unwrap().kind, UnitKind::Function);
    }

    #[test]
    fn ts_symbols() {
        let src = "export function a() {}\nconst b = () => 1;\nclass C {\n  m() { return a(); }\n  n = () => 2;\n}\n";
        assert_eq!(
            paths(Lang::TypeScript, src),
            vec![
                ("a".into(), UnitKind::Function),
                ("b".into(), UnitKind::Function),
                ("C".into(), UnitKind::Class),
                ("C::m".into(), UnitKind::Method),
                ("C::n".into(), UnitKind::Method),
            ]
        );
    }

    #[test]
    fn php_symbols_and_calls() {
        let src = "<?php\nnamespace App;\n\nfunction helper($x) { return strlen($x); }\n\nclass Cart {\n    public function add($item) {\n        $this->items[] = $item;\n        return Totals::recalc(new Item($item));\n    }\n}\n\ntrait Loggable { function log() {} }\n";
        let p = parse_source(Lang::Php, src).unwrap();
        let got: Vec<_> = p
            .symbols
            .iter()
            .skip(1)
            .map(|s| (s.path.as_str(), s.kind))
            .collect();
        assert_eq!(
            got,
            vec![
                ("helper", UnitKind::Function),
                ("Cart", UnitKind::Class),
                ("Cart::add", UnitKind::Method),
                ("Loggable", UnitKind::Class),
                ("Loggable::log", UnitKind::Method),
            ]
        );
        let calls: Vec<_> = p
            .get("Cart::add")
            .unwrap()
            .calls
            .iter()
            .map(|c| (c.name.as_str(), c.method))
            .collect();
        assert_eq!(calls, vec![("recalc", true), ("Item", false)]);
        assert_eq!(p.get("helper").unwrap().calls[0].name, "strlen");
    }

    #[test]
    fn php_local_renames_and_quotes() {
        let a = parse_source(
            Lang::Php,
            "<?php\nfunction f($x) { $total = $x + 1; return $total; }\n",
        )
        .unwrap();
        let b = parse_source(
            Lang::Php,
            "<?php\nfunction f($x) {\n    // note\n    $sum = $x + 1;\n    return $sum;\n}\n",
        )
        .unwrap();
        assert_eq!(a.get("f").unwrap().norm, b.get("f").unwrap().norm);
        // Renaming a parameter matters (named arguments), and so do quotes.
        let c = parse_source(
            Lang::Php,
            "<?php\nfunction f($y) { $total = $y + 1; return $total; }\n",
        )
        .unwrap();
        assert_ne!(a.get("f").unwrap().norm, c.get("f").unwrap().norm);
        let q1 = parse_source(Lang::Php, "<?php\nfunction g() { return 'a$b'; }\n").unwrap();
        let q2 = parse_source(Lang::Php, "<?php\nfunction g() { return \"a$b\"; }\n").unwrap();
        assert_ne!(q1.get("g").unwrap().norm, q2.get("g").unwrap().norm);
    }

    #[test]
    fn local_renames_normalize_equal() {
        let a = parse_source(
            Lang::Python,
            "def f(x):\n    total = x + 1\n    return total\n",
        )
        .unwrap();
        let b = parse_source(
            Lang::Python,
            "def f(x):\n    result = x + 1  # note\n    return result\n",
        )
        .unwrap();
        assert_eq!(a.get("f").unwrap().norm, b.get("f").unwrap().norm);
        // Parameters are keyword-addressable in Python, so renaming one matters.
        let c = parse_source(
            Lang::Python,
            "def f(y):\n    total = y + 1\n    return total\n",
        )
        .unwrap();
        assert_ne!(a.get("f").unwrap().norm, c.get("f").unwrap().norm);
    }

    #[test]
    fn class_own_content_excludes_methods() {
        let a = parse_source(
            Lang::Python,
            "class S:\n    x = 1\n    def a(self):\n        return 1\n",
        )
        .unwrap();
        let b = parse_source(
            Lang::Python,
            "class S:\n    x = 1\n    def a(self):\n        return 2\n",
        )
        .unwrap();
        assert_eq!(a.get("S").unwrap().own_text, b.get("S").unwrap().own_text);
    }

    #[test]
    fn calls_are_collected() {
        let p = parse_source(Lang::Rust, "fn a() { b(); x.c(); Foo::d(); }").unwrap();
        let calls: Vec<_> = p
            .get("a")
            .unwrap()
            .calls
            .iter()
            .map(|c| (c.name.clone(), c.method))
            .collect();
        assert_eq!(
            calls,
            vec![("b".into(), false), ("c".into(), true), ("d".into(), true)]
        );
    }
}
