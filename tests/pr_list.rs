//! `teer pr list`: which remote names the repository, the exact gh calls,
//! and how their answers are merged. gh is a fake script; git is real.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use teer::cli::{Args, Command, PrAction};
use teer::pr_list::{self, Section};

fn script(dir: &Path, body: &str) -> PathBuf {
    let p = dir.join("gh");
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

fn repo_with(remotes: &[(&str, &str)]) -> FixtureRepo {
    isolate_env();
    let repo = FixtureRepo::new();
    repo.write("a.py", "x = 1\n").commit("init");
    for (name, url) in remotes {
        repo.git(&["remote", "add", name, url]);
    }
    repo
}

#[test]
fn the_cli_parses_pr_list_and_still_takes_a_url() {
    use clap::Parser;
    let a = Args::try_parse_from(["teer", "pr", "list", "--json"]).unwrap();
    assert!(matches!(
        a.command,
        Some(Command::Pr {
            action: Some(PrAction::List),
            url: None
        })
    ));
    assert!(a.json);
    let a = Args::try_parse_from(["teer", "pr", "ls"]).unwrap();
    assert!(matches!(
        a.command,
        Some(Command::Pr {
            action: Some(PrAction::List),
            ..
        })
    ));
    let a = Args::try_parse_from(["teer", "pr", "https://github.com/a/b/pull/1"]).unwrap();
    assert!(matches!(
        a.command,
        Some(Command::Pr { action: None, url: Some(ref u) }) if u == "https://github.com/a/b/pull/1"
    ));
    assert!(Args::try_parse_from(["teer", "pr"]).is_err());
    assert!(Args::try_parse_from(["teer", "pr", "list", "https://github.com/a/b/pull/1"]).is_err());
}

#[test]
fn the_remote_gh_chose_wins_then_upstream_then_origin() {
    let repo = repo_with(&[
        ("origin", "git@github.com:me/shop.git"),
        ("upstream", "https://github.com/Acme/Shop.git"),
        ("mirror", "https://git.corp.example/team/shop"),
    ]);
    let t = pr_list::target_repo(&repo.path()).unwrap();
    assert_eq!(
        (t.remote.as_str(), t.name()),
        ("upstream", "Acme/Shop".into())
    );

    repo.git(&["config", "remote.mirror.gh-resolved", "base"]);
    let t = pr_list::target_repo(&repo.path()).unwrap();
    assert_eq!(t.remote, "mirror");
    assert_eq!(t.gh_repo(), "git.corp.example/team/shop");

    let only_origin = repo_with(&[("origin", "git@github.com:me/shop.git")]);
    assert_eq!(
        pr_list::target_repo(&only_origin.path()).unwrap().gh_repo(),
        "me/shop"
    );
}

#[test]
fn no_github_remote_is_explained() {
    let none = repo_with(&[]);
    let err = format!("{:#}", pr_list::target_repo(&none.path()).unwrap_err());
    assert!(err.contains("has no remotes"), "{err}");

    let local = repo_with(&[("origin", "/srv/git/shop.git")]);
    let err = format!("{:#}", pr_list::target_repo(&local.path()).unwrap_err());
    assert!(err.contains("origin  /srv/git/shop.git"), "{err}");
}

#[tokio::test]
async fn three_gh_searches_are_merged_into_sections() {
    let repo = repo_with(&[("origin", "https://github.com/acme/shop.git")]);
    let d = tempfile::tempdir().unwrap();
    let log = d.path().join("calls");
    let pr = |n: u32, t: &str| {
        format!(
            r#"{{"number":{n},"title":"{t}","url":"https://github.com/acme/shop/pull/{n}","author":{{"login":"bob"}},"isDraft":false,"reviewDecision":"REVIEW_REQUIRED","updatedAt":"2026-10-12T10:00:00Z","headRefName":"b{n}"}}"#
        )
    };
    let gh = script(
        d.path(),
        &format!(
            r#"echo "$@" >> {log}
case "$*" in
  *review-requested:@me*) echo '[{review}]' ;;
  *"--author @me"*) echo '[{mine}]' ;;
  *"--assignee @me"*) echo '[{both}]' ;;
  *) echo "unexpected: $*" >&2; exit 1 ;;
