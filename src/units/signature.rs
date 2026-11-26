//! The shape of a function's parameter list, measured from the syntax
//! tree: how many parameters, how many of them optional, how many share
//! one type. No model counts these; teer does, so a signature that grew
//! long is said, and the ask tile offers to look into it.

use std::collections::HashMap;

use tree_sitter::Node;

use super::lang::Lang;
use crate::review::model::Signature;

/// How deep below a definition its parameter list is looked for: an
/// export, a declaration, a declarator, the function.
const MAX_DEPTH: usize = 4;

/// The signature of the function or method `outer` defines, if it has a
/// parameter list.
pub fn of(lang: Lang, outer: Node, src: &[u8]) -> Option<Signature> {
    let params = match parameter_list(outer)? {
        List::Many(list) => parameters(lang, list, src),
        // `x => x`: one parameter, no type.
        List::One => vec![Param::default()],
    };
    let mut groups: HashMap<&str, u32> = HashMap::new();
    for p in &params {
        *groups.entry(p.ty.as_str()).or_default() += 1;
    }
    // The largest group; of equal ones the typed one, then by name, so
    // the same code always measures the same.
    let (alike_type, alike) = groups
        .into_iter()
        .max_by_key(|(ty, n)| (*n, !ty.is_empty(), std::cmp::Reverse(*ty)))
        .map(|(ty, n)| (ty.to_string(), n))
        .unwrap_or_default();
    Some(Signature {
        params: params.len() as u32,
        optional: params.iter().filter(|p| p.optional).count() as u32,
        alike,
        alike_type,
        before: None,
    })
}

enum List<'t> {
    Many(Node<'t>),
    One,
}

/// The parameter list nearest to `outer`: its own, or that of the
/// function it wraps (a decorator, an export, `const f = () => {}`).
fn parameter_list(outer: Node) -> Option<List> {
    let mut level = vec![outer];
    for _ in 0..=MAX_DEPTH {
        let mut next = Vec::new();
        for n in level {
            if let Some(list) = n.child_by_field_name("parameters") {
                return Some(List::Many(list));
            }
            if n.kind() == "arrow_function" && n.child_by_field_name("parameter").is_some() {
                return Some(List::One);
            }
            let mut cursor = n.walk();
            next.extend(n.named_children(&mut cursor));
        }
        level = next;
    }
    None
}

#[derive(Default)]
struct Param {
    /// Its type without what makes it nullable; empty with none written.
    ty: String,
    /// It has a default value, or its type takes "nothing".
    optional: bool,
}

fn parameters(lang: Lang, list: Node, src: &[u8]) -> Vec<Param> {
    let text = |n: Node| n.utf8_text(src).unwrap_or("");
    let mut out = Vec::new();
    let mut cursor = list.walk();
    for (i, p) in list.named_children(&mut cursor).enumerate() {
        let kind = p.kind();
        // Not parameters a caller passes.
        if kind.contains("comment")
            || matches!(
                kind,
                "self_parameter" | "keyword_separator" | "positional_separator" | "attribute_item"
            )
        {
            continue;
        }
        let name = p
            .child_by_field_name("name")
            .or_else(|| p.child_by_field_name("pattern"))
            .map_or_else(|| text(p), text);
        let receiver = match lang {
            Lang::Python => i == 0 && matches!(name, "self" | "cls"),
            Lang::TypeScript | Lang::Tsx => name == "this",
            _ => false,
        };
        if receiver {
            continue;
        }
        let written: String = p
            .child_by_field_name("type")
            .map(text)
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let ty = plain_type(&written);
        let optional = matches!(
            kind,
            "default_parameter"
                | "typed_default_parameter"
                | "assignment_pattern"
                | "optional_parameter"
        ) || p.child_by_field_name("default_value").is_some()
            || p.child_by_field_name("value").is_some()
            || ty != written.trim_start_matches(':');
        // Go names several parameters of one type at once: `a, b string`.
        let mut names = p.walk();
        let count = p.children_by_field_name("name", &mut names).count().max(1);
        for _ in 0..count {
            out.push(Param {
                ty: ty.clone(),
                optional,
            });
        }
    }
    out
}

