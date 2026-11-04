//! The repositories saved on the start screen, kept beside teer's reviews:
//! every folder picked, dropped or typed there saves its repository, the
//! newest first, so their pull requests waiting for review are a glance
//! away. Forgetting one leaves its reviews alone. The add sheet checks a
//! folder before it is saved ([`check_repo`]); a pull request's link finds
//! the clone it opens in among them ([`pr_link`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    /// The repository's root.
    pub path: PathBuf,
    /// The folder's name.
    pub name: String,
}

fn file() -> PathBuf {
    teer::review::store::data_dir().join("desktop-repos.json")
}

fn read(file: &Path) -> Vec<Saved> {
    std::fs::read(file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write(file: &Path, list: &[Saved]) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, serde_json::to_vec_pretty(list)?)
}

/// The list, without folders that are gone.
fn list_in(file: &Path) -> Vec<Saved> {
    read(file).into_iter().filter(|s| s.path.is_dir()).collect()
}

/// `root` saved first, unless it is saved already (then it stays where
/// it is).
fn add_in(file: &Path, root: &Path) -> std::io::Result<Vec<Saved>> {
    let mut list = list_in(file);
    if !list.iter().any(|s| s.path == root) {
        list.insert(
            0,
            Saved {
                path: root.to_path_buf(),
                name: name_of(root),
            },
        );
        write(file, &list)?;
    }
    Ok(list)
}

fn remove_in(file: &Path, path: &Path) -> std::io::Result<Vec<Saved>> {
    let mut list = list_in(file);
    list.retain(|s| s.path != path);
    write(file, &list)?;
    Ok(list)
}

/// The repositories saved.
#[tauri::command]
pub fn saved_repos() -> Vec<Saved> {
    list_in(&file())
}

/// Save the repository `dir` is in; a folder outside one is not saved.
#[tauri::command]
pub async fn save_repo(dir: String) -> Result<Vec<Saved>, String> {
    let root = tauri::async_runtime::spawn_blocking(move || teer::git::repo_root(&typed(&dir)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
    add_in(&file(), &root).map_err(|e| e.to_string())
}

/// A folder as typed: `~/` is the home folder.
fn typed(dir: &str) -> PathBuf {
    let dir = dir.trim();
    match (dir.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(dir),
    }
}

fn name_of(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

/// A folder the add sheet was given: its repository, and the GitHub
/// repository whose pull requests it lists (`None` without one).
#[derive(Debug, Clone, Serialize)]
pub struct RepoCheck {
    pub path: PathBuf,
    pub name: String,
    pub github: Option<String>,
}

/// What the add sheet says of `dir` before it is saved; an error for a
/// folder outside a repository.
#[tauri::command]
pub async fn check_repo(dir: String) -> Result<RepoCheck, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = teer::git::repo_root(&typed(&dir)).map_err(|e| format!("{e:#}"))?;
        let github = teer::pr_list::target_repo(&root).ok().map(|r| r.name());
        Ok(RepoCheck {
            name: name_of(&root),
            path: root,
            github,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A pull request's link: its repository and number, and the clone it
/// opens in, the first saved (then recent) repository with a remote that
/// points at it; `None` when there is none to open it in yet.
#[derive(Debug, Clone, Serialize)]
pub struct PrLink {
    pub url: String,
    /// `owner/repo`.
    pub repo: String,
    pub number: u32,
    pub clone: Option<Saved>,
}

/// The first of `paths` that is a clone of `pr`'s repository.
fn clone_of(paths: &[PathBuf], pr: &teer::review::model::PrInfo) -> Option<PathBuf> {
    paths
        .iter()
        .find(|p| teer::pr::find_remote(p, pr).is_ok())
        .cloned()
}

/// Read a pull request's link and find the clone it opens in.
#[tauri::command]
pub async fn pr_link(url: String) -> Result<PrLink, String> {
    let url = url.trim().to_string();
    let pr = teer::pr::parse_url(&url).map_err(|e| format!("{e:#}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut paths: Vec<PathBuf> = list_in(&file()).into_iter().map(|s| s.path).collect();
        for r in crate::recent::list() {
            if !paths.contains(&r.path) {
                paths.push(r.path);
            }
        }
        let clone = clone_of(&paths, &pr).map(|path| Saved {
            name: name_of(&path),
            path,
        });
        Ok(PrLink {
            repo: format!("{}/{}", pr.owner, pr.repo),
            number: pr.number,
            url,
            clone,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// No longer keep `path` on the start screen.
#[tauri::command]
pub fn forget_repo(path: String) -> Result<Vec<Saved>, String> {
    remove_in(&file(), Path::new(&path)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_are_saved_once_newest_first_and_forgotten() {
        let dir = std::env::temp_dir().join(format!("teer-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (shop, web) = (dir.join("shop"), dir.join("web"));
        std::fs::create_dir_all(&shop).unwrap();
        std::fs::create_dir_all(&web).unwrap();
        let file = dir.join("desktop-repos.json");
        assert!(list_in(&file).is_empty());
        add_in(&file, &shop).unwrap();
        add_in(&file, &web).unwrap();
        let list = add_in(&file, &shop).unwrap();
        let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["web", "shop"], "saved once, the newest first");
        // A folder that is gone isn't listed.
        std::fs::remove_dir_all(&web).unwrap();
        assert_eq!(list_in(&file)[0].name, "shop");
        assert_eq!(list_in(&file).len(), 1);
        assert!(remove_in(&file, &shop).unwrap().is_empty());
        assert!(list_in(&file).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_typed_folder_may_start_at_home() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(typed(" ~/code/shop "), home.join("code/shop"));
        assert_eq!(typed("/work/shop"), PathBuf::from("/work/shop"));
    }

    #[test]
    fn a_pull_requests_link_opens_in_a_clone_of_its_repository() {
        let dir = std::env::temp_dir().join(format!("teer-clones-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = |name: &str, remote: &str| {
            let path = dir.join(name);
            std::fs::create_dir_all(&path).unwrap();
            let git = |args: &[&str]| {
                let ok = std::process::Command::new("git")
                    .args(args)
                    .current_dir(&path)
                    .output()
                    .unwrap()
                    .status
                    .success();
                assert!(ok, "git {args:?}");
            };
            git(&["init", "-q"]);
            git(&["remote", "add", "origin", remote]);
            path
        };
        let web = repo("web", "https://github.com/acme/web.git");
        let shop = repo("shop", "git@github.com:acme/shop.git");
        let paths = [web.clone(), shop.clone()];
        let pr = teer::pr::parse_url("https://github.com/acme/shop/pull/482").unwrap();
        assert_eq!(clone_of(&paths, &pr), Some(shop));
        let other = teer::pr::parse_url("https://github.com/acme/api/pull/7").unwrap();
        assert_eq!(clone_of(&paths, &other), None, "no clone of it");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
