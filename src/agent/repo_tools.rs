//! The read-only tools teer gives a model API so it can look around the
//! repository (Claude Code and Codex bring their own): list files, read a
//! file, search with a regular expression. Nothing leaves the checkout
//! under review: paths are relative to its root, `..` and `.git` are
//! refused, and a symlink that leads out of it is not followed.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use globset::{Glob, GlobMatcher};
use serde_json::{Value, json};

/// File paths `list_files` returns at most.
const MAX_LISTED: usize = 400;
/// Lines `read_file` returns by default and at most.
const READ_LINES: usize = 400;
const MAX_READ_LINES: usize = 1000;
/// Matches `grep` returns at most.
const MAX_MATCHES: usize = 200;
/// Characters of one line shown, by `read_file` and `grep`.
const LINE_CHARS: usize = 400;
/// Files larger than this are not searched.
const MAX_GREP_BYTES: u64 = 1024 * 1024;

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_files",
            description: "List the repository's files (tracked and new, ignored ones left out), paths relative to its root. \
`pattern` is a glob such as `src/**/*.rs`; one without `/` matches file names anywhere (`*.test.ts`).",
            schema: json!({
                "type": "object",
                "properties": { "pattern": { "type": "string", "description": "Glob; leave out for every file" } }
            }),
        },
        Tool {
            name: "read_file",
            description: "Read a file of the repository with line numbers, up to 400 lines from `offset`.",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative to the repository root" },
                    "offset": { "type": "integer", "description": "First line, from 1" },
                    "limit": { "type": "integer", "description": "How many lines (at most 1000)" }
                },
                "required": ["path"]
            }),
        },
        Tool {
            name: "grep",
            description: "Search the repository's files for a regular expression (Rust syntax); returns `path:line: text`.",
            schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "glob": { "type": "string", "description": "Only files matching this glob" },
                    "ignore_case": { "type": "boolean" }
                },
                "required": ["pattern"]
            }),
        },
    ]
}

/// Run tool `name` in the checkout at `root`. `Err` is a message for the
/// model (a bad path, no such file), sent back as a failed tool result.
pub fn run(root: &Path, name: &str, input: &Value) -> Result<String, String> {
    let s = |k: &str| input.get(k).and_then(Value::as_str);
    let n = |k: &str| input.get(k).and_then(Value::as_u64).map(|v| v as usize);
    match name {
        "list_files" => list(root, s("pattern")),
        "read_file" => read(
            root,
            s("path").ok_or("`path` is missing")?,
            n("offset"),
            n("limit"),
        ),
        "grep" => grep(
            root,
            s("pattern").ok_or("`pattern` is missing")?,
            s("glob"),
            input
                .get("ignore_case")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
        other => Err(format!("there is no tool `{other}`")),
    }
}

/// The repository's files: tracked and untracked ones git doesn't ignore;
/// every file under `root` when it isn't a git checkout.
fn files(root: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output();
    if let Ok(o) = out
        && o.status.success()
    {
        let mut v: Vec<String> = String::from_utf8_lossy(&o.stdout)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(String::from)
            .collect();
        v.sort();
        v.dedup();
        return v;
    }
    let mut v = Vec::new();
    walk(root, root, &mut v);
    v.sort();
    v
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(ty) = e.file_type() else { continue };
        if e.file_name() == ".git" {
            continue;
        }
        if ty.is_dir() {
            walk(root, &path, out);
        } else if ty.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.to_string_lossy().into_owned());
        }
    }
}

/// A glob as the tools take it: one without `/` matches file names.
struct Pattern {
    m: GlobMatcher,
    name_only: bool,
}

impl Pattern {
    fn new(glob: &str) -> Result<Pattern, String> {
        let glob = glob.trim().trim_start_matches("./");
        let m = globset::GlobBuilder::new(glob)
            .literal_separator(true)
            .build()
            .map(|g: Glob| g.compile_matcher())
            .map_err(|e| format!("bad glob `{glob}`: {e}"))?;
        Ok(Pattern {
            m,
            name_only: !glob.contains('/'),
        })
    }

    fn matches(&self, path: &str) -> bool {
        if self.name_only {
            let name = path.rsplit('/').next().unwrap_or(path);
            self.m.is_match(name)
        } else {
            self.m.is_match(path)
        }
    }
}

fn list(root: &Path, pattern: Option<&str>) -> Result<String, String> {
    let pat = pattern
        .filter(|p| !p.trim().is_empty())
        .map(Pattern::new)
        .transpose()?;
    let hits: Vec<String> = files(root)
        .into_iter()
        .filter(|f| pat.as_ref().is_none_or(|p| p.matches(f)))
        .collect();
    if hits.is_empty() {
        return Ok("no files match".into());
    }
    let mut out = hits
        .iter()
        .take(MAX_LISTED)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    if hits.len() > MAX_LISTED {
        out.push_str(&format!(
            "\n… {} more; narrow the pattern",
            hits.len() - MAX_LISTED
        ));
    }
    Ok(out)
}

/// `rel` inside `root`, refused when it leaves the checkout or names `.git`.
pub fn inside(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim().trim_start_matches("./");
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err("paths are relative to the repository root".into());
    }
    for c in p.components() {
        match c {
            Component::Normal(s) if s == ".git" => {
                return Err("`.git` is not part of the review".into());
            }
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err("`..` is not allowed: stay inside the repository".into()),
        }
    }
    let base = root
        .canonicalize()
        .map_err(|e| format!("the repository is gone: {e}"))?;
    let full = base
        .join(p)
        .canonicalize()
        .map_err(|_| format!("{rel}: no such file"))?;
    let within = full
        .strip_prefix(&base)
        .map_err(|_| format!("{rel} leads outside the repository"))?;
    if within.components().any(|c| c.as_os_str() == ".git") {
        return Err("`.git` is not part of the review".into());
    }
    Ok(full)
}

