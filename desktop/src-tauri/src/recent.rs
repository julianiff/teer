//! The repositories reviewed in the app lately, newest first, kept beside
//! teer's reviews.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How many repositories the list keeps.
const KEEP: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recent {
    pub path: PathBuf,
    /// The folder's name.
    pub name: String,
    /// RFC 3339.
    pub opened: String,
}

fn file() -> PathBuf {
    teer::review::store::data_dir().join("desktop-recent.json")
}

/// The list, without folders that are gone.
pub fn list() -> Vec<Recent> {
    let list: Vec<Recent> = std::fs::read(file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    list.into_iter().filter(|r| r.path.is_dir()).collect()
}

/// `repo` was opened now: it goes to the top.
pub fn add(repo: &Path) {
    let mut list = list();
    list.retain(|r| r.path != repo);
    list.insert(
        0,
        Recent {
            path: repo.to_path_buf(),
            name: repo
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| repo.display().to_string()),
            opened: chrono::Utc::now().to_rfc3339(),
        },
    );
    list.truncate(KEEP);
    if let Ok(json) = serde_json::to_vec_pretty(&list) {
        let path = file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, json);
    }
}
