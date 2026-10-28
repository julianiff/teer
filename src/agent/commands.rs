//! Every external command line teer runs, in one place so they are easy to
//! update when a CLI changes. Flags were checked against `claude --help`
//! and live probes on Claude Code 2.1.291, and against `codex exec --help`
//! and live probes on Codex 0.159.2.

use std::path::Path;

use super::PrepareModel;

pub const CLAUDE_BIN: &str = "claude";
pub const CODEX_BIN: &str = "codex";

/// Tools a review run may use. It can read the repo but must not change it.
const READ_ONLY_TOOLS: &str = "Read,Grep,Glob";

/// `dontAsk` denies every tool call that is not pre-approved, and (verified
/// with a live probe) reads outside the working directory, including through
/// a symlink inside it, are not pre-approved. That is what keeps the agent
/// from reading `~/.ssh`, `~/.aws`, `/etc`, ... so no blanket `Read` allow rule
/// is ever passed.
const PERMISSION_MODE: &str = "dontAsk";

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// Describing units: a fresh run (no `--resume`) with no tools at all, since
/// the prompt carries the code, on the preparation model and effort when set.
/// No tool means one model turn, which is what makes it fast.
pub fn claude_prepare(prep: &PrepareModel, schema_json: &str) -> Vec<String> {
    let mut a = s(&["-p", "--no-session-persistence", "--tools", ""]);
    a.extend(s(&["--permission-mode", PERMISSION_MODE]));
    if let Some(m) = &prep.model {
        a.extend(["--model".to_string(), m.clone()]);
    }
    if let Some(e) = &prep.effort {
        a.extend(["--effort".to_string(), e.clone()]);
    }
    a.extend(s(&[
        "--output-format",
        "json",
        "--json-schema",
        schema_json,
    ]));
    a
}

/// A question with a structured answer: a plain headless `claude -p` in the
/// repo, or a fork of `session` when one was given (so it knows why the
/// changes were made). `--no-session-persistence` keeps either out of the
/// session list (verified: no transcript is written). The prompt goes to
/// stdin and the answer is in `structured_output` of the JSON result.
/// Tools: the read-only repo tools (nothing that runs commands: no Bash),
/// and the web tools only when `web`. `--tools` limits what exists;
/// `--allowedTools` pre-approves only the web tools, so file tools stay
/// confined to the working directory by `--permission-mode dontAsk`.
pub fn claude_dive(session: Option<&str>, schema_json: &str, web: bool) -> Vec<String> {
    let mut tools = String::from(READ_ONLY_TOOLS);
    if web {
        tools.push_str(",WebSearch,WebFetch");
    }
    let mut a = s(&["-p"]);
    if let Some(id) = session {
        a.extend(s(&["--resume", id, "--fork-session"]));
    }
    a.extend(s(&["--no-session-persistence", "--tools", &tools]));
    if web {
        a.extend(s(&["--allowedTools", "WebSearch,WebFetch"]));
    }
    a.extend(s(&[
        "--permission-mode",
        PERMISSION_MODE,
        "--output-format",
        "json",
        "--json-schema",
        schema_json,
    ]));
    a
}

/// Extra flags when the code under review is untrusted (a pull request).
/// `--safe-mode` turns off CLAUDE.md, skills, custom agents and commands, rules,
/// hooks, plugins and MCP servers (verified: a CLAUDE.md in the working
/// directory is not loaded); it does not by itself ignore the project's
/// `.claude/settings.json` (verified: a `deny Read` rule there still applied),
/// so `--setting-sources user` stays, and `--strict-mcp-config` skips `.mcp.json`.
pub fn untrusted_flags() -> Vec<String> {
    s(&[
        "--safe-mode",
        "--setting-sources",
        "user",
        "--strict-mcp-config",
    ])
}

/// A headless Codex run: `codex exec` in `cwd` with the prompt on stdin
/// (`-`), the answer held to the schema in `schema_file` and written to
/// `out_file`. `--sandbox read-only` lets its commands read but not write
/// or reach the network; unlike Claude Code's read tools that sandbox reads
/// the whole disk, not only `cwd`. `--ephemeral` keeps the run out of the
/// session list. `model` empty leaves Codex's own model and effort.
pub fn codex_exec(
    cwd: &Path,
    model: &PrepareModel,
    schema_file: &Path,
    out_file: &Path,
    untrusted: bool,
) -> Vec<String> {
    let mut a = s(&[
        "exec",
        "--sandbox",
        "read-only",
        "--ephemeral",
        "--skip-git-repo-check",
    ]);
    a.extend(s(&["--color", "never", "-C"]));
    a.push(cwd.to_string_lossy().into_owned());
    if let Some(m) = &model.model {
        a.extend(["-m".to_string(), m.clone()]);
    }
    if let Some(e) = &model.effort {
        a.extend(["-c".to_string(), format!("model_reasoning_effort=\"{e}\"")]);
    }
    if untrusted {
        // The pull request's own AGENTS.md and skills would reach the
        // prompt as instructions (verified with `codex debug prompt-input`);
        // its exec rules could allow commands.
        a.extend(s(&[
            "-c",
            "project_doc_max_bytes=0",
            "-c",
            "skills.include_instructions=false",
            "--ignore-rules",
        ]));
    }
    a.push("--output-schema".into());
    a.push(schema_file.to_string_lossy().into_owned());
    a.push("-o".into());
    a.push(out_file.to_string_lossy().into_owned());
    a.push("-".into());
    a
}

