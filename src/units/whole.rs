//! Whole-file units: a changed text file teer has no parser for (Markdown,
//! JSON, YAML, a language it doesn't parse) or whose language `languages`
//! leaves out is one unit for the whole file, so nothing that changed goes
//! unseen. It has no symbols, so no callers or callees. Lock files and test
//! snapshots are routine and never sent to the agent; neither is a change
//! too large to describe.

use std::path::Path;

use crate::git::{ChangedFile, FileStatus};
use crate::review::model::{Decision, Prep, Range, Unit, UnitKind, UnitStatus};
use crate::util::{DiffOp, diff_lines, hash_hex};

/// The symbol of a whole-file unit.
pub const WHOLE_FILE: &str = "(whole file)";

/// A whole-file change with more changed lines than this, or more text in
/// them than [`MAX_DESCRIBED_BYTES`] (minified code, data), is not sent to
/// the agent: the reviewer reads the diff.
pub const MAX_DESCRIBED_LINES: usize = 300;
pub const MAX_DESCRIBED_BYTES: usize = 32 * 1024;

const LOCK_FILES: [&str; 9] = [
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "composer.lock",
    "poetry.lock",
    "Gemfile.lock",
    "go.sum",
    "uv.lock",
];

/// Why a file is routine whatever changed in it, found from its path: a
/// lock file or a test snapshot.
pub fn generated(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_string_lossy();
    if LOCK_FILES.contains(&name.as_ref()) {
        return Some("lock file");
    }
    let snapshot = path.extension().is_some_and(|e| e == "snap")
        || path.components().any(|c| c.as_os_str() == "__snapshots__");
    snapshot.then_some("test snapshot")
}

