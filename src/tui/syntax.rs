//! Syntax colours for the code change. Each grammar ships the query editors
//! highlight with; run over the whole file, it says what every name is to
//! the language (a function, a class, a variable), not what it looks like.
//! A file is parsed when first drawn and kept until the review is rebuilt.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use crate::units::lang::Lang;

/// What a stretch of code is, as far as colour goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Keyword,
    Function,
    /// Classes, structs, interfaces and other types, and constructors.
    Type,
    /// Variables, parameters and properties.
    Variable,
    String,
    Comment,
    /// Numbers and constants.
    Constant,
}

impl Class {
    /// The class a highlight capture (`function.method`) stands for; `None`
    /// for operators, punctuation and the rest, which keep the line's colour.
    fn of(capture: &str) -> Option<Class> {
        Some(match capture.split('.').next().unwrap_or(capture) {
            "keyword" => Class::Keyword,
            "function" => Class::Function,
            "type" | "constructor" => Class::Type,
            "variable" | "property" => Class::Variable,
            "string" | "escape" => Class::String,
            "comment" => Class::Comment,
            "number" | "constant" => Class::Constant,
            _ => return None,
        })
    }
}

/// For each line of a file (as `str::lines` splits it), the class of each
/// of its chars.
pub type Lines = Vec<Vec<Option<Class>>>;

/// Files larger than this aren't parsed for colour: the first frame would
/// wait on it.
const MAX_BYTES: usize = 2_000_000;

/// The highlight query `lang`'s grammar ships; TypeScript's adds to
/// JavaScript's.
fn query_source(lang: Lang) -> String {
    match lang {
        Lang::Rust => tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
        Lang::Python => tree_sitter_python::HIGHLIGHTS_QUERY.into(),
        Lang::JavaScript => tree_sitter_javascript::HIGHLIGHT_QUERY.into(),
        Lang::TypeScript | Lang::Tsx => format!(
            "{}\n{}",
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY
        ),
        Lang::Php => tree_sitter_php::HIGHLIGHTS_QUERY.into(),
    }
}

thread_local! {
    static QUERIES: RefCell<HashMap<Lang, Option<Rc<Query>>>> = RefCell::default();
}

/// `lang`'s compiled query, once per thread; `None` when it doesn't compile.
fn query(lang: Lang) -> Option<Rc<Query>> {
    QUERIES.with(|q| {
        q.borrow_mut()
            .entry(lang)
            .or_insert_with(|| {
                let language = lang.ts_language();
                Query::new(&language, &query_source(lang)).ok().map(Rc::new)
            })
            .clone()
    })
}

/// The class of each char of `content`, a file in `lang`, line by line.
/// Empty when the grammar or its query fails, or the file is too large.
pub fn classes(lang: Lang, content: &str) -> Lines {
    if content.len() > MAX_BYTES {
        return Vec::new();
    }
    let Some(query) = query(lang) else {
        return Vec::new();
    };
    let mut parser = Parser::new();
    if parser.set_language(&lang.ts_language()).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };
    let names = query.capture_names();
    let mut found = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(&query, tree.root_node(), content.as_bytes());
    while let Some((m, i)) = captures.next() {
        let c = m.captures()[*i];
        if let Some(class) = Class::of(names[c.index as usize]) {
            found.push((c.node.byte_range(), m.pattern_index, class));
        }
    }
    // Wider nodes first, then later patterns: a name inside a call is the
    // call's function, and a specific pattern beats the general one before
    // it (`(identifier) @variable` comes first in the queries).
    found.sort_by_key(|(r, pattern, _)| (std::cmp::Reverse(r.len()), *pattern));
    let mut by_byte = vec![None; content.len()];
    for (r, _, class) in found {
        by_byte[r].fill(Some(class));
    }
    let mut out = Vec::new();
    let mut at = 0;
    for line in content.split_inclusive('\n') {
        let text = line.strip_suffix('\n').unwrap_or(line);
        let text = text.strip_suffix('\r').unwrap_or(text);
        out.push(text.char_indices().map(|(k, _)| by_byte[at + k]).collect());
        at += line.len();
    }
    out
}

/// The classes of the files drawn so far, by path and side (`true` for the
/// new file), and of pieces of files, by path and text.
#[derive(Debug, Default)]
pub struct Cache {
    files: RefCell<HashMap<(PathBuf, bool), Rc<Lines>>>,
    pieces: RefCell<HashMap<(PathBuf, String), Rc<Lines>>>,
}

impl Cache {
    /// Forget every file: the review was built again.
    pub fn clear(&self) {
        self.files.borrow_mut().clear();
    }

    /// The classes of `content`, the `new` (or old) side of `path`; empty
    /// for a language teer has no grammar for.
    pub fn get(&self, path: &Path, new: bool, content: &str) -> Rc<Lines> {
        let key = (path.to_path_buf(), new);
        if let Some(lines) = self.files.borrow().get(&key) {
            return lines.clone();
        }
        let lines = Rc::new(
            Lang::from_path(path)
                .map(|lang| classes(lang, content))
                .unwrap_or_default(),
        );
        self.files.borrow_mut().insert(key, lines.clone());
        lines
    }

