//! Watching a run while it answers (see [`super::AgentAdapter::ask_live`]):
//! the steps the agent takes and, where the provider streams it, the answer
//! as it is written.

use std::path::Path;

use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

/// What a run is doing, as it happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Live {
    /// A step the agent took, in a few words: `read src/cart.rs`.
    Step(String),
    /// The answer as far as it has come: the start of its JSON.
    Partial(String),
}

pub type LiveTx = UnboundedSender<Live>;

/// Longest step, in columns.
const STEP_MAX: usize = 120;

/// A tool call as a step: what it reads or searches, paths relative to
/// `root`. Claude Code's tools and teer's own (`super::repo_tools`); `None`
/// for Claude Code's `StructuredOutput`, which is the answer itself.
pub fn tool_step(name: &str, input: &Value, root: &Path) -> Option<String> {
    let s = |k: &str| input[k].as_str().unwrap_or("");
    let path = |k: &str| relative(s(k), root);
    let step = match name {
        "StructuredOutput" => return None,
        "Read" => format!("read {}", path("file_path")),
        "read_file" => format!("read {}", path("path")),
        "Grep" | "grep" => {
            let within = match s("glob") {
                "" => path("path"),
                g => g.to_string(),
            };
            match within.as_str() {
                "" => format!("search “{}”", s("pattern")),
                w => format!("search “{}” in {w}", s("pattern")),
            }
        }
        "Glob" => format!("find {}", s("pattern")),
        "list_files" => match s("pattern") {
            "" => "list the files".into(),
            p => format!("find {p}"),
        },
        "WebSearch" => format!("search the web for “{}”", s("query")),
        "WebFetch" => format!("fetch {}", s("url")),
        other => other.to_string(),
    };
    Some(crate::util::truncate(&step, STEP_MAX))
}

/// A command Codex runs as a step, without the shell around it:
/// `/bin/zsh -lc "rg -n foo ."` is `run rg -n foo .`.
pub fn command_step(command: &str) -> String {
    let inner = shlex::split(command)
        .filter(|w| w.len() == 3 && matches!(w[1].as_str(), "-c" | "-lc"))
        .map(|mut w| w.remove(2));
    let step = format!("run {}", inner.as_deref().unwrap_or(command));
    crate::util::truncate(&step, STEP_MAX)
}

/// `p` relative to `root`; empty for `root` itself.
fn relative(p: &str, root: &Path) -> String {
    match Path::new(p).strip_prefix(root) {
        Ok(r) => r.display().to_string(),
        Err(_) => p.trim_start_matches("./").to_string(),
    }
}

/// String `key` of the JSON object that `json` starts, as far as it has
/// come: `{"text": "Retries are do` gives `Retries are do`. `None` until
/// that value has begun. Text before the object (a fence, a sentence) is
/// skipped.
pub fn partial_string(json: &str, key: &str) -> Option<String> {
    let start = json.find('{')?;
    let mut it = json[start + 1..].chars();
    let mut depth = 1usize;
    let mut expect_key = true;
    let mut current: Option<String> = None;
    while let Some(c) = it.next() {
        match c {
            '"' => {
                let (s, closed) = read_string(&mut it);
                if depth == 1 && expect_key {
                    if !closed {
                        return None;
                    }
                    current = Some(s);
                    expect_key = false;
                } else if depth == 1 && current.as_deref() == Some(key) {
                    return Some(s);
                } else if !closed {
                    return None;
                }
            }
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return None;
                }
            }
            ',' if depth == 1 => {
                expect_key = true;
                current = None;
            }
            _ => {}
        }
    }
    None
}

/// The rest of a JSON string after its opening quote, decoded, and whether
/// it closed. An escape cut off at the end is left out.
fn read_string(it: &mut std::str::Chars) -> (String, bool) {
    let mut out = String::new();
    while let Some(c) = it.next() {
        match c {
            '"' => return (out, true),
            '\\' => {
                let Some(e) = it.next() else { break };
                match e {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'u' => {
                        let Some(hi) = hex4(it) else { break };
                        if (0xD800..0xDC00).contains(&hi) {
                            // A surrogate pair: `😀`.
                            if it.next() != Some('\\') || it.next() != Some('u') {
                                break;
                            }
                            let Some(lo) = hex4(it) else { break };
                            let c =
                                0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                            out.extend(char::from_u32(c));
                        } else {
                            out.extend(char::from_u32(hi));
                        }
                    }
                    // `\"`, `\\`, `\/`
                    other => out.push(other),
                }
            }
            c => out.push(c),
        }
    }
    (out, false)
}

