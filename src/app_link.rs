//! `teer://` links: how the terminal hands a review to Teer.app (`teer
//! app`, `:app`) and how a link in a browser or chat opens one there.
//! `teer://open?repo=<path>` reviews the repository's changes; with
//! `&pr=<number or URL>` a pull request of it, with `&base=<ref>` its
//! branch since that base.

use std::path::{Path, PathBuf};

/// The scheme Teer.app registers.
pub const SCHEME: &str = "teer";

/// A review a link asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub repo: PathBuf,
    pub pr: Option<String>,
    pub base: Option<String>,
}

/// The link that opens `repo` (and `pr`, or since `base`) in Teer.app.
pub fn url(repo: &Path, pr: Option<&str>, base: Option<&str>) -> String {
    let mut url = format!("{SCHEME}://open?repo={}", encode(&repo.to_string_lossy()));
    for (key, value) in [("pr", pr), ("base", base)] {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            url.push_str(&format!("&{key}={}", encode(v)));
        }
    }
    url
}

/// What `url` asks for; `None` for a link that isn't teer's, or names no
/// repository.
pub fn parse(url: &str) -> Option<Link> {
    let rest = url.strip_prefix(&format!("{SCHEME}://"))?;
    let (what, query) = rest.split_once('?').unwrap_or((rest, ""));
    if what.trim_end_matches('/') != "open" {
        return None;
    }
    let mut link = Link {
        repo: PathBuf::new(),
        pr: None,
        base: None,
    };
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = decode(value)?;
        match key {
            "repo" => link.repo = PathBuf::from(value),
            "pr" => link.pr = Some(value).filter(|v| !v.is_empty()),
            "base" => link.base = Some(value).filter(|v| !v.is_empty()),
            _ => {}
        }
    }
    (link.repo.is_absolute()).then_some(link)
}

/// Hand `url` to the system: Teer.app opens it, started first if need be.
pub fn open(url: &str) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "nothing opened the link: is Teer.app installed?",
        ))
    }
}

/// Percent-encode everything but unreserved characters and `/`.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Undo [`encode`] (and `+` for a space); `None` when it isn't UTF-8 or a
/// `%` isn't followed by two hex digits.
fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_says_the_repository_and_what_of_it() {
        let repo = Path::new("/Users/ada/my shop");
        let u = url(repo, Some("https://github.com/a/b/pull/7"), None);
        assert_eq!(
            u,
            "teer://open?repo=/Users/ada/my%20shop&pr=https%3A//github.com/a/b/pull/7"
        );
        assert_eq!(
            parse(&u),
            Some(Link {
                repo: repo.into(),
                pr: Some("https://github.com/a/b/pull/7".into()),
                base: None
            })
        );
        let u = url(repo, None, Some("origin/main"));
        assert_eq!(parse(&u).unwrap().base.as_deref(), Some("origin/main"));
        assert_eq!(
            parse(&url(Path::new("/ü/ß"), None, None)).unwrap().repo,
            Path::new("/ü/ß")
        );
    }

    #[test]
    fn other_links_and_relative_paths_are_not_reviews() {
        assert_eq!(parse("https://example.com/open?repo=/x"), None);
        assert_eq!(parse("teer://delete?repo=/x"), None);
        assert_eq!(parse("teer://open?repo=relative/path"), None);
        assert_eq!(parse("teer://open?pr=4"), None);
        assert_eq!(parse("teer://open?repo=/x%zz"), None);
        assert!(parse("teer://open/?repo=/x").is_some());
    }
}
