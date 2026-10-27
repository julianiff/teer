//! Running the GitHub CLI: which program, for how long, and what it printed.
//! Prompts and colours are off, and nothing runs a shell.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::approve::tidy_output;

/// How long one `gh` run may take.
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// The `gh` program teer runs: `$TEER_GH`, else `gh` (tests point it at a
/// script, or at nothing).
pub fn program() -> OsString {
    std::env::var_os("TEER_GH")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "gh".into())
}

/// What a `gh` run that finished printed.
pub struct Output {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Why a `gh` run didn't finish.
#[derive(Debug)]
pub enum Error {
    /// `gh` isn't on PATH.
    NotInstalled,
    /// It couldn't start or ran longer than [`TIMEOUT`]: why, tidied.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotInstalled => f.write_str("gh is not installed (not found on PATH)"),
            Error::Failed(why) => f.write_str(why),
        }
    }
}

/// Run `gh args` in `cwd` with `stdin` written to it (closed at once when
/// `None`), killed after [`TIMEOUT`].
pub async fn run<S: AsRef<OsStr>>(
    gh: &OsStr,
    cwd: &Path,
    args: &[S],
    stdin: Option<&str>,
) -> Result<Output, Error> {
    let failed = |e: std::io::Error| {
        Error::Failed(format!("couldn't run gh: {}", tidy_output(&e.to_string())))
    };
    let mut child = tokio::process::Command::new(gh)
        .args(args)
        .current_dir(cwd)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotInstalled,
            _ => failed(e),
        })?;
    let mut pipe = child.stdin.take();
    let payload = stdin.unwrap_or_default().as_bytes().to_vec();
    let job = async move {
        if let Some(pipe) = pipe.as_mut() {
            // A gh that exits early closes the pipe: not an error here.
            let _ = pipe.write_all(&payload).await;
        }
        drop(pipe);
        child.wait_with_output().await
    };
    match tokio::time::timeout(TIMEOUT, job).await {
        Err(_) => Err(Error::Failed(format!(
            "gh timed out after {}s",
            TIMEOUT.as_secs()
        ))),
        Ok(Err(e)) => Err(failed(e)),
        Ok(Ok(o)) => Ok(Output {
            ok: o.status.success(),
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }),
    }
}