    /// The classes of `text`, some lines of `path` parsed on their own (PHP
    /// gets the opening tag it needs first). Close, not exact: what the
    /// lines sit in is not known.
    pub fn piece(&self, path: &Path, text: &str) -> Rc<Lines> {
        let key = (path.to_path_buf(), text.to_string());
        if let Some(lines) = self.pieces.borrow().get(&key) {
            return lines.clone();
        }
        let lines = Rc::new(match Lang::from_path(path) {
            Some(Lang::Php) if !text.trim_start().starts_with("<?") => {
                let mut l = classes(Lang::Php, &format!("<?php\n{text}"));
                if !l.is_empty() {
                    l.remove(0);
                }
                l
            }
            Some(lang) => classes(lang, text),
            None => Vec::new(),
        });
        self.pieces.borrow_mut().insert(key, lines.clone());
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The class of the first char of `word` on line `line` (0-based).
    fn class_at(lines: &Lines, src: &str, line: usize, word: &str) -> Option<Class> {
        let text = src.lines().nth(line).unwrap();
        let k = text[..text.find(word).unwrap()].chars().count();
        lines[line][k]
    }

    #[test]
    fn names_are_coloured_by_what_they_are() {
        let src = "class Cart:\n    def add(self, item):\n        total = price(item) # sum\n        return \"ok\", 3\n";
        let lines = classes(Lang::Python, src);
        assert_eq!(lines.len(), 4);
        assert_eq!(class_at(&lines, src, 0, "class"), Some(Class::Keyword));
        assert_eq!(class_at(&lines, src, 0, "Cart"), Some(Class::Type));
        assert_eq!(class_at(&lines, src, 1, "add"), Some(Class::Function));
        assert_eq!(class_at(&lines, src, 2, "total"), Some(Class::Variable));
        assert_eq!(class_at(&lines, src, 2, "price"), Some(Class::Function));
        assert_eq!(class_at(&lines, src, 2, "# sum"), Some(Class::Comment));
        assert_eq!(class_at(&lines, src, 3, "\"ok\""), Some(Class::String));
        assert_eq!(class_at(&lines, src, 3, "3"), Some(Class::Constant));
        assert_eq!(class_at(&lines, src, 2, "="), None);
        assert_eq!(class_at(&lines, src, 3, "return"), Some(Class::Keyword));
        let src = "def f(order):\n    if order.total <= 0:\n        raise ValueError(\"x\")\n";
        let lines = classes(Lang::Python, src);
        assert_eq!(class_at(&lines, src, 1, "if"), Some(Class::Keyword));
        assert_eq!(class_at(&lines, src, 2, "raise"), Some(Class::Keyword));
    }

    #[test]
    fn every_grammar_has_a_query_that_compiles() {
        for lang in [
            Lang::Rust,
            Lang::Python,
            Lang::JavaScript,
            Lang::TypeScript,
            Lang::Tsx,
            Lang::Php,
        ] {
            assert!(query(lang).is_some(), "{lang:?}");
        }
        let ts = "class Cart { add(item: Item): number { return price(item); } }\n";
        let lines = classes(Lang::TypeScript, ts);
        assert_eq!(class_at(&lines, ts, 0, "Cart"), Some(Class::Type));
        assert_eq!(class_at(&lines, ts, 0, "Item"), Some(Class::Type));
        assert_eq!(class_at(&lines, ts, 0, "price"), Some(Class::Function));
        let rs = "fn charge(order: &Order) -> u32 { pay(order) }\n";
        let lines = classes(Lang::Rust, rs);
        assert_eq!(class_at(&lines, rs, 0, "fn"), Some(Class::Keyword));
        assert_eq!(class_at(&lines, rs, 0, "charge"), Some(Class::Function));
        assert_eq!(class_at(&lines, rs, 0, "Order"), Some(Class::Type));
        let php = "<?php\nfunction charge($order) { return pay($order); }\n";
        let lines = classes(Lang::Php, php);
        assert_eq!(class_at(&lines, php, 1, "charge"), Some(Class::Function));
        assert_eq!(class_at(&lines, php, 1, "$order"), Some(Class::Variable));
    }

    #[test]
    fn lines_split_like_str_lines_and_count_chars_not_bytes() {
        let src = "x = \"ä\"\r\ny = 1";
        let lines = classes(Lang::Python, src);
        assert_eq!(lines.len(), src.lines().count());
        assert_eq!(lines[0].len(), "x = \"ä\"".chars().count());
        assert_eq!(lines[1].len(), 5);
        // No grammar for the file: no colour, and asking twice parses once.
        let cache = Cache::default();
        assert!(cache.get(Path::new("notes.md"), true, "# hi").is_empty());
        let a = cache.get(Path::new("a.py"), true, src);
        let b = cache.get(Path::new("a.py"), true, "other");
        assert!(Rc::ptr_eq(&a, &b));
        cache.clear();
        assert!(cache.get(Path::new("a.py"), true, "").is_empty());
    }
}