esac"#,
            log = log.display(),
            review = pr(1, "Fix totals"),
            mine = pr(2, "My change"),
            both = [pr(2, "My change"), pr(3, "Assigned one")].join(","),
        ),
    );
    let target = pr_list::target_repo(&repo.path()).unwrap();
    let l = pr_list::fetch(gh.as_os_str(), &repo.path(), target)
        .await
        .unwrap();
    let nums = |s: Section| l.section(s).iter().map(|p| p.number).collect::<Vec<_>>();
    assert_eq!(nums(Section::NeedsReview), [1]);
    assert_eq!(nums(Section::Mine), [2]);
    assert_eq!(nums(Section::Assigned), [3]);

    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(calls.lines().count(), 3, "{calls}");
    for line in calls.lines() {
        assert!(
            line.starts_with("pr list --repo acme/shop --state open --limit 100 --json number,title,url,author,isDraft,reviewDecision,updatedAt,headRefName "),
            "{line}"
        );
    }
}

#[tokio::test]
async fn a_gh_failure_is_reported_without_secrets() {
    let repo = repo_with(&[("origin", "https://github.com/acme/shop.git")]);
    let d = tempfile::tempdir().unwrap();
    let gh = script(
        d.path(),
        "echo 'HTTP 401: Bad credentials (token ghp_abc123)' >&2; exit 1",
    );
    let target = pr_list::target_repo(&repo.path()).unwrap();
    let err = format!(
        "{:#}",
        pr_list::fetch(gh.as_os_str(), &repo.path(), target)
            .await
            .unwrap_err()
    );
    assert!(err.contains("Bad credentials"), "{err}");
    assert!(!err.contains("ghp_abc123"), "{err}");

    let missing = d.path().join("no-such-gh");
    let target = pr_list::target_repo(&repo.path()).unwrap();
    let err = format!(
        "{:#}",
        pr_list::fetch(missing.as_os_str(), &repo.path(), target)
            .await
            .unwrap_err()
    );
    assert!(err.contains("needs the GitHub CLI"), "{err}");
}

// ------------------------------------------------------------------ picker

mod picker {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use teer::config::Theme as ThemeName;
    use teer::keymap::{Keymap, parse_keys};
    use teer::pr_list::{self, ListedPr, RepoRef, Section};
    use teer::tui::pr_pick::{self, Picked, Picker};
    use teer::tui::theme::Theme;

    fn repo() -> RepoRef {
        RepoRef {
            remote: "origin".into(),
            host: "github.com".into(),
            owner: "acme".into(),
            repo: "shop".into(),
        }
    }

    fn listed(n: u32, title: &str, author: &str, updated: &str) -> ListedPr {
        ListedPr {
            number: n,
            title: title.into(),
            url: format!("https://github.com/acme/shop/pull/{n}"),
            author: author.into(),
            draft: false,
            review_decision: None,
            updated_at: Some(updated.parse().unwrap()),
            head: format!("b{n}"),
        }
    }

    fn picker() -> Picker {
        let mut mine = listed(
            118,
            "Split checkout into steps",
            "me",
            "2026-10-10T12:00:00Z",
        );
        mine.draft = true;
        mine.review_decision = Some("CHANGES_REQUESTED".into());
        let l = pr_list::merge(
            repo(),
            vec![
                (
                    Section::NeedsReview,
                    vec![
                        listed(
                            123,
                            "Fix rounding of order totals",
                            "bob",
                            "2026-10-13T10:00:00Z",
                        ),
                        listed(
                            120,
                            "Reject unsupported currencies",
                            "alice",
                            "2026-10-12T12:00:00Z",
                        ),
                    ],
                ),
                (Section::Mine, vec![mine]),
                (Section::Assigned, vec![]),
            ],
        );
        let mut p = Picker::new(
            repo(),
            Theme::new(ThemeName::Mono),
            Keymap::default(),
            "2026-10-13T12:00:00Z".parse().unwrap(),
        );
        p.loaded(l);
        p
    }

    fn key(p: &mut Picker, code: KeyCode) -> Option<Picked> {
        p.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Type `keys`, in Neovim notation; what the last one did.
    fn keys(p: &mut Picker, keys: &str) -> Option<Picked> {
        let mut last = None;
        for k in parse_keys(keys).unwrap() {
            last = p.on_key(k.event());
        }
        last
    }

    fn render(p: &mut Picker, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| pr_pick::draw(f, p)).unwrap();
        t
    }

