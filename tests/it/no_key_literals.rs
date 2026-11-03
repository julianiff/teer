//! Screens handle actions, never keys (Q16): `KeyCode::` appears in
//! `src/keymap.rs` only. Test modules may build key events, so a file is
//! read up to its first `#[cfg(test)]`.

use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_key_literals_outside_the_keymap() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    files.sort();
    let mut found = Vec::new();
    for path in &files {
        let rel = path.strip_prefix(root).unwrap();
        if rel == Path::new("src/keymap.rs") {
            continue;
        }
        let text = std::fs::read_to_string(path).unwrap();
        let code = text.split("#[cfg(test)]").next().unwrap_or_default();
        for (i, line) in code.lines().enumerate() {
            if line.contains("KeyCode::") {
                found.push(format!("{}:{}: {}", rel.display(), i + 1, line.trim()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "KeyCode:: outside src/keymap.rs; match actions from the keymap instead:\n{}",
        found.join("\n")
    );
}
