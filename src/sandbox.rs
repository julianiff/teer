//! Running code from the change under review, confined. This is the only
//! place teer runs repository code: the Behavior probe (see
//! `review::behavior`), on a checkout teer owns.
//!
//! macOS `sandbox-exec` with a profile that denies the network, Apple Events
//! and the keychain and pasteboard services; allows writes only in the
//! checkout and its cache folder; and denies reads in the home folder except
//! the checkout, its cache, linked dependency folders and language toolchains.
//! The environment is cleared down to a short list (no tokens or keys), the
//! command runs in its own process group, and the whole group is killed when
//! it ends or runs out of time. Other systems have no sandbox here, so
//! nothing runs there.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Most bytes kept of each output stream; a run that prints more is cut off.
pub const MAX_OUTPUT: u64 = 1024 * 1024;

/// Folders in the home folder that hold language toolchains (read only).
const TOOLCHAINS: &[&str] = &[
    ".cargo",
    ".rustup",
    ".nvm",
    ".volta",
    ".fnm",
    ".bun",
    ".deno",
    ".pyenv",
    ".rbenv",
    ".asdf",
    ".local/bin",
    ".local/lib",
    ".local/share/mise",
    ".local/share/fnm",
    ".local/share/uv",
    ".local/pipx",
    ".composer",
    ".config/composer",
    ".npm-global",
    ".pnpm-store",
    "Library/pnpm",
    "Library/Python",
];

/// Variables passed through from teer's own environment; everything else
/// (tokens, keys, proxies) is dropped.
const PASS_ENV: &[&str] = &[
    "PATH",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "USER",
    "LOGNAME",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "NVM_DIR",
    "NVM_BIN",
    "VOLTA_HOME",
    "PYENV_ROOT",
    "JAVA_HOME",
];

/// Later rules win: everything is allowed, then network, writes and home
/// reads are taken away, then the few allowed places are given back.
/// `(param ...)` values come from `-D`, so no path is ever spliced into the
/// profile text; `READS` becomes one `(subpath (param "R<n>"))` per place.
const PROFILE: &str = r#"(version 1)
(allow default)
(deny network*)
(deny appleevent-send)
(deny mach-lookup
  (global-name "com.apple.SecurityServer")
  (global-name "com.apple.securityd.xpc")
  (global-name "com.apple.security.agent")
  (global-name "com.apple.pasteboard.1"))