/// Codex's model catalog as JSON (`models[].slug`, `visibility`, `priority`).
pub fn codex_models() -> (&'static str, Vec<String>) {
    (CODEX_BIN, s(&["debug", "models"]))
}

/// All processes with their parent and full command line.
pub fn ps_all() -> (&'static str, Vec<String>) {
    ("ps", s(&["-axo", "pid=,ppid=,args="]))
}

/// Working directories of the given processes (macOS and other non-procfs systems).
pub fn lsof_cwd(pids: &str) -> (&'static str, Vec<String>) {
    ("lsof", s(&["-a", "-p", pids, "-d", "cwd", "-Fpn"]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_command_line_is_fresh_toolless_and_names_the_model() {
        let fast = PrepareModel {
            model: Some("haiku".into()),
            effort: Some("low".into()),
        };
        let a = claude_prepare(&fast, "{}");
        assert_eq!(
            a,
            [
                "-p",
                "--no-session-persistence",
                "--tools",
                "",
                "--permission-mode",
                "dontAsk",
                "--model",
                "haiku",
                "--effort",
                "low",
                "--output-format",
                "json",
                "--json-schema",
                "{}"
            ]
        );
        // Never a fork of the session, and no tool at all.
        assert!(!a.iter().any(|x| x == "--resume" || x == "--fork-session"));
        for tool in ["Read", "Grep", "Glob", "Bash", "Edit", "Write", "Web"] {
            assert!(!a.iter().any(|x| x.contains(tool)), "{tool}");
        }
        // Claude Code's own default model and effort when unset.
        let plain = claude_prepare(&PrepareModel::default(), "{}").join(" ");
        assert_eq!(
            plain,
            "-p --no-session-persistence --tools  --permission-mode dontAsk --output-format json --json-schema {}"
        );
    }

    #[test]
    fn codex_runs_read_only_and_leaves_the_prs_instructions_out() {
        let fast = PrepareModel {
            model: Some("gpt-6.1-terra".into()),
            effort: Some("low".into()),
        };
        let a = codex_exec(
            Path::new("/r"),
            &fast,
            Path::new("/t/s.json"),
            Path::new("/t/o.txt"),
            false,
        );
        assert_eq!(
            a.join(" "),
            "exec --sandbox read-only --ephemeral --skip-git-repo-check --color never -C /r \
-m gpt-6.1-terra -c model_reasoning_effort=\"low\" --output-schema /t/s.json -o /t/o.txt -"
        );
        let pr = codex_exec(
            Path::new("/r"),
            &PrepareModel::default(),
            Path::new("s"),
            Path::new("o"),
            true,
        )
        .join(" ");
        assert!(!pr.contains(" -m ") && !pr.contains("model_reasoning_effort"));
        assert!(pr.contains(
            "-c project_doc_max_bytes=0 -c skills.include_instructions=false --ignore-rules"
        ));
        assert!(!pr.contains("workspace-write") && !pr.contains("danger"));
    }

    #[test]
    fn untrusted_flags_load_no_repo_customisation() {
        assert_eq!(
            untrusted_flags().join(" "),
            "--safe-mode --setting-sources user --strict-mcp-config"
        );
    }

    #[test]
    fn dive_command_lines_cannot_run_commands_or_read_everything() {
        let off = claude_dive(None, "{}", false);
        let off_s = off.join(" ");
        assert_eq!(
            off_s,
            "-p --no-session-persistence --tools Read,Grep,Glob --permission-mode dontAsk --output-format json --json-schema {}"
        );
        // No Bash, nothing pre-approved: reads stay inside the working directory.
        assert!(!off.iter().any(|a| a.contains("Bash")));
        assert!(!off.iter().any(|a| a == "--allowedTools"));
        let on = claude_dive(Some("abc"), "{}", true);
        let on_s = on.join(" ");
        assert!(on_s.starts_with("-p --resume abc --fork-session --no-session-persistence "));
        assert!(on_s.contains("--tools Read,Grep,Glob,WebSearch,WebFetch "));
        assert!(on_s.contains("--allowedTools WebSearch,WebFetch "));
        assert!(on_s.contains("--permission-mode dontAsk"));
        assert!(!on_s.contains("Bash") && !on_s.contains("Edit") && !on_s.contains("Write"));
    }
}