fn hex4(it: &mut std::str::Chars) -> Option<u32> {
    let mut v = 0;
    for _ in 0..4 {
        v = v * 16 + it.next()?.to_digit(16)?;
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_answers_text_grows_as_its_json_comes() {
        let full = r#"{"text": "Retries \"back off\"\n twice ✓ 😀", "show": []}"#;
        let mut seen = Vec::new();
        for end in (0..=full.len()).filter(|&i| full.is_char_boundary(i)) {
            if let Some(t) = partial_string(&full[..end], "text") {
                seen.push(t);
            }
        }
        // Nothing until the value begins, then only ever longer.
        assert_eq!(seen[0], "");
        assert!(seen.windows(2).all(|w| w[1].starts_with(&w[0])), "{seen:?}");
        assert_eq!(seen.last().unwrap(), "Retries \"back off\"\n twice ✓ 😀");
        assert_eq!(partial_string(r#"{"te"#, "text"), None);
        assert_eq!(partial_string(r#"{"text"#, "text"), None);
        assert_eq!(
            partial_string(r#"{"text": "a\"#, "text").as_deref(),
            Some("a")
        );
        assert_eq!(
            partial_string(r#"{"text": "a\u00"#, "text").as_deref(),
            Some("a")
        );
    }

    #[test]
    fn only_the_top_level_key_counts_wherever_it_is() {
        let j = r#"Here: ```json
{"show": [{"text": "no", "why": "x"}], "note": "text", "text": "yes"#;
        assert_eq!(partial_string(j, "text").as_deref(), Some("yes"));
        assert_eq!(partial_string(r#"{"show": [{"text": "no"#, "text"), None);
        assert_eq!(partial_string(r#"{"text": 3, "b": "x"}"#, "text"), None);
        assert_eq!(partial_string("I'll look at {it}. {", "text"), None);
        assert_eq!(partial_string("no JSON", "text"), None);
    }

    #[test]
    fn tool_calls_read_as_steps_with_paths_in_the_repo() {
        let root = Path::new("/r");
        let step = |name: &str, input: Value| tool_step(name, &input, root);
        assert_eq!(
            step("Read", json!({"file_path": "/r/src/a.rs"})).unwrap(),
            "read src/a.rs"
        );
        assert_eq!(
            step("read_file", json!({"path": "src/a.rs"})).unwrap(),
            "read src/a.rs"
        );
        assert_eq!(
            step("Grep", json!({"pattern": "retry", "path": "/r"})).unwrap(),
            "search “retry”"
        );
        assert_eq!(
            step("Grep", json!({"pattern": "retry", "path": "/r/src"})).unwrap(),
            "search “retry” in src"
        );
        assert_eq!(
            step("grep", json!({"pattern": "x", "glob": "*.rs"})).unwrap(),
            "search “x” in *.rs"
        );
        assert_eq!(step("list_files", json!({})).unwrap(), "list the files");
        assert_eq!(
            step("WebFetch", json!({"url": "https://x.dev"})).unwrap(),
            "fetch https://x.dev"
        );
        assert_eq!(step("StructuredOutput", json!({"text": "hi"})), None);
        // A control character from the model can't reach the terminal.
        assert_eq!(
            step("Glob", json!({"pattern": "a\u{1b}[2Jb"})).unwrap(),
            "find a [2Jb"
        );
        let long = step("Glob", json!({"pattern": "x".repeat(300)})).unwrap();
        assert!(long.chars().count() <= STEP_MAX && long.ends_with('…'));
    }

    #[test]
    fn codex_commands_lose_their_shell() {
        assert_eq!(
            command_step(r#"/bin/zsh -lc "rg -n 'trait\\s+AgentAdapter\\b' .""#),
            r"run rg -n 'trait\s+AgentAdapter\b' ."
        );
        assert_eq!(command_step("ls src"), "run ls src");
    }
}
