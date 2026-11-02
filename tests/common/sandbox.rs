//! Running the real binary hermetically: a [`Sandbox`] gives each run its own
//! `HOME`, a `PATH` with git and the test's fake programs only (never the real
//! claude, codex or gh), a fake gh through `TEER_GH`, its own config file, and
//! the test process's data dir ([`super::isolate_env`]).

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{isolate_env, teer_bin};

/// A program as a shell script at `path`.
pub fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// The git the tests run, found on this process's `PATH`.
pub fn real_git() -> PathBuf {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|d| d.join("git"))
        .find(|g| g.is_file())
        .expect("git on the PATH")
}

/// Where one test runs teer: see the module doc.
pub struct Sandbox {
    pub root: tempfile::TempDir,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        isolate_env();
        let root = tempfile::tempdir().unwrap();
        let s = Sandbox { root };
        std::fs::create_dir_all(s.bin()).unwrap();
        std::fs::create_dir_all(s.home()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(real_git(), s.bin().join("git")).unwrap();
        s
    }

    /// The only directory of the `PATH` besides `/usr/bin` and `/bin`.
    pub fn bin(&self) -> PathBuf {
        self.root.path().join("bin")
    }

    pub fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    /// teer's config; it doesn't exist until [`Sandbox::config`] writes it.
    pub fn config_path(&self) -> PathBuf {
        self.root.path().join("config.toml")
    }

    pub fn config(&self, toml: &str) -> &Self {
        std::fs::write(self.config_path(), toml).unwrap();
        self
    }

    /// `TEER_GH`; missing (gh not installed) until [`Sandbox::gh`] writes it.
    pub fn gh_path(&self) -> PathBuf {
        self.root.path().join("gh")
    }

    /// gh as a script; `"$(dirname "$0")/gh.log"` is [`Sandbox::gh_calls`].
    pub fn gh(&self, body: &str) -> &Self {
        script(&self.gh_path(), body);
        self
    }

    /// Every call gh logged, one per line.
    pub fn gh_calls(&self) -> String {
        std::fs::read_to_string(self.root.path().join("gh.log")).unwrap_or_default()
    }

    /// A fake program on the `PATH` (`claude`, `codex`).
    pub fn program(&self, name: &str, body: &str) -> &Self {
        script(&self.bin().join(name), body);
        self
    }

    pub fn command(&self, dir: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(teer_bin());
        c.args(args)
            .arg("-C")
            .arg(dir)
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.home().join(".config"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin().display()))
            .env("TEER_CONFIG", self.config_path())
            .env("TEER_DATA_DIR", isolate_env().path().join("data"))
            .env("TEER_GH", self.gh_path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        c
    }

    /// Run teer in `dir`, without a terminal, for at most 60 s.
    pub fn run(&self, dir: &Path, args: &[&str]) -> Output {
        let mut child = self
            .command(dir, args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > Duration::from_secs(60) {
                let _ = child.kill();
                panic!("teer {args:?} didn't finish in 60 s (waiting for a terminal?)");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        child.wait_with_output().unwrap()
    }

    /// Run teer in `dir`; it must succeed and print JSON.
    pub fn json(&self, dir: &Path, args: &[&str]) -> Value {
        let out = self.run(dir, args);
        assert!(out.status.success(), "teer {args:?}: {}", stderr(&out));
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("teer {args:?}: {e}: {}", stdout(&out)))
    }
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The paths of a review's changed files (`--json`), sorted.
pub fn files(review: &Value) -> Vec<String> {
    let mut v: Vec<String> = review["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}