    fn screen(t: &Terminal<TestBackend>) -> String {
        let b = t.backend().buffer();
        (0..b.area.height)
            .map(|y| {
                (0..b.area.width)
                    .map(|x| b[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn moves_across_sections_and_enter_picks_the_selected_one() {
        let mut p = picker();
        assert_eq!(p.selected().unwrap().number, 123);
        assert_eq!(keys(&mut p, "j"), None);
        assert_eq!(key(&mut p, KeyCode::Down), None);
        // Past the empty "Assigned to you" into "Yours".
        assert_eq!(p.selected().unwrap().number, 118);
        keys(&mut p, "j");
        assert_eq!(p.selected().unwrap().number, 118, "stays on the last");
        keys(&mut p, "gg");
        assert_eq!(p.selected().unwrap().number, 123);
        keys(&mut p, "G");
        keys(&mut p, "k");
        assert_eq!(p.selected().unwrap().number, 120);
        keys(&mut p, "<home>2j");
        assert_eq!(p.selected().unwrap().number, 118, "a count moves that far");
        keys(&mut p, "<end><up>");
        match keys(&mut p, "<cr>") {
            Some(Picked::Pr(pr)) => assert_eq!(pr.url, "https://github.com/acme/shop/pull/120"),
            other => panic!("{other:?}"),
        }
        // `l` picks too.
        match keys(&mut p, "l") {
            Some(Picked::Pr(pr)) => assert_eq!(pr.number, 120),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn next_and_previous_file_jump_between_sections() {
        let mut p = picker();
        // Past the empty "Assigned to you" into "Yours", and no further.
        keys(&mut p, "]f");
        assert_eq!(p.selected().unwrap().number, 118);
        keys(&mut p, "]f");
        assert_eq!(p.selected().unwrap().number, 118);
        keys(&mut p, "[f");
        assert_eq!(p.selected().unwrap().number, 123);
        // From the middle of a section to the first of the one above: none
        // above, so it stays.
        keys(&mut p, "j[f");
        assert_eq!(p.selected().unwrap().number, 120);
    }

    #[test]
    fn q_h_esc_and_ctrl_c_leave_without_picking() {
        let mut p = picker();
        assert_eq!(keys(&mut p, "q"), Some(Picked::Quit));
        assert_eq!(keys(&mut p, "h"), Some(Picked::Quit));
        assert_eq!(keys(&mut p, "<esc>"), Some(Picked::Quit));
        assert_eq!(keys(&mut p, "<c-c>"), Some(Picked::Quit));
        // ctrl+c also while a prefix waits for its next key.
        assert_eq!(keys(&mut p, "g<c-c>"), Some(Picked::Quit));
    }

    #[test]
    fn while_gh_is_asked_only_quitting_works() {
        let mut p = Picker::new(
            repo(),
            Theme::new(ThemeName::Mono),
            Keymap::default(),
            chrono::Utc::now(),
        );
        assert_eq!(keys(&mut p, "<cr>"), None);
        assert_eq!(keys(&mut p, "j"), None);
        assert_eq!(keys(&mut p, "]f[f<c-d><pagedown>"), None);
        let text = screen(&render(&mut p, 70, 6));
        assert!(
            text.contains("Asking GitHub for the pull requests of acme/shop"),
            "{text}"
        );
        assert_eq!(keys(&mut p, "q"), Some(Picked::Quit));
    }

    #[test]
    fn the_selection_stays_on_screen_in_a_short_terminal() {
        let mut p = picker();
        let numbers: Vec<u32> = (1..=30).collect();
        let many = numbers
            .iter()
            .map(|n| listed(*n, &format!("Change {n}"), "bob", "2026-10-12T12:00:00Z"))
            .collect();
        p.loaded(pr_list::merge(repo(), vec![(Section::NeedsReview, many)]));
        keys(&mut p, "20j");
        let text = screen(&render(&mut p, 60, 10));
        let sel = p.selected().unwrap().number;
        assert!(
            text.lines()
                .any(|l| l.starts_with(" ▌") && l.contains(&format!("#{sel} "))),
            "{text}"
        );
        keys(&mut p, "gg");
        let text = screen(&render(&mut p, 60, 10));
        assert!(text.contains("Waiting for your review · 30"), "{text}");
        // A page is the 7 rows on screen; half a page 3.
        keys(&mut p, "<pagedown>");
        assert_eq!(p.selected().unwrap().number, 30 - 7);
        keys(&mut p, "<c-u>");
        assert_eq!(p.selected().unwrap().number, 30 - 4);
        keys(&mut p, "<c-d><c-d>");
        assert_eq!(p.selected().unwrap().number, 30 - 10);
        keys(&mut p, "<pageup>");
        assert_eq!(p.selected().unwrap().number, 30 - 3);
    }

    #[test]
    fn snapshot() {
        let mut p = picker();
        keys(&mut p, "j");
        insta::assert_snapshot!("pr_pick", render(&mut p, 90, 14).backend());
    }
}
