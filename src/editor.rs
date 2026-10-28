//! The editor teer hands a file to (`editor` in the config): a command whose
//! words may hold `{file}` and `{line}`, filled in per run. It is split like
//! a shell would (quotes keep spaces) but no shell runs it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

/// Neovim at the line.
pub const DEFAULT: &str = "nvim +{line} {file}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editor {
    words: Vec<String>,
}

impl Default for Editor {
    fn default() -> Self {
        Editor::parse(DEFAULT).expect("the default editor parses")
    }
}

/// Just a program: the file goes last (tests point it at a script).
impl From<PathBuf> for Editor {
    fn from(p: PathBuf) -> Self {
        Editor {
            words: vec![p.to_string_lossy().into_owned()],
        }
    }
}

impl Editor {
    pub fn parse(cmd: &str) -> Result<Editor> {
        let Some(words) = shlex::split(cmd) else {
            bail!("editor = {cmd:?}: a quote isn't closed");
        };
        if words.is_empty() {
            bail!("editor is empty: name a command, such as {DEFAULT:?}");
        }
        Ok(Editor { words })
    }

    /// The program as written: `nvim`, `/usr/local/bin/code`.
    pub fn program(&self) -> &str {
        &self.words[0]
    }

    /// The program's own name, for messages: `nvim`, `idea`.
    pub fn name(&self) -> String {
        Path::new(self.program())
            .file_name()
            .map_or_else(|| self.program().into(), |n| n.to_string_lossy().into())
    }

    /// The command that opens `file` at `line`. Without `{file}` in it, the
    /// file goes last.
    pub fn command(&self, file: &Path, line: u32) -> Command {
        let line = line.to_string();
        let path = file.to_string_lossy();
        let mut c = Command::new(self.program());
        c.args(self.words[1..].iter().map(|w| -> OsString {
            if w == "{file}" {
                // Kept as it is, even when it isn't UTF-8.
                file.into()
            } else {
                fill(w, &path, &line).into()
            }
        }));
        if !self.words[1..].iter().any(|w| w.contains("{file}")) {
            c.arg(file);
        }
        c
    }
}

/// `word` with `{file}` and `{line}` replaced, in one pass (a file name
/// holding `{line}` stays as it is).
fn fill(word: &str, file: &str, line: &str) -> String {
    let mut out = String::new();
    let mut rest = word;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("{file}") {
            out.push_str(file);
            rest = r;
        } else if let Some(r) = rest.strip_prefix("{line}") {
            out.push_str(line);
            rest = r;
        } else {
            out.push('{');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(cmd: &str, file: &str, line: u32) -> Vec<String> {
        let c = Editor::parse(cmd).unwrap().command(Path::new(file), line);
        std::iter::once(c.get_program())
            .chain(c.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_file_and_line_are_filled_in() {
        assert_eq!(argv(DEFAULT, "/r/a.rs", 12), ["nvim", "+12", "/r/a.rs"]);
        assert_eq!(
            argv("code --goto {file}:{line}", "/r/a b.rs", 3),
            ["code", "--goto", "/r/a b.rs:3"]
        );
        assert_eq!(
            argv(
                "'/Applications/IntelliJ IDEA.app/Contents/MacOS/idea' --line {line} {file}",
                "/r/a.rs",
                7
            ),
            [
                "/Applications/IntelliJ IDEA.app/Contents/MacOS/idea",
                "--line",
                "7",
                "/r/a.rs"
            ]
        );
    }

    #[test]
    fn without_file_it_goes_last() {
        assert_eq!(argv("subl -n", "/r/a.rs", 1), ["subl", "-n", "/r/a.rs"]);
        // Filled in once: a name holding a placeholder stays as it is.
        assert_eq!(argv("x {file}", "/r/{line}.rs", 4), ["x", "/r/{line}.rs"]);
        assert_eq!(argv("x {file}{", "/r/{line}", 4), ["x", "/r/{line}{"]);
    }

    #[test]
    fn names_and_bad_commands() {
        let idea = Editor::parse("\"/opt/IntelliJ IDEA/bin/idea\" {file}").unwrap();
        assert_eq!(idea.name(), "idea");
        assert_eq!(Editor::default().name(), "nvim");
        assert!(Editor::parse("  ").is_err());
        assert!(Editor::parse("code 'unclosed").is_err());
    }
}
