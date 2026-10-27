//! `teer pr list`: the open pull requests of this repository that are
//! waiting for your review, assigned to you, or yours. On a terminal they
//! open in a picker ([`crate::tui::pr_pick`]); the one picked is reviewed.
//!
//! git can't ask GitHub who you are, so this uses `gh` (three `gh pr list`
//! searches with `@me`, run side by side). The repository is the one behind
//! a remote of this clone: the remote `gh repo set-default` chose, else
//! `upstream`, else `origin`, else the first that names a `<host>/<owner>/<repo>`.
//! Nothing runs a shell; titles and names from GitHub are sanitised before
//! they are printed.

use std::ffi::OsStr;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::approve::tidy_output;
use crate::config::{Config, KeyMap};
use crate::pr;
use crate::tui::pr_pick::Picked;
use crate::util::{sanitize, truncate};

/// Most pull requests fetched per search.
pub const LIMIT: usize = 100;
const FIELDS: &str = "number,title,url,author,isDraft,reviewDecision,updatedAt,headRefName";

/// The GitHub repository a remote of this clone points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepoRef {
    pub remote: String,
    pub host: String,
    pub owner: String,
    pub repo: String,
}

impl RepoRef {
    /// `owner/repo`.
    pub fn name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// The `--repo` value for gh: `owner/repo`, with the host in front when
    /// it is not github.com.
    pub fn gh_repo(&self) -> String {
        if self.host == "github.com" {
            self.name()
        } else {
            format!("{}/{}", self.host, self.name())
        }
    }
}

/// Why a pull request is listed. Each one is listed once, under the first
/// that applies, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    NeedsReview,
    Mine,
    Assigned,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::NeedsReview => "Waiting for your review",
            Section::Assigned => "Assigned to you",
            Section::Mine => "Yours",
        }
    }

    /// The gh filter that finds this section's pull requests.
    fn filter(self) -> [&'static str; 2] {
        match self {
            Section::NeedsReview => ["--search", "review-requested:@me"],
            Section::Mine => ["--author", "@me"],
            Section::Assigned => ["--assignee", "@me"],
        }
    }
}

/// Order of the sections on screen.
pub const SHOWN: [Section; 3] = [Section::NeedsReview, Section::Assigned, Section::Mine];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedPr {
    pub number: u32,
    pub title: String,
    pub url: String,
    pub author: String,
    pub draft: bool,
    /// GitHub's review state: `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    pub head: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Listing {
    pub repo: RepoRef,
    pub needs_review: Vec<ListedPr>,
    pub assigned: Vec<ListedPr>,
    pub mine: Vec<ListedPr>,
    /// A search returned [`LIMIT`] results: there may be more.
    pub truncated: bool,
}

impl Listing {
    pub fn section(&self, s: Section) -> &[ListedPr] {
        match s {
            Section::NeedsReview => &self.needs_review,
            Section::Assigned => &self.assigned,
            Section::Mine => &self.mine,
        }
    }

    fn section_mut(&mut self, s: Section) -> &mut Vec<ListedPr> {
        match s {
            Section::NeedsReview => &mut self.needs_review,
            Section::Assigned => &mut self.assigned,
            Section::Mine => &mut self.mine,
        }
    }

    pub fn is_empty(&self) -> bool {
        SHOWN.iter().all(|s| self.section(*s).is_empty())
    }
}

// ------------------------------------------------------------------- remote

/// The remote `gh repo set-default` picked (`remote.<name>.gh-resolved`).
fn gh_default_remote(repo: &Path) -> Option<String> {
    pr::git_ok(
        repo,
        &["config", "--get-regexp", r"^remote\..*\.gh-resolved$"],
    )?
    .lines()
    .filter_map(|l| l.split_whitespace().next())
    .filter_map(|k| k.strip_prefix("remote.")?.strip_suffix(".gh-resolved"))
    .map(String::from)
    .next()
}

fn repo_ref(repo: &Path, remote: &str) -> Option<RepoRef> {
    pr::remote_urls(repo, remote).iter().find_map(|u| {
        let (host, owner, name) = pr::parse_remote_url(u)?;
        (pr::valid_host(&host) && pr::valid_name(&owner) && pr::valid_name(&name)).then(|| {
            RepoRef {
                remote: remote.to_string(),
                host,
                owner,
                repo: name,
            }
        })
    })
}