(deny file-write*)
(allow file-write*
  (subpath (param "DIR"))
  (subpath (param "CACHE"))
  (literal "/dev/null")
  (literal "/dev/zero")
  (literal "/dev/stdout")
  (literal "/dev/stderr")
  (regex #"^/dev/fd/"))
(deny file-read* (subpath (param "HOME")))
(allow file-read-metadata)
(allow file-read*
  (subpath (param "DIR"))
  (subpath (param "CACHE"))
  READS)
"#;

/// Where a confined command may work.
#[derive(Debug, Clone)]
pub struct Jail {
    /// The working directory; writable.
    pub dir: PathBuf,
    /// Writable too: temp files, caches, the cargo target folder.
    pub cache: PathBuf,
    /// More places in the home folder that may be read (linked dependency folders).
    pub read: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunOutput {
    /// Exit code; `None` when killed by a signal or the time limit.
    pub code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
}

impl RunOutput {
    /// "exit code 1", "timed out", ...
    pub fn status_text(&self) -> String {
        match (self.timed_out, self.code) {
            (true, _) => "timed out".into(),
            (false, Some(0)) => "exited normally".into(),
            (false, Some(c)) => format!("exit code {c}"),
            (false, None) => "killed by a signal".into(),
        }
    }
}

/// Can code be run confined on this system?
pub fn available() -> bool {
    cfg!(target_os = "macos") && Path::new(SANDBOX_EXEC).is_file()
}

/// Why nothing can run here, for the status line.
pub const UNAVAILABLE: &str =
    "running code needs the macOS sandbox (sandbox-exec), which this system doesn't have";

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// The sandbox matches real paths: `/tmp` is `/private/tmp` on macOS.
fn real(p: &Path) -> Option<PathBuf> {
    p.canonicalize().ok()
}

/// Places in the home folder the command may read: toolchains, the folders
/// on PATH and the toolchain homes set in the environment, and `extra`.
fn reads(home: &Path, extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut add = |p: PathBuf| {
        if let Some(r) = real(&p)
            && r != home
            && !out.contains(&r)
        {
            out.push(r);
        }
    };
    for t in TOOLCHAINS {
        add(home.join(t));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if dir.starts_with(home) {
                add(dir);
            }
        }
    }
    for var in [
        "CARGO_HOME",
        "RUSTUP_HOME",
        "NVM_DIR",
        "VOLTA_HOME",
        "PYENV_ROOT",
    ] {
        if let Some(v) = std::env::var_os(var) {
            add(PathBuf::from(v));
        }
    }
    for p in extra {
        add(p.clone());
    }
    out
}

fn env(home: &Path, cache: &Path) -> Vec<(OsString, OsString)> {
    let mut out: Vec<(OsString, OsString)> = PASS_ENV
        .iter()
        .filter_map(|k| std::env::var_os(k).map(|v| (OsString::from(k), v)))
        .collect();
    let set = |k: &str, v: OsString| (OsString::from(k), v);
    out.extend([
        set("HOME", home.into()),
        set("TMPDIR", cache.join("tmp").into()),
        set("XDG_CACHE_HOME", cache.join("xdg").into()),
        set("npm_config_cache", cache.join("npm").into()),
        set("CARGO_TARGET_DIR", cache.join("cargo-target").into()),
        set("CARGO_NET_OFFLINE", "true".into()),
        set("PYTHONDONTWRITEBYTECODE", "1".into()),
        set("NO_COLOR", "1".into()),
        set("TERM", "dumb".into()),
    ]);
    out
}

/// The `sandbox-exec` arguments before the command: the profile and its parameters.
fn sandbox_args(jail: &Jail, home: &Path) -> Result<Vec<OsString>> {
    let dir = real(&jail.dir).context("the probe folder is missing")?;
    let cache = real(&jail.cache).context("the probe cache folder is missing")?;
    let home = real(home).unwrap_or_else(|| home.to_path_buf());
    let places = reads(&home, &jail.read);
    let reads: Vec<String> = (0..places.len())
        .map(|i| format!("(subpath (param \"R{i}\"))"))
        .collect();
    let profile = PROFILE.replace("READS", &reads.join(" "));
    let mut args: Vec<OsString> = vec!["-p".into(), profile.into()];
    let mut param = |k: &str, v: &Path| {
        let mut s = OsString::from(format!("{k}="));
        s.push(v.as_os_str());
        args.push("-D".into());
        args.push(s);
    };
    param("HOME", &home);
    param("DIR", &dir);
    param("CACHE", &cache);
    for (i, p) in places.iter().enumerate() {
        param(&format!("R{i}"), p);
    }
    Ok(args)
}

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(r: Option<R>) -> String {
    let Some(r) = r else {
        return String::new();
    };
    let mut buf = Vec::new();
    let _ = r.take(MAX_OUTPUT).read_to_end(&mut buf).await;
    String::from_utf8_lossy(&buf).into_owned()
}

fn kill_group(pid: u32) {
    let _ = std::process::Command::new("/bin/kill")
        .args(["-KILL", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Run `argv` in `jail.dir`, confined, for at most `timeout`. Fails only when
/// it can't be started; a crash or a time-out is a normal [`RunOutput`].
pub async fn run(jail: &Jail, argv: &[String], timeout: Duration) -> Result<RunOutput> {
    if !available() {
        bail!(UNAVAILABLE);
    }
    let Some((prog, rest)) = argv.split_first() else {
        bail!("no command to run");
    };
    if prog.is_empty() || prog.starts_with('-') || argv.iter().any(|a| a.contains('\0')) {
        bail!("refusing to run '{prog}'");
    }
    let home = home().context("HOME is not set")?;
    // A fresh temp folder per run: nothing carries over from the last one.
    let tmp = jail.cache.join("tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let mut cmd = Command::new(SANDBOX_EXEC);
    cmd.args(sandbox_args(jail, &home)?)
        .arg(prog)
        .args(rest)
        .current_dir(&jail.dir)
        .env_clear()
        .envs(env(&home, &jail.cache))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = cmd.spawn().context("couldn't start sandbox-exec")?;
    let pid = child.id();
    // Both streams are read on their own, so a full pipe never stalls the run.
    let out = tokio::spawn(read_capped(child.stdout.take()));
    let err = tokio::spawn(read_capped(child.stderr.take()));
    let status = tokio::time::timeout(timeout, child.wait()).await;
    // Whatever happened, nothing it started may outlive it; that also closes
    // the pipes a leftover child still held open.
    if let Some(pid) = pid {
        kill_group(pid);
    }
    let _ = child.start_kill();
    let collect = |h: tokio::task::JoinHandle<String>| async move {
        tokio::time::timeout(Duration::from_secs(2), h)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
    };
    let (stdout, stderr) = (collect(out).await, collect(err).await);
    Ok(match status {
        Ok(status) => RunOutput {
            code: status.context("waiting for the probe failed")?.code(),
            timed_out: false,
            stdout,
            stderr,
        },
        Err(_) => RunOutput {
            code: None,
            timed_out: true,
            stdout,
            stderr: format!("{stderr}\nstopped after {} seconds", timeout.as_secs()),
        },
    })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn jail(root: &Path) -> Jail {
        let dir = root.join("tree");
        let cache = root.join("cache");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        Jail {
            dir,
            cache,
            read: Vec::new(),
        }
    }

    async fn sh(j: &Jail, script: &str) -> RunOutput {
        let argv = ["/bin/sh", "-c", script].map(String::from);
        run(j, &argv, Duration::from_secs(20)).await.unwrap()
    }

    /// A folder inside the home folder, so the home rules apply to it.
    fn home_tmp() -> tempfile::TempDir {
        let base = home().unwrap().join(".local/share");
        std::fs::create_dir_all(&base).unwrap();
        tempfile::Builder::new()
            .prefix("teer-sandbox-test")
            .tempdir_in(base)
            .unwrap()
    }

    #[tokio::test]
    async fn writes_stay_in_the_checkout_and_its_cache() {
        let root = home_tmp();
        let j = jail(root.path());
        let outside = root.path().join("outside");
        let out = sh(
            &j,
            &format!(
                "echo a > in.txt && echo b > \"$TMPDIR/t\" && echo c > {}; echo done",
                outside.display()
            ),
        )
        .await;
        assert!(out.stdout.contains("done"), "{out:?}");
        assert!(j.dir.join("in.txt").is_file());
        assert!(!outside.exists(), "wrote outside: {out:?}");
    }

    #[tokio::test]
    async fn home_secrets_and_the_network_are_out_of_reach() {
        let root = home_tmp();
        let j = jail(root.path());
        // A file in the home folder that is not in an allowed place.
        let secret = root.path().join("secret.txt");
        std::fs::write(&secret, "s3cret").unwrap();
        let out = sh(
            &j,
            &format!(
                "cat {} || echo no-read; /usr/bin/curl -s -m 5 https://example.com >/dev/null && echo NET || echo no-net; pwd",
                secret.display()
            ),
        )
        .await;
        assert!(!out.stdout.contains("s3cret"), "{out:?}");
        assert!(out.stdout.contains("no-read"), "{out:?}");
        assert!(out.stdout.contains("no-net"), "{out:?}");
        assert!(
            out.stdout.contains("tree"),
            "the working directory: {out:?}"
        );
    }

    #[tokio::test]
    async fn the_environment_is_cleared() {
        let root = home_tmp();
        let j = jail(root.path());
        // cargo sets this for test runs; it is not on the pass list.
        assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
        let out = sh(&j, "env").await;
        assert!(!out.stdout.contains("CARGO_MANIFEST_DIR"), "{out:?}");
        assert!(out.stdout.contains("CARGO_NET_OFFLINE=true"), "{out:?}");
    }

    #[tokio::test]
    async fn a_run_that_hangs_is_stopped_with_its_children() {
        let root = home_tmp();
        let j = jail(root.path());
        let argv = ["/bin/sh", "-c", "sleep 30 & sleep 30"].map(String::from);
        let started = std::time::Instant::now();
        let out = run(&j, &argv, Duration::from_secs(1)).await.unwrap();
        assert!(out.timed_out, "{out:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(out.status_text(), "timed out");
    }

    #[tokio::test]
    async fn a_child_left_behind_does_not_hold_the_run() {
        let root = home_tmp();
        let j = jail(root.path());
        // The shell exits at once; its child keeps stdout open for 30 s.
        let argv = ["/bin/sh", "-c", "echo printed; sleep 30 &"].map(String::from);
        let started = std::time::Instant::now();
        let out = run(&j, &argv, Duration::from_secs(20)).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(10), "{out:?}");
        assert!(!out.timed_out);
        assert_eq!(out.stdout, "printed\n");
    }

    #[tokio::test]
    async fn output_is_capped() {
        let root = home_tmp();
        let j = jail(root.path());
        let out = sh(&j, "yes | head -c 3000000").await;
        assert_eq!(out.stdout.len() as u64, MAX_OUTPUT);
    }

    #[tokio::test]
    async fn options_are_never_taken_for_the_program() {
        let root = home_tmp();
        let j = jail(root.path());
        let argv = ["-p".to_string(), "x".to_string()];
        assert!(run(&j, &argv, Duration::from_secs(1)).await.is_err());
        assert!(run(&j, &[], Duration::from_secs(1)).await.is_err());
    }
}