/// `written` (no whitespace) without what makes it nullable: `?string`,
/// `string|null`, `Option<String>`, `Optional[str]`, `str | None` and
/// `string | undefined` are all their inner type.
fn plain_type(written: &str) -> String {
    let mut t = written.trim_start_matches(':').trim_start_matches('?');
    for (open, close) in [("Option<", ">"), ("Optional[", "]")] {
        if let Some(inner) = t.strip_prefix(open).and_then(|r| r.strip_suffix(close)) {
            t = inner;
        }
    }
    let kept: Vec<&str> = t
        .split('|')
        .filter(|part| !matches!(*part, "null" | "None" | "undefined"))
        .collect();
    kept.join("|")
}

#[cfg(test)]
mod tests {
    use super::super::extract::parse_source;
    use super::*;

    /// (params, optional, alike, alike_type) of symbol `path` in `src`.
    fn measured(lang: Lang, src: &str, path: &str) -> (u32, u32, u32, String) {
        let file = parse_source(lang, src).expect("parses");
        let s = file
            .get(path)
            .unwrap_or_else(|| panic!("no symbol {path}"))
            .signature
            .clone()
            .unwrap_or_else(|| panic!("no signature for {path}"));
        (s.params, s.optional, s.alike, s.alike_type)
    }

    #[test]
    fn php_counts_nullable_and_defaulted_parameters() {
        let src = "<?php\nclass F\n{\n    public function create(\n        string $grantType,\n        ?string $code = null,\n        ?string $assertion = null,\n        string|null $scope,\n        int $ttl = 60,\n        Client ...$clients,\n    ): Req {\n    }\n}\n";
        assert_eq!(
            measured(Lang::Php, src, "F::create"),
            (6, 4, 4, "string".into())
        );
    }

    #[test]
    fn rust_leaves_out_self_and_unwraps_option() {
        let src = "struct S;\nimpl S {\n    fn go(&self, a: u32, b: Option<u32>, c: &str) {}\n}\n";
        assert_eq!(measured(Lang::Rust, src, "S::go"), (3, 1, 2, "u32".into()));
    }

    #[test]
    fn python_leaves_out_self_and_the_separators() {
        let src = "class C:\n    def run(self, a, b: int, /, c=1, *, d: str | None = None, **rest):\n        pass\n";
        // a, b, c, d, rest: `c` and `d` are optional; `a`, `c` and `rest` have no type.
        assert_eq!(
            measured(Lang::Python, src, "C::run"),
            (5, 2, 3, String::new())
        );
        let decorated = "@cached\ndef f(a: int, b: int):\n    pass\n";
        assert_eq!(
            measured(Lang::Python, decorated, "f"),
            (2, 0, 2, "int".into())
        );
    }

    #[test]
    fn typescript_knows_optional_and_default_parameters() {
        let src = "export function f(a: string, b?: string, c: string | undefined, d = 1, ...rest: number[]) {}\n";
        assert_eq!(
            measured(Lang::TypeScript, src, "f"),
            (5, 3, 3, "string".into())
        );
        let arrow = "export const g = (a: number, b: number) => a + b;\nconst h = x => x;\n";
        assert_eq!(
            measured(Lang::TypeScript, arrow, "g"),
            (2, 0, 2, "number".into())
        );
        assert_eq!(
            measured(Lang::TypeScript, arrow, "h"),
            (1, 0, 1, String::new())
        );
    }

    #[test]
    fn javascript_parameters_have_no_type() {
        let src = "function f(a, b = 2, { c }, ...d) {}\n";
        assert_eq!(
            measured(Lang::JavaScript, src, "f"),
            (4, 1, 4, String::new())
        );
    }

    #[test]
    fn go_counts_each_name_and_not_the_receiver() {
        let src = "package p\n\nfunc (c *Cart) Add(a, b string, n bool, rest ...int) {}\n\nfunc None() {}\n";
        assert_eq!(
            measured(Lang::Go, src, "Cart::Add"),
            (4, 0, 2, "string".into())
        );
        assert_eq!(measured(Lang::Go, src, "None"), (0, 0, 0, String::new()));
    }

    #[test]
    fn a_class_has_no_signature() {
        let file = parse_source(
            Lang::Php,
            "<?php\nclass F\n{\n    public function a(int $x): void\n    {\n    }\n}\n",
        )
        .unwrap();
        assert!(file.get("F").unwrap().signature.is_none());
    }
}