fn clip(line: &str) -> String {
    match line.char_indices().nth(LINE_CHARS) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

fn read(
    root: &Path,
    rel: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, String> {
    let path = inside(root, rel)?;
    if path.is_dir() {
        return Err(format!(
            "{rel} is a directory; list_files shows what is in it"
        ));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("{rel}: {e}"))?;
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err(format!("{rel} is a binary file"));
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = offset.unwrap_or(1).max(1);
    let take = limit.unwrap_or(READ_LINES).clamp(1, MAX_READ_LINES);
    if lines.is_empty() {
        return Ok(format!("{rel} is empty"));
    }
    if start > lines.len() {
        return Err(format!("{rel} has {} lines", lines.len()));
    }
    let end = (start - 1 + take).min(lines.len());
    let mut out: Vec<String> = (start..=end)
        .map(|n| format!("{n:>6}\t{}", clip(lines[n - 1])))
        .collect();
    if end < lines.len() {
        out.push(format!(
            "(lines {start}-{end} of {}; read on with offset {})",
            lines.len(),
            end + 1
        ));
    }
    Ok(out.join("\n"))
}

fn grep(
    root: &Path,
    pattern: &str,
    glob: Option<&str>,
    ignore_case: bool,
) -> Result<String, String> {
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| format!("bad pattern: {e}"))?;
    let pat = glob
        .filter(|g| !g.trim().is_empty())
        .map(Pattern::new)
        .transpose()?;
    let mut hits = Vec::new();
    let mut more = 0;
    for f in files(root) {
        if pat.as_ref().is_some_and(|p| !p.matches(&f)) {
            continue;
        }
        let Ok(path) = inside(root, &f) else { continue };
        if std::fs::metadata(&path).map_or(true, |m| !m.is_file() || m.len() > MAX_GREP_BYTES) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.iter().take(8192).any(|&b| b == 0) {
            continue;
        }
        for (i, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
            if re.is_match(line) {
                if hits.len() < MAX_MATCHES {
                    hits.push(format!("{f}:{}: {}", i + 1, clip(line.trim_end())));
                } else {
                    more += 1;
                }
            }
        }
    }
    if hits.is_empty() {
        return Ok("no matches".into());
    }
    let mut out = hits.join("\n");
    if more > 0 {
        out.push_str(&format!(
            "\n… {more} more matches; narrow the pattern or the glob"
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        std::fs::create_dir_all(p.join("src/deep")).unwrap();
        std::fs::write(p.join("src/main.rs"), "fn main() {\n    run();\n}\n").unwrap();
        std::fs::write(p.join("src/deep/run.rs"), "pub fn run() {}\n").unwrap();
        std::fs::write(p.join("README.md"), "# demo\n").unwrap();
        std::fs::create_dir_all(p.join(".git")).unwrap();
        std::fs::write(
            p.join(".git/config"),
            "[remote] url = https://token@example.com\n",
        )
        .unwrap();
        d
    }

    #[test]
    fn lists_files_by_glob_and_by_name() {
        let d = repo();
        assert_eq!(
            run(d.path(), "list_files", &json!({})).unwrap(),
            "README.md\nsrc/deep/run.rs\nsrc/main.rs"
        );
        assert_eq!(
            run(d.path(), "list_files", &json!({"pattern": "*.rs"})).unwrap(),
            "src/deep/run.rs\nsrc/main.rs"
        );
        assert_eq!(
            run(d.path(), "list_files", &json!({"pattern": "src/*.rs"})).unwrap(),
            "src/main.rs"
        );
        assert_eq!(
            run(d.path(), "list_files", &json!({"pattern": "src/**/*.rs"})).unwrap(),
            "src/deep/run.rs\nsrc/main.rs"
        );
    }

    #[test]
    fn reads_numbered_lines_from_an_offset() {
        let d = repo();
        let out = run(
            d.path(),
            "read_file",
            &json!({"path": "src/main.rs", "offset": 2, "limit": 1}),
        )
        .unwrap();
        assert_eq!(
            out,
            "     2\t    run();\n(lines 2-2 of 3; read on with offset 3)"
        );
    }

    #[test]
    fn greps_with_line_numbers() {
        let d = repo();
        let out = run(
            d.path(),
            "grep",
            &json!({"pattern": "fn run", "glob": "*.rs"}),
        )
        .unwrap();
        assert_eq!(out, "src/deep/run.rs:1: pub fn run() {}");
        assert_eq!(
            run(d.path(), "grep", &json!({"pattern": "nothing here"})).unwrap(),
            "no matches"
        );
    }

    #[test]
    fn nothing_outside_the_checkout_or_in_git_is_read() {
        let d = repo();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "key").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("secret"), d.path().join("link")).unwrap();
        for path in [
            "/etc/hosts",
            "../secret",
            "src/../../secret",
            ".git/config",
            "src/../.git/config",
            "link",
        ] {
            let r = run(d.path(), "read_file", &json!({ "path": path }));
            assert!(r.is_err(), "{path}: {r:?}");
        }
        // grep skips what it can't read instead of failing.
        assert_eq!(
            run(d.path(), "grep", &json!({"pattern": "key|token"})).unwrap(),
            "no matches"
        );
    }
}