/// The GitHub repository whose pull requests are listed.
pub fn target_repo(repo: &Path) -> Result<RepoRef> {
    let names = pr::remotes(repo)?;
    let mut order: Vec<&str> = Vec::new();
    let preferred = gh_default_remote(repo);
    for want in [preferred.as_deref(), Some("upstream"), Some("origin")]
        .into_iter()
        .flatten()
    {
        if names.iter().any(|n| n == want) && !order.contains(&want) {
            order.push(want);
        }
    }
    for n in &names {
        if !order.contains(&n.as_str()) {
            order.push(n);
        }
    }
    if let Some(r) = order.iter().find_map(|n| repo_ref(repo, n)) {
        return Ok(r);
    }
    let mut msg =
        String::from("none of this repository's remotes points to a GitHub repository.\n");
    if names.is_empty() {
        msg.push_str("This repository has no remotes.");
    } else {
        msg.push_str("Remotes of this repository:");
        for n in &names {
            let shown = pr::remote_urls(repo, n)
                .first()
                .map(|u| pr::display_url(u))
                .unwrap_or_default();
            msg.push_str(&format!("\n  {n}  {shown}"));
        }
    }
    bail!(msg)
}

// ---------------------------------------------------------------------- gh

fn text(v: &Value, max: usize) -> String {
    let s = sanitize(v.as_str().unwrap_or("")).trim().to_string();
    s.chars().take(max).collect()
}

/// Pull requests from `gh pr list --json` output. Entries without a valid
/// number are skipped.
pub fn parse_gh_json(out: &str) -> Result<Vec<ListedPr>> {
    let v: Value = serde_json::from_str(out).context("gh printed something that isn't JSON")?;
    let items = v.as_array().context("gh printed JSON that isn't a list")?;
    Ok(items
        .iter()
        .filter_map(|p| {
            let number = p["number"]
                .as_u64()
                .filter(|n| *n > 0 && *n <= u32::MAX as u64)?;
            Some(ListedPr {
                number: number as u32,
                title: text(&p["title"], 300),
                url: pr::display_url(&text(&p["url"], 300)),
                author: text(&p["author"]["login"], 100),
                draft: p["isDraft"].as_bool().unwrap_or(false),
                review_decision: Some(text(&p["reviewDecision"], 40)).filter(|s| !s.is_empty()),
                updated_at: p["updatedAt"]
                    .as_str()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&Utc)),
                head: text(&p["headRefName"], 200),
            })
        })
        .collect())
}

async fn search(gh: &OsStr, cwd: &Path, target: &RepoRef, s: Section) -> Result<Vec<ListedPr>> {
    let limit = LIMIT.to_string();
    let repo = target.gh_repo();
    let mut args = vec![
        "pr", "list", "--repo", &repo, "--state", "open", "--limit", &limit, "--json", FIELDS,
    ];
    args.extend(s.filter());
    let out = crate::gh::run(gh, cwd, &args, None)
        .await
        .map_err(|e| match e {
            crate::gh::Error::NotInstalled => anyhow::anyhow!(
                "teer pr list needs the GitHub CLI: install gh (https://cli.github.com) and run 'gh auth login'"
            ),
            e => anyhow::anyhow!("{e}"),
        })?;
    if !out.ok {
        bail!(
            "gh couldn't list the pull requests of {}: {}",
            target.gh_repo(),
            tidy_output(&out.stderr)
        );
    }
    parse_gh_json(&out.stdout)
}

/// Put each pull request under the first section that applies, newest
/// update first.
pub fn merge(repo: RepoRef, found: Vec<(Section, Vec<ListedPr>)>) -> Listing {
    let truncated = found.iter().any(|(_, prs)| prs.len() >= LIMIT);
    let mut l = Listing {
        repo,
        needs_review: Vec::new(),
        assigned: Vec::new(),
        mine: Vec::new(),
        truncated,
    };
    let mut seen = std::collections::HashSet::new();
    for want in [Section::NeedsReview, Section::Mine, Section::Assigned] {
        for (s, prs) in &found {
            if *s != want {
                continue;
            }
            for p in prs {
                if seen.insert(p.number) {
                    l.section_mut(want).push(p.clone());
                }
            }
        }
    }
    for s in SHOWN {
        l.section_mut(s).sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then(b.number.cmp(&a.number))
        });
    }
    l
}

