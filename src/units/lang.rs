//! Supported languages and the tree-sitter node kinds that matter to us.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tree_sitter::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Php,
}

impl Lang {
    pub fn from_path(path: &Path) -> Option<Lang> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "rs" => Lang::Rust,
            "py" | "pyi" => Lang::Python,
            "js" | "jsx" | "mjs" | "cjs" => Lang::JavaScript,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "php" => Lang::Php,
            _ => return None,
        })
    }

    /// Name used in config files (`languages = ["rust", "python"]`).
    pub fn config_name(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Python => "python",
            Lang::JavaScript => "javascript",
            Lang::TypeScript | Lang::Tsx => "typescript",
            Lang::Php => "php",
        }
    }

    pub fn ts_language(self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            // The full grammar, so files with `<?php` tags and inline HTML parse.
            Lang::Php => tree_sitter_php::LANGUAGE_PHP.into(),
        }
    }

    pub fn is_js_like(self) -> bool {
        matches!(self, Lang::JavaScript | Lang::TypeScript | Lang::Tsx)
    }

    /// Top-level statements that don't change behavior on their own.
    pub fn is_import(self, kind: &str) -> bool {
        match self {
            Lang::Rust => matches!(kind, "use_declaration" | "extern_crate_declaration"),
            Lang::Python => matches!(
                kind,
                "import_statement" | "import_from_statement" | "future_import_statement"
            ),
            Lang::Php => kind == "namespace_use_declaration",
            _ => kind == "import_statement",
        }
    }
}
