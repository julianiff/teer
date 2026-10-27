//! Append-only log in the data dir (`teer.log`). Dropped claims and agent
//! failures go here, since the TUI owns the terminal.

use std::io::Write;

pub fn path() -> std::path::PathBuf {
    crate::review::store::data_dir().join("teer.log")
}

pub fn write(msg: &str) {
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // One write per line, so lines from parallel writers don't interleave.
    let line = format!(
        "{} {msg}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
    {
        let _ = f.write_all(line.as_bytes());
    }
}