/// Ask gh for the open pull requests of `target` that involve you.
pub async fn fetch(gh: &OsStr, cwd: &Path, target: RepoRef) -> Result<Listing> {
    let (a, b, c) = tokio::join!(
        search(gh, cwd, &target, Section::NeedsReview),
        search(gh, cwd, &target, Section::Mine),
        search(gh, cwd, &target, Section::Assigned),
    );
    Ok(merge(
        target,
        vec![
            (Section::NeedsReview, a?),
            (Section::Mine, b?),
            (Section::Assigned, c?),
        ],
    ))
}

// ------------------------------------------------------------------ output

/// `5m`, `3h`, `2d`, `6w`, or `now`.
pub fn age(then: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(then) = then else {
        return String::new();
    };
    let mins = (now - then).num_minutes().max(0);
    match mins {
        0 => "now".into(),
        m if m < 60 => format!("{m}m"),
        m if m < 60 * 24 => format!("{}h", m / 60),
        m if m < 60 * 24 * 14 => format!("{}d", m / (60 * 24)),
        m => format!("{}w", m / (60 * 24 * 7)),
    }
}

/// What is worth knowing beside the title: draft, and the review state of
/// your own pull requests or the author of the others.
pub fn details(p: &ListedPr, s: Section) -> String {
    let mut parts = Vec::new();
    if p.draft {
        parts.push("draft".to_string());
    }
    if s == Section::Mine {
        match p.review_decision.as_deref() {
            Some("APPROVED") => parts.push("approved".into()),
            Some("CHANGES_REQUESTED") => parts.push("changes requested".into()),
            Some("REVIEW_REQUIRED") => parts.push("review required".into()),
            _ => {}
        }
    } else if !p.author.is_empty() {
        parts.push(format!("@{}", p.author));
    }
    parts.join(" · ")
}

/// The listing as text. `width` cuts titles so each line fits; `None`
/// (not a terminal) keeps them whole. `color` adds bold section titles and
/// dim details.
pub fn render(l: &Listing, now: DateTime<Utc>, width: Option<usize>, color: bool) -> String {
    use unicode_width::UnicodeWidthStr;
    let (bold, dim, off) = if color {
        ("\x1b[1m", "\x1b[2m", "\x1b[0m")
    } else {
        ("", "", "")
    };
    let mut out = String::new();
    if l.is_empty() {
        out.push_str(&format!(
            "No open pull requests in {} wait for your review, are assigned to you or are yours.\n",
            l.repo.name()
        ));
        return out;
    }
    let all: Vec<(Section, &ListedPr)> = SHOWN
        .iter()
        .flat_map(|s| l.section(*s).iter().map(move |p| (*s, p)))
        .collect();
    let num_w = all
        .iter()
        .map(|(_, p)| p.number.to_string().len() + 1)
        .max()
        .unwrap_or(2);
    let meta_w = all
        .iter()
        .map(|(s, p)| details(p, *s).width())
        .max()
        .unwrap_or(0);
    let age_w = all
        .iter()
        .map(|(_, p)| age(p.updated_at, now).width())
        .max()
        .unwrap_or(0);
    let url_w = all.iter().map(|(_, p)| p.url.width()).max().unwrap_or(0);
    let widest_title = all.iter().map(|(_, p)| p.title.width()).max().unwrap_or(0);
    // "  #12  title  meta  age  url"
    let fixed = 2 + num_w + 2 + 2 + meta_w + 2 + age_w + 2 + url_w;
    let title_w = match width {
        Some(w) => w.saturating_sub(fixed).clamp(20, widest_title.max(20)),
        None => widest_title,
    };
    out.push_str(&format!("{bold}{}{off}\n", l.repo.name()));
    for s in SHOWN {
        let prs = l.section(s);
        out.push_str(&format!("\n{bold}{} · {}{off}\n", s.title(), prs.len()));
        if prs.is_empty() {
            out.push_str(&format!("  {dim}none{off}\n"));
        }
        for p in prs {
            let title = truncate(&p.title, title_w);
            let tpad = " ".repeat(title_w.saturating_sub(title.width()));
            let meta = details(p, s);
            let mpad = " ".repeat(meta_w.saturating_sub(meta.width()));
            out.push_str(&format!(
                "  {:>num_w$}  {title}{tpad}  {dim}{meta}{mpad}  {:>age_w$}{off}  {}\n",
                format!("#{}", p.number),
                age(p.updated_at, now),
                p.url,
            ));
        }
    }
    if l.truncated {
        out.push_str(&format!(
            "\n{dim}Only the first {LIMIT} of a section are listed.{off}\n"
        ));
    }
    out.push_str(&format!("\n{dim}Review one with: teer pr <url>{off}\n"));
    out
}

