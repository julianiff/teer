//! An app started from Finder or the Dock gets the system's PATH only
//! (`/usr/bin:/bin:/usr/sbin:/sbin`): no `gh`, `claude` or `codex` from
//! Homebrew, `~/.local/bin` or cargo. At start, before anything runs, the
//! app takes the environment of the user's login shell: PATH, and the
//! variables the shell sets that the app lacks (an API key exported in
//! `.zshrc`). Login and interactive, so a zsh config under ZDOTDIR and
//! `.zshrc` both count. Started from a terminal, the app has all of it.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long the shell may take to say its environment.
const TIMEOUT: Duration = Duration::from_secs(5);

/// What comes before the environment in the shell's output: a shell's
/// greeting or a prompt that prints stays out of it.
const MARK: &str = "__TEER_ENV__";

/// Variables the app keeps its own of.
const OWN: &[&str] = &[
    "PWD",
    "OLDPWD",
    "SHLVL",
    "_",
    "TERM_SESSION_ID",
    "__CF_BUNDLE_IDENTIFIER",
];

/// Take the login shell's environment, unless the app was started from a
/// terminal (it has a TERM then, and everything with it).
pub fn import() {
    if std::env::var_os("TERM").is_some() {
        return;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let Some(out) = run(&shell) else {
        return;
    };
    let Some(vars) = parse(&out) else {
        return;
    };
    for (key, value) in vars {
        if OWN.contains(&key.as_str()) {
            continue;
        }
        if key == "PATH" || std::env::var_os(&key).is_none() {
            // SAFETY: at start, before the app runs any other thread.
            unsafe { std::env::set_var(&key, value) };
        }
    }
}

/// What `shell -l -i -c` prints: the mark, then `env -0`.
fn run(shell: &str) -> Option<Vec<u8>> {
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &format!("printf '{MARK}'; env -0")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

/// The variables in `out`, after the mark; `None` without it.
fn parse(out: &[u8]) -> Option<HashMap<String, String>> {
    let text = String::from_utf8_lossy(out);
    let (_, env) = text.split_once(MARK)?;
    Some(
        env.split('\0')
            .filter_map(|kv| kv.split_once('='))
            .filter(|(k, _)| !k.is_empty() && !k.contains('\n'))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_comes_after_the_mark() {
        let out = b"Welcome!\n__TEER_ENV__PATH=/opt/homebrew/bin:/usr/bin\0KEY=a=b\0EMPTY=\0";
        let vars = parse(out).unwrap();
        assert_eq!(vars["PATH"], "/opt/homebrew/bin:/usr/bin");
        assert_eq!(vars["KEY"], "a=b");
        assert_eq!(vars["EMPTY"], "");
        assert!(parse(b"PATH=/x\0").is_none());
    }

    #[test]
    fn a_real_shell_says_its_path() {
        let Some(out) = run("/bin/sh") else {
            panic!("sh ran");
        };
        let vars = parse(&out).expect("the mark");
        assert!(vars.contains_key("PATH"), "{vars:?}");
    }
}
