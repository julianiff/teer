//! Sessions of pi: finding the running ones, and sending them a message.
//!
//! Filled in by the delivery slice.

use std::path::Path;

use super::{Dirs, Target};

/// The pi sessions running now in `repo`.
pub fn list(dirs: &Dirs, repo: &Path) -> Vec<Target> {
    let _ = (dirs, repo);
    Vec::new()
}

/// Send `text` to session `t`.
pub fn deliver(dirs: &Dirs, t: &Target, text: &str) -> Result<(), String> {
    let _ = (dirs, t, text);
    Err("sending isn't built yet".into())
}
