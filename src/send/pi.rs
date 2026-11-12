//! Sessions of pi: finding the running ones, and sending them a message,
//! through teer's own pi extension (`teer-inbox.ts`), which each session
//! loads at start; installing that extension.
//!
//! Filled in by the delivery slice.

use std::path::{Path, PathBuf};

use super::{Dirs, PiState, Target};

/// The pi sessions running now in `repo` that loaded teer's extension.
pub fn list(dirs: &Dirs, repo: &Path) -> Vec<Target> {
    let _ = (dirs, repo);
    Vec::new()
}

/// Send `text` to session `t`.
pub fn deliver(dirs: &Dirs, t: &Target, text: &str) -> Result<(), String> {
    let _ = (dirs, t, text);
    Err("sending isn't built yet".into())
}

/// Whether pi is installed, and teer's extension with it.
pub fn state(dirs: &Dirs) -> PiState {
    let _ = dirs;
    PiState::NotFound
}

/// Write teer's extension where pi finds it (`<pi dir>/extensions/`):
/// the file written.
pub fn install(dirs: &Dirs) -> Result<PathBuf, String> {
    let _ = dirs;
    Err("installing isn't built yet".into())
}