/// What kind of file this is, in words for the agent: "Markdown file".
pub fn file_type(path: &Path) -> String {
    if let Some(what) = generated(path) {
        return what.into();
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if matches!(name.as_str(), "Dockerfile" | "Containerfile") || name.ends_with(".Dockerfile") {
        return "Dockerfile".into();
    }
    if matches!(name.as_str(), "Makefile" | "GNUmakefile") {
        return "Makefile".into();
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let kind = match ext.as_str() {
        "md" | "markdown" | "mdx" => "Markdown",
        "json" | "jsonc" | "json5" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "xml" => "XML",
        "svg" => "SVG",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "scss" | "sass" => "Sass",
        "less" => "Less",
        "sql" => "SQL",
        "graphql" | "gql" => "GraphQL",
        "proto" => "Protocol Buffers",
        "csv" => "CSV",
        "ini" | "cfg" | "conf" => "config",
        "txt" => "text",
        "sh" | "bash" | "zsh" => return "shell script".into(),
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "swift" => "Swift",
        "rb" => "Ruby",
        "c" | "h" => "C",
        "cc" | "cpp" | "cxx" | "hpp" => "C++",
        "cs" => "C#",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "tf" => "Terraform",
        "rs" => "Rust",
        "py" | "pyi" => "Python",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "ts" | "mts" | "cts" | "tsx" => "TypeScript",
        "php" => "PHP",
        "" => return "text file".into(),
        other => return format!(".{other} file"),
    };
    format!("{kind} file")
}

/// Is the change of `f` too large to send to the agent? Then the reviewer
/// reads its diff instead.
pub fn too_large(f: &ChangedFile) -> bool {
    let old: Vec<&str> = f.old_content.as_deref().unwrap_or("").lines().collect();
    let new: Vec<&str> = f.new_content.as_deref().unwrap_or("").lines().collect();
    let (mut lines, mut bytes) = (0, 0);
    for (op, i, j) in diff_lines(&old, &new) {
        match op {
            DiffOp::Equal => continue,
            DiffOp::Delete => bytes += old[i].len(),
            DiffOp::Insert => bytes += new[j].len(),
        }
        lines += 1;
    }
    lines > MAX_DESCRIBED_LINES || bytes > MAX_DESCRIBED_BYTES
}

/// The whole of one side, as a range; `None` when the side doesn't exist.
fn whole(content: Option<&str>) -> Option<Range> {
    content.map(|c| Range::new(1, (c.lines().count() as u32).max(1)))
}

/// One unit for the whole of `f`. A rename with the same text has no
/// behavior change; any other file whose text didn't change (a mode
/// change) has no unit.
pub fn unit(f: &ChangedFile) -> Option<Unit> {
    let old = f.old_content.as_deref();
    let new = f.new_content.as_deref();
    let status = match f.status {
        FileStatus::Added => UnitStatus::Added,
        FileStatus::Deleted => UnitStatus::Removed,
        _ if old == new => UnitStatus::NoBehaviorChange,
        _ => UnitStatus::Changed,
    };
    let nbc_reason = match (&f.old_path, status) {
        (Some(from), UnitStatus::NoBehaviorChange) => {
            Some(format!("renamed from {}", from.display()))
        }
        (None, UnitStatus::NoBehaviorChange) => return None,
        _ => None,
    };
    // Lock files, snapshots and very large changes are never sent to the agent.
    let asked =
        status != UnitStatus::NoBehaviorChange && generated(&f.path).is_none() && !too_large(f);
    Some(Unit {
        id: super::unit_id(&f.path, WHOLE_FILE),
        file: f.path.clone(),
        symbol: WHOLE_FILE.into(),
        kind: UnitKind::File,
        status,
        old_range: whole(old),
        new_range: whole(new),
        renamed_from: None,
        fingerprint: hash_hex(&[old.unwrap_or("").as_bytes(), new.unwrap_or("").as_bytes()]),
        changes: Vec::new(),
        nbc_reason,
        prep: if asked {
            Prep::NotRequested
        } else {
            Prep::Skipped
        },
        callers: Vec::new(),
        callees: Vec::new(),
        decision: Decision::Pending,
        summary: None,
        dive: Default::default(),
        dive_pending: Default::default(),
        checks: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file(path: &str, status: FileStatus, old: Option<&str>, new: Option<&str>) -> ChangedFile {
        ChangedFile {
            path: PathBuf::from(path),
            old_path: None,
            status,
            old_content: old.map(String::from),
            new_content: new.map(String::from),
        }
    }

    #[test]
    fn lock_files_and_snapshots_are_named() {
        for (path, want) in [
            ("package-lock.json", Some("lock file")),
            ("app/yarn.lock", Some("lock file")),
            ("Cargo.lock", Some("lock file")),
            ("go.sum", Some("lock file")),
            ("tests/snapshots/tui__slim.snap", Some("test snapshot")),
            (
                "test/cli/__snapshots__/help.test.ts.snap",
                Some("test snapshot"),
            ),
            ("src/__snapshots__/a.txt", Some("test snapshot")),
            ("README.md", None),
            ("package.json", None),
        ] {
            assert_eq!(generated(Path::new(path)), want, "{path}");
        }
    }

    #[test]
    fn file_types_read_as_words() {
        assert_eq!(file_type(Path::new("README.md")), "Markdown file");
        assert_eq!(file_type(Path::new("a/b.YML")), "YAML file");
        assert_eq!(file_type(Path::new("Dockerfile")), "Dockerfile");
        assert_eq!(file_type(Path::new("run.sh")), "shell script");
        assert_eq!(file_type(Path::new("LICENSE")), "text file");
        assert_eq!(file_type(Path::new("x.weird")), ".weird file");
        assert_eq!(file_type(Path::new("yarn.lock")), "lock file");
    }

    #[test]
    fn whole_file_units_for_each_status() {
        let added = unit(&file("a.md", FileStatus::Added, None, Some("x\ny\n"))).unwrap();
        assert_eq!(added.status, UnitStatus::Added);
        assert_eq!(added.new_range, Some(Range::new(1, 2)));
        assert_eq!(added.old_range, None);
        assert_eq!(added.prep, Prep::NotRequested);
        assert_eq!(added.id, "a.md::(whole file)");

        let gone = unit(&file("a.md", FileStatus::Deleted, Some("x\n"), None)).unwrap();
        assert_eq!(gone.status, UnitStatus::Removed);
        assert_eq!(gone.display_line(), 1);

        let same = file("a.md", FileStatus::Modified, Some("x\n"), Some("x\n"));
        assert!(unit(&same).is_none(), "a mode change has no unit");

        let mut moved = file("b.md", FileStatus::Renamed, Some("x\n"), Some("x\n"));
        moved.old_path = Some(PathBuf::from("a.md"));
        let moved = unit(&moved).unwrap();
        assert_eq!(moved.status, UnitStatus::NoBehaviorChange);
        assert_eq!(moved.nbc_reason.as_deref(), Some("renamed from a.md"));

        let lock = unit(&file(
            "Cargo.lock",
            FileStatus::Modified,
            Some("a\n"),
            Some("b\n"),
        ));
        assert_eq!(lock.unwrap().prep, Prep::Skipped);

        let big: String = (0..400).map(|i| format!("{i}\n")).collect();
        let big = unit(&file("data.json", FileStatus::Added, None, Some(&big))).unwrap();
        assert_eq!(big.prep, Prep::Skipped, "too large to describe");
        let minified = format!("{{{}}}\n", "\"k\": 1, ".repeat(5000));
        let minified = file("app.min.json", FileStatus::Added, None, Some(&minified));
        assert_eq!(
            unit(&minified).unwrap().prep,
            Prep::Skipped,
            "one huge line"
        );
    }
}
