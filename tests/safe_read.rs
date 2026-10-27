//! A repository (or pull request) can contain symlinks, FIFOs and devices.
//! teer must never follow them out of the repository, never block on them
//! and never read an unbounded amount from them.

#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use serde_json::json;
use teer::agent::mock::MockAdapter;
use teer::review::model::{Review, SessionRef};
use teer::review::{BuildOptions, build_review};

const SHOP_OLD: &str = "def price(qty):\n    return qty * 10\n";
const SHOP_NEW: &str = "def price(qty):\n    return qty * 12\n";
const SECRET: &str = "def leak():\n    price(1)  # SECRET_TOKEN_XYZ\n";

fn repo_with_changed_price() -> FixtureRepo {
    let repo = FixtureRepo::new();
    repo.write("shop.py", SHOP_OLD)
        .write("app.py", "def checkout():\n    return price(2)\n")
        .commit("init");
    repo.write("shop.py", SHOP_NEW);
    repo
}

/// Build the review on a thread, failing the test when it hangs.
fn build_with_timeout(repo: &Path) -> Review {
    let repo = repo.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(build_review(&repo, &BuildOptions::default()));
    });
    rx.recv_timeout(Duration::from_secs(30))
        .expect("building the review hung (followed a device or FIFO?)")
        .unwrap()
}

fn callers_of_price(r: &Review) -> Vec<String> {
    r.units
        .iter()
        .find(|u| u.symbol == "price")
        .unwrap()
        .callers
        .clone()
}

async fn prompts_of(mut r: Review, cwd: &Path) -> String {
    isolate_env();
    let mock = MockAdapter::new(|_, _| Ok(json!({"units": []})));
    let prompts = mock.prompts.clone();
    let session = SessionRef::fresh("mock", cwd.to_path_buf());
    teer::cli::prepare_all(&mut r, Arc::new(mock), session, 2).await;
    prompts.lock().unwrap().join("\n=====\n")
}

#[tokio::test]
async fn a_symlink_to_a_file_outside_the_repo_is_never_read() {
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("credentials.py");
    std::fs::write(&secret, SECRET).unwrap();

    // Control: the same content as a regular tracked file IS found as a caller,
    // so the assertions below can fail.
    let control = repo_with_changed_price();
    control.write("leaky.py", SECRET);
    control.git(&["add", "leaky.py"]);
    let r = build_with_timeout(&control.path());
    assert!(
        callers_of_price(&r).iter().any(|c| c.contains("leak")),
        "{:?}",
        callers_of_price(&r)
    );

    let repo = repo_with_changed_price();
    // Tracked symlink, and an untracked one (the working-tree path).
    std::os::unix::fs::symlink(&secret, repo.dir.path().join("leaky.py")).unwrap();
    repo.git(&["add", "leaky.py"]);
    std::os::unix::fs::symlink(&secret, repo.dir.path().join("untracked_leaky.py")).unwrap();
    // A directory symlink: files below it are outside the repo too.
    std::os::unix::fs::symlink(outside.path(), repo.dir.path().join("linkdir")).unwrap();
    let r = build_with_timeout(&repo.path());
    assert!(callers_of_price(&r).iter().all(|c| !c.contains("leak")));
    let json = serde_json::to_string(&r).unwrap();
    assert!(
        !json.contains("SECRET_TOKEN_XYZ"),
        "secret reached the review"
    );
    assert!(
        r.files
            .iter()
            .all(|f| !f.path.to_string_lossy().contains("leaky"))
    );

    let prompts = prompts_of(r, &repo.path()).await;
    assert!(prompts.contains("def price"), "prompts were captured");
    assert!(!prompts.contains("SECRET_TOKEN_XYZ"));
    assert!(!prompts.contains("leak"));
}

#[test]
fn dev_zero_and_fifos_are_skipped_without_blocking() {
    let repo = repo_with_changed_price();
    std::os::unix::fs::symlink("/dev/zero", repo.dir.path().join("zero.py")).unwrap();
    repo.git(&["add", "zero.py"]);
    // An untracked FIFO named like source code.
    let fifo = repo.dir.path().join("pipe.py");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    // A tracked FIFO cannot be committed, but a FIFO that replaces a tracked file can exist.
    repo.write("shop.py", SHOP_OLD);
    repo.write("swapped.py", "def swapped():\n    price(3)\n")
        .commit("add swapped");
    repo.write("shop.py", SHOP_NEW);
    std::fs::remove_file(repo.dir.path().join("swapped.py")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(repo.dir.path().join("swapped.py"))
            .status()
            .unwrap()
            .success()
    );

    let start = std::time::Instant::now();
    let r = build_with_timeout(&repo.path());
    assert!(start.elapsed() < Duration::from_secs(20));
    assert!(r.units.iter().any(|u| u.symbol == "price"));
    assert!(r.files.iter().all(|f| {
        let p = f.path.to_string_lossy();
        !p.contains("zero") && !p.contains("pipe")
    }));
}

#[test]
fn read_text_file_accepts_only_plain_files_inside_the_repo() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("ok.py"), "x = 1\n").unwrap();
    std::fs::write(root.join("big.py"), "y".repeat(100)).unwrap();
    std::fs::write(outside.path().join("o.py"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.path().join("o.py"), root.join("l.py")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("d")).unwrap();
    std::os::unix::fs::symlink("/dev/zero", root.join("z.py")).unwrap();
    std::fs::create_dir(root.join("sub.py")).unwrap();
    std::fs::write(root.join("bin.py"), [0xff, 0xfe, 0x00]).unwrap();
    let read = |p: &str, limit| teer::git::read_text_file(root, Path::new(p), limit);
    assert_eq!(read("ok.py", 1000).as_deref(), Some("x = 1\n"));
    assert_eq!(read("big.py", 50), None, "over the limit");
    assert_eq!(read("big.py", 100).map(|s| s.len()), Some(100));
    assert_eq!(read("l.py", 1000), None, "symlink out of the repo");
    assert_eq!(read("d/o.py", 1000), None, "through a symlinked directory");
    assert_eq!(read("z.py", 1000), None, "device behind a symlink");
    assert_eq!(read("sub.py", 1000), None, "directory");
    assert_eq!(read("bin.py", 1000), None, "not UTF-8");
    assert_eq!(read("../x.py", 1000), None);
    assert_eq!(read("/etc/hosts", 1000), None);
    assert_eq!(read("missing.py", 1000), None);
}

#[test]
fn an_endless_device_is_refused_quickly() {
    // /dev/zero reports size 0; the cap must come from what is actually read.
    // (read_text_file refuses it by type; this documents that no path relies on len().)
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("/dev/zero", dir.path().join("z")).unwrap();
    let start = std::time::Instant::now();
    assert_eq!(
        teer::git::read_text_file(dir.path(), Path::new("z"), 1 << 20),
        None
    );
    assert!(start.elapsed() < Duration::from_secs(2));
}