/// `teer pr list` for the repository at `cwd`. On a terminal the pull
/// requests open in a picker and the one picked is returned, to be reviewed;
/// piped or with `json`, they are printed.
pub async fn run(cwd: &Path, json: bool, cfg: &Config, keys: KeyMap) -> Result<Option<ListedPr>> {
    use std::io::IsTerminal;
    let repo = crate::git::repo_root(cwd)
        .context("teer pr list must run inside a clone of a GitHub repository")?;
    let target = {
        let repo = repo.clone();
        tokio::task::spawn_blocking(move || target_repo(&repo)).await??
    };
    let gh = crate::gh::program();
    if !json && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        let theme = crate::tui::theme::Theme::new(cfg.theme);
        let listing = fetch(&gh, &repo, target.clone());
        return match crate::tui::pr_pick::pick(target, listing, theme, keys).await? {
            Picked::Pr(p) => Ok(Some(p)),
            Picked::Quit => Ok(None),
            Picked::Empty(l) => {
                print!("{}", render(&l, Utc::now(), None, false));
                Ok(None)
            }
        };
    }
    let listing = fetch(&gh, &repo, target).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&listing)?);
        return Ok(None);
    }
    let tty = std::io::stdout().is_terminal();
    let width = tty
        .then(|| crossterm::terminal::size().ok().map(|(w, _)| w as usize))
        .flatten();
    let color = tty && std::env::var_os("NO_COLOR").is_none();
    print!("{}", render(&listing, Utc::now(), width, color));
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoRef {
        RepoRef {
            remote: "origin".into(),
            host: "github.com".into(),
            owner: "acme".into(),
            repo: "shop".into(),
        }
    }

    fn listed(n: u32, updated: &str) -> ListedPr {
        ListedPr {
            number: n,
            title: format!("Title {n}"),
            url: format!("https://github.com/acme/shop/pull/{n}"),
            author: "alice".into(),
            draft: false,
            review_decision: None,
            updated_at: Some(updated.parse().unwrap()),
            head: "feature".into(),
        }
    }

    #[test]
    fn gh_repo_names_the_host_only_off_github_com() {
        assert_eq!(repo().gh_repo(), "acme/shop");
        let mut r = repo();
        r.host = "git.corp.example".into();
        assert_eq!(r.gh_repo(), "git.corp.example/acme/shop");
    }

    #[test]
    fn parses_gh_json_and_cleans_text() {
        let out = r#"[
            {"number": 7, "title": "Fix \u001b[31mtotals\n", "url": "https://github.com/acme/shop/pull/7",
             "author": {"login": "alice", "is_bot": false}, "isDraft": true, "reviewDecision": "",
             "updatedAt": "2026-10-12T10:00:00Z", "headRefName": "fix-totals"},
            {"number": 0, "title": "bad"},
            {"title": "no number"},
            {"number": 9, "reviewDecision": "APPROVED"}
        ]"#;
        let prs = parse_gh_json(out).unwrap();
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].number, 7);
        assert!(
            !prs[0].title.chars().any(char::is_control),
            "{:?}",
            prs[0].title
        );
        assert!(prs[0].title.starts_with("Fix "));
        assert!(prs[0].draft);
        assert_eq!(prs[0].review_decision, None);
        assert_eq!(prs[0].author, "alice");
        assert!(prs[0].updated_at.is_some());
        assert_eq!(prs[1].review_decision.as_deref(), Some("APPROVED"));
        assert!(parse_gh_json("not json").is_err());
        assert!(parse_gh_json("{}").is_err());
    }

    #[test]
    fn each_pr_is_listed_once_under_the_first_section_that_applies() {
        let l = merge(
            repo(),
            vec![
                (
                    Section::NeedsReview,
                    vec![listed(1, "2026-10-10T00:00:00Z")],
                ),
                // Mine and assigned to me: stays under Yours.
                (Section::Mine, vec![listed(2, "2026-10-11T00:00:00Z")]),
                (
                    Section::Assigned,
                    vec![
                        listed(1, "2026-10-10T00:00:00Z"),
                        listed(2, "2026-10-11T00:00:00Z"),
                        listed(3, "2026-10-09T00:00:00Z"),
                        listed(4, "2026-10-12T00:00:00Z"),
                    ],
                ),
            ],
        );
        let nums = |s: Section| l.section(s).iter().map(|p| p.number).collect::<Vec<_>>();
        assert_eq!(nums(Section::NeedsReview), [1]);
        assert_eq!(nums(Section::Mine), [2]);
        // Newest update first.
        assert_eq!(nums(Section::Assigned), [4, 3]);
        assert!(!l.truncated);
    }

    #[test]
    fn ages_are_short() {
        let now: DateTime<Utc> = "2026-10-13T12:00:00Z".parse().unwrap();
        let at = |s: &str| Some(s.parse().unwrap());
        assert_eq!(age(at("2026-10-13T12:00:00Z"), now), "now");
        assert_eq!(age(at("2026-10-13T11:55:00Z"), now), "5m");
        assert_eq!(age(at("2026-10-13T09:00:00Z"), now), "3h");
        assert_eq!(age(at("2026-10-11T12:00:00Z"), now), "2d");
        assert_eq!(age(at("2026-08-13T12:00:00Z"), now), "8w");
        assert_eq!(age(None, now), "");
    }

    #[test]
    fn renders_sections_with_state_and_fits_the_width() {
        let now: DateTime<Utc> = "2026-10-13T12:00:00Z".parse().unwrap();
        let mut mine = listed(12, "2026-10-13T09:00:00Z");
        mine.review_decision = Some("CHANGES_REQUESTED".into());
        mine.draft = true;
        let mut long = listed(3, "2026-10-11T12:00:00Z");
        long.title =
            "A very long title that goes on and on about what this pull request does".into();
        let l = merge(
            repo(),
            vec![
                (Section::NeedsReview, vec![long]),
                (Section::Mine, vec![mine]),
                (Section::Assigned, vec![]),
            ],
        );
        let text = render(&l, now, Some(100), false);
        assert!(text.starts_with("acme/shop\n"), "{text}");
        let review = text.find("Waiting for your review · 1").unwrap();
        let assigned = text.find("Assigned to you · 0").unwrap();
        let yours = text.find("Yours · 1").unwrap();
        assert!(review < assigned && assigned < yours, "{text}");
        assert!(text.contains("  none\n"), "{text}");
        assert!(text.contains("@alice"), "{text}");
        assert!(text.contains("draft · changes requested"), "{text}");
        assert!(
            text.contains("https://github.com/acme/shop/pull/12"),
            "{text}"
        );
        assert!(text.contains("teer pr <url>"), "{text}");
        for line in text.lines() {
            use unicode_width::UnicodeWidthStr;
            assert!(line.width() <= 100, "{line:?}");
        }
        assert!(text.contains('…'), "{text}");
        // Not a terminal: whole titles.
        let piped = render(&l, now, None, false);
        assert!(piped.contains("goes on and on about what this pull request does"));
        assert!(!piped.contains('\x1b'));
        assert!(render(&l, now, None, true).contains("\x1b[1m"));
    }

    #[test]
    fn empty_listing_says_so() {
        let l = merge(repo(), vec![]);
        let text = render(&l, Utc::now(), Some(80), false);
        assert!(
            text.starts_with("No open pull requests in acme/shop"),
            "{text}"
        );
    }
}
