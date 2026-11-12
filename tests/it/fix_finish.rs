//! Proposed changes: fix_finish (see FEATURES.md, proposed changes). The
//! finish of a change the agent proposed: its hunks in the notes and in the
//! fix loop's prompt, the hunks the reviewer suggested on a pull request as
//! GitHub suggestions in the posted review (or diffs in its summary), and
//! its line in the flagged list. No network: `gh` is a script in a temp
//! dir, and the "remote" a local bare repository.

use crate::common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::v2::{Driver, review, unit};
use common::{FixtureRepo, isolate_env};
use teer::approve::{self, Tools, Verdict};
use teer::fix::{HunkState, Proposal, ProposedHunk, Reason};
use teer::git::{ChangedFile, FileStatus};
use teer::review::model::{
    Decision, DiveKind, DiveResult, PrInfo, Range, Review, Risk, Side, Unit,
};
use teer::security::SecondCheck;
use teer::session::app::Screen;
use teer::session::finish::Step;

// ---- Building proposals ------------------------------------------------------

fn hunk(file: &str, lines: (u32, u32), old: &str, new: &str, why: &str) -> ProposedHunk {
    ProposedHunk {
        file: PathBuf::from(file),
        lines: Range::new(lines.0, lines.1),
        old: old.into(),
        new: new.into(),
        why: why.into(),
        parses: true,
        state: HunkState::Proposed,
    }
}

fn state(mut h: ProposedHunk, s: HunkState) -> ProposedHunk {
    h.state = s;
    h
}

/// `u` with a proposed change of `hunks`, answering `reason`.
fn proposing(mut u: Unit, reason: Reason, hunks: Vec<ProposedHunk>) -> Unit {
    u.dive.results.insert(
        DiveKind::Fix,
        DiveResult {
            text: "adds the owner check".into(),
            proposal: Some(Proposal {
                reason,
                summary: "adds the owner check before the query, nothing else".into(),
                hunks,
                second: None,
                dropped: 0,
            }),
            ..Default::default()
        },
    );
    u
}

fn proposal_mut(u: &mut Unit) -> &mut Proposal {
    u.dive
        .results
        .get_mut(&DiveKind::Fix)
        .and_then(|r| r.proposal.as_mut())
        .unwrap()
}

const IDOR: &str = "the asset id reaches the query without the owner check";

fn finding() -> Reason {
    Reason::Finding {
        index: 0,
        title: IDOR.into(),
        severity: Risk::High,
    }
}

/// The owner check, at lines 3-5 of `src/a.php`.
fn owner_check() -> ProposedHunk {
    hunk(
        "src/a.php",
        (3, 5),
        "function show($id) {\n    return $this->find($id);\n}",
        "function show($id) {\n    $this->denyAccessUnlessGranted('VIEW');\n    return $this->find($id);\n}",
        "check the owner first",
    )
}

/// Three units: u0 flagged with a note, u1 noted and accepted, u2 flagged
/// without a note.
fn noted() -> Vec<Unit> {
    let mut us = vec![
        unit(0, "src/a.php"),
        unit(1, "src/a.php"),
        unit(2, "src/b.php"),
    ];
    us[0].decision = Decision::Investigate;
    us[0].dive.note = "check the owner first".into();
    us[1].decision = Decision::Accepted;
    us[1].dive.note = "fine".into();
    us[2].decision = Decision::Investigate;
    us
}

// ---- Notes and the fix loop ---------------------------------------------------

#[test]
fn the_notes_carry_a_proposed_change_under_its_unit() {
    let mut us = noted();
    us[0] = proposing(
        us[0].clone(),
        finding(),
        vec![
            owner_check(),
            state(
                hunk("src/a.php", (9, 9), "a", "b", ""),
                HunkState::Suggested,
            ),
            state(
                hunk("src/a.php", (12, 12), "c", "d", "gone"),
                HunkState::Applied,
            ),
        ],
    );
    proposal_mut(&mut us[0]).second = Some(SecondCheck {
        agree: true,
        why: "it closes the hole and changes nothing else".into(),
        evidence: None,
    });
    let md = teer::notes::markdown(&review(us));
    let at = md.find("## src/a.php · `s0`").expect(&md);
    let end = md[at..].find("\n## ").map_or(md.len(), |e| at + e);
    let want = [
        "> check the owner first",
        "",
        "**Proposed change**",
        "",
        &format!("- answers: the finding \"{IDOR}\" (high)"),
        "- what it does: adds the owner check before the query, nothing else",
        "- second model: agrees: it closes the hole and changes nothing else",
        "- proposed: check the owner first",
        "",
        "  ```diff",
        "  --- src/a.php:3-5",
        "   function show($id) {",
        "  +    $this->denyAccessUnlessGranted('VIEW');",
        "       return $this->find($id);",
        "   }",
        "  ```",
        "- suggested",
        "",
        "  ```diff",
        "  --- src/a.php:9-9",
        "  -a",
        "  +b",
        "  ```",
        "",
    ]
    .join("\n");
    assert!(md[at..end].ends_with(&want), "{md}");
    // The hunk already in the code isn't.
    assert!(!md.contains("gone") && !md.contains("12-12"), "{md}");
    // The other units say nothing about it.
    assert_eq!(md.matches("**Proposed change**").count(), 1, "{md}");
}

#[test]
fn notes_without_a_proposal_are_as_they_were() {
    let bare = review(noted());
    let mut us = noted();
    // Every hunk in the code already, or none at all: nothing to say.
    us[0] = proposing(
        us[0].clone(),
        Reason::Note,
        vec![state(owner_check(), HunkState::Applied)],
    );
    us[2] = proposing(us[2].clone(), Reason::Advice, Vec::new());
    let r = review(us);
    assert_eq!(teer::notes::markdown(&r), teer::notes::markdown(&bare));
    assert_eq!(
        teer::notes::agent_prompt(&r),
        teer::notes::agent_prompt(&bare)
    );
    assert_eq!(teer::notes::listed(&r).len(), 3);
    assert!(!teer::notes::markdown(&r).contains("Proposed"));
}

#[test]
fn a_unit_with_only_a_proposed_change_is_listed_and_counted() {
    let mut us = noted();
    // Pending, no note: there for its proposed change alone.
    us.push(proposing(
        unit(3, "src/c.php"),
        Reason::Advice,
        vec![owner_check()],
    ));
    let r = review(us);
    let listed = teer::notes::listed(&r);
    assert_eq!(listed.len(), 4);
    assert_eq!(
        teer::notes::count(&listed),
        "2 notes, 1 flagged unit without one and 1 proposed change"
    );
    let md = teer::notes::markdown(&r);
    let at = md.find("## src/c.php · `s3`").expect(&md);
    assert!(
        md[at..]
            .starts_with("## src/c.php · `s3`\n\n**Proposed change**\n\n- answers: the advice\n"),
        "{md}"
    );
    assert!(!md[at..].contains("_Flagged in the review"), "{md}");
    // Alone, and beside a note.
    let only = review(vec![proposing(
        unit(0, "a.rs"),
        Reason::Note,
        vec![owner_check()],
    )]);
    assert_eq!(
        teer::notes::count(&teer::notes::listed(&only)),
        "1 proposed change"
    );
    let mut two = vec![unit(0, "a.rs"), unit(1, "a.rs")];
    two[0].dive.note = "n".into();
    two[1] = proposing(two[1].clone(), Reason::Note, vec![owner_check()]);
    assert_eq!(
        teer::notes::count(&teer::notes::listed(&review(two))),
        "1 note and 1 proposed change"
    );
}

#[test]
fn the_prompt_asks_to_put_a_proposed_change_in_only_when_there_is_one() {
    const ASK: &str = "Where a change is proposed, put it in unless you see a reason not to, \
and say why if you don't.";
    let bare = teer::notes::agent_prompt(&review(noted()));
    assert!(!bare.contains(ASK), "{bare}");
    let mut us = noted();
    us[0] = proposing(us[0].clone(), finding(), vec![owner_check()]);
    let r = review(us);
    let prompt = teer::notes::agent_prompt(&r);
    assert_eq!(prompt.matches(ASK).count(), 1, "{prompt}");
    assert!(prompt.ends_with(&teer::notes::markdown(&r)), "{prompt}");
    // Before the notes, after the rest of what to do.
    assert!(
        prompt.find(ASK).unwrap() < prompt.find("# Review notes").unwrap(),
        "{prompt}"
    );
}

// ---- The PR post ----------------------------------------------------------------

const OLD: &str = "def show(id):
    item = find(id)
    return item


def a():
    return 1


def b():
    return 2


def save(item):
    db.write(item)
    return True
";

/// The PR changed `show`'s first line and added line 3: its diff shows
/// lines 1-6 of the new side; `save` (15-17) is outside it.
const NEW: &str = "def show(id, user):
    item = find(id)
    log(user)
    return item


def a():
    return 1


def b():
    return 2


def save(item):
    db.write(item)
    return True
";

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn pr_info() -> PrInfo {
    PrInfo {
        host: "github.com".into(),
        owner: "acme".into(),
        repo: "shop".into(),
        number: 7,
        remote: Some("origin".into()),
        head_sha: Some(SHA.into()),
        repo_dir: Some(PathBuf::from("/nowhere/teer-fix-finish")),
        ..Default::default()
    }
}

/// `old` starts and ends inside lines 1 and 2.
fn two_lines() -> ProposedHunk {
    state(
        hunk(
            "shop.py",
            (1, 2),
            "id, user):\n    item = find",
            "id, user):\n    item = find_for",
            "find it for this user",
        ),
        HunkState::Suggested,
    )
}

fn one_line() -> ProposedHunk {
    state(
        hunk(
            "shop.py",
            (4, 4),
            "return item",
            "return item or None",
            "say when there is none",
        ),
        HunkState::Suggested,
    )
}

/// A review of the PR: `show` (1-4) flagged with a note on line 1 and a
/// proposed change of two suggested hunks in the diff and one never
/// suggested; `save` (15-17) accepted, its two suggested hunks outside the
/// diff and in a file the PR doesn't touch.
fn pr_review(info: PrInfo) -> Review {
    let mut show = unit(0, "shop.py");
    show.symbol = "show".into();
    show.old_range = Some(Range::new(1, 3));
    show.new_range = Some(Range::new(1, 4));
    show.decision = Decision::Investigate;
    show.dive.note = "only the owner may see it".into();
    let show = proposing(
        show,
        Reason::Note,
        vec![
            two_lines(),
            one_line(),
            hunk(
                "shop.py",
                (3, 3),
                "log(user)",
                "audit(user)",
                "never posted",
            ),
        ],
    );
    let mut save = unit(1, "shop.py");
    save.symbol = "save".into();
    save.old_range = Some(Range::new(14, 16));
    save.new_range = Some(Range::new(15, 17));
    save.decision = Decision::Accepted;
    let save = proposing(
        save,
        Reason::Advice,
        vec![
            state(
                hunk(
                    "shop.py",
                    (16, 16),
                    "db.write(item)",
                    "db.write(item, user)",
                    "write who saved it",
                ),
                HunkState::Suggested,
            ),
            state(
                hunk(
                    "db.py",
                    (2, 2),
                    "def write(item):",
                    "def write(item, user):",
                    "take the user",
                ),
                HunkState::Suggested,
            ),
        ],
    );
    let mut r = review(vec![show, save]);
    r.head = SHA.into();
    r.head_commit = SHA.into();
    r.pr = Some(info);
    r.files = vec![ChangedFile {
        path: PathBuf::from("shop.py"),
        old_path: None,
        status: FileStatus::Modified,
        old_content: Some(OLD.into()),
        new_content: Some(NEW.into()),
    }];
    r
}

fn plan(r: &Review, inline_on: bool) -> approve::Plan {
    let order: Vec<usize> = (0..r.units.len()).collect();
    approve::plan(r, &order, approve::DEFAULT_INTRO, inline_on)
}

const TWO_LINES: &str =
    "find it for this user\n\n```suggestion\ndef show(id, user):\n    item = find_for(id)\n```";

#[test]
fn a_suggested_hunk_in_the_diff_is_a_suggestion_on_its_whole_lines() {
    let p = plan(&pr_review(pr_info()), true);
    assert_eq!(p.inline.len(), 2, "{:#?}", p.inline);
    // Lines 1-2, `old` cut inside both: the whole lines, replaced. It joins
    // the note on line 1, which it covers.
    let c = &p.inline[0];
    assert_eq!(
        (c.path.as_str(), c.start_line, c.line, c.side, c.suggestion),
        ("shop.py", Some(1), 2, Side::New, true)
    );
    assert_eq!(c.body, format!("only the owner may see it\n\n{TWO_LINES}"));
    // Line 4: one line, no start_line, a comment of its own.
    let c = &p.inline[1];
    assert_eq!((c.start_line, c.line, c.suggestion), (None, 4, true));
    assert_eq!(
        c.body,
        "say when there is none\n\n```suggestion\n    return item or None\n```"
    );
    // The hunk never suggested isn't posted anywhere.
    assert!(!p.body.contains("audit") && !p.inline.iter().any(|c| c.body.contains("audit")));
    // The JSON: start_line and the side for the range.
    let json: serde_json::Value = serde_json::from_str(&approve::review_json(
        SHA,
        Verdict::Comment,
        &p.body,
        &p.inline,
    ))
    .unwrap();
    let c = &json["comments"][0];
    assert_eq!(
        (&c["start_line"], &c["start_side"], &c["line"], &c["side"]),
        (
            &serde_json::json!(1),
            &serde_json::json!("RIGHT"),
            &serde_json::json!(2),
            &serde_json::json!("RIGHT")
        )
    );
    assert!(json["comments"][1].get("start_line").is_none(), "{json}");
}

#[test]
fn without_a_note_on_its_lines_a_suggestion_is_a_comment_of_its_own() {
    let mut r = pr_review(pr_info());
    r.units[0].dive.note.clear();
    let p = plan(&r, true);
    assert_eq!(p.inline.len(), 2, "{:#?}", p.inline);
    assert_eq!((p.inline[0].start_line, p.inline[0].line), (Some(1), 2));
    assert_eq!(p.inline[0].body, TWO_LINES);
    // The note on line 1, the suggestion on 4: two comments.
    let mut r = pr_review(pr_info());
    proposal_mut(&mut r.units[0]).hunks.remove(0);
    let p = plan(&r, true);
    assert_eq!(p.inline.len(), 2, "{:#?}", p.inline);
    assert_eq!(
        (p.inline[0].line, p.inline[0].suggestion),
        (1, false),
        "{:#?}",
        p.inline
    );
    assert_eq!(p.inline[0].body, "only the owner may see it");
    assert_eq!((p.inline[1].line, p.inline[1].suggestion), (4, true));
}

#[test]
fn a_hunk_outside_the_diff_or_in_another_file_goes_to_the_summary() {
    let p = plan(&pr_review(pr_info()), true);
    let at = p.body.find("### Proposed changes").expect(&p.body);
    assert_eq!(
        &p.body[at..],
        "### Proposed changes

- save (`shop.py:16`): write who saved it

```diff
-    db.write(item)
+    db.write(item, user)
```

- save (`db.py:2`): take the user

```diff
-def write(item):
+def write(item, user):
```"
    );
    assert!(p.body.starts_with(approve::DEFAULT_INTRO), "{}", p.body);
    assert!(
        !p.inline.iter().any(|c| c.symbol == "save"),
        "{:#?}",
        p.inline
    );
    // A base teer guessed: its diff isn't the PR's, nothing inline.
    let mut info = pr_info();
    info.base_assumed = Some("main".into());
    let p = plan(&pr_review(info), true);
    assert!(p.inline.is_empty(), "{:#?}", p.inline);
    assert_eq!(p.body.matches("```diff").count(), 4, "{}", p.body);
}

#[test]
fn suggestions_count_toward_the_inline_limit() {
    let mut r = pr_review(pr_info());
    // 49 more notes inline in another file: room for one comment more.
    r.files.push(ChangedFile {
        path: PathBuf::from("n.py"),
        old_path: None,
        status: FileStatus::Added,
        old_content: None,
        new_content: Some("x\n".repeat(60)),
    });
    for i in 0..49 {
        let mut u = unit(10 + i, "n.py");
        u.new_range = Some(Range::new(i as u32 + 1, i as u32 + 1));
        u.dive.note = format!("n{i}");
        r.units.insert(0, u);
    }
    let p = plan(&r, true);
    assert_eq!(p.inline.len(), approve::INLINE_MAX);
    // The note and the first suggestion in one; the second waits in the
    // summary, a diff.
    assert_eq!(p.inline.iter().filter(|c| c.suggestion).count(), 1);
    let at = p.body.find("### Proposed changes").expect(&p.body);
    assert!(
        p.body[at..].contains("- show (`shop.py:4`): say when there is none"),
        "{}",
        p.body
    );
}

#[test]
fn proposed_hunks_never_suggested_are_not_posted() {
    let mut r = pr_review(pr_info());
    for u in &mut r.units {
        for h in &mut proposal_mut(u).hunks {
            h.state = HunkState::Proposed;
        }
    }
    let p = plan(&r, true);
    assert!(!p.inline.iter().any(|c| c.suggestion), "{:#?}", p.inline);
    assert!(!p.body.contains("Proposed changes"), "{}", p.body);
    assert_eq!(p.inline.len(), 1);
    assert_eq!(p.inline[0].body, "only the owner may see it");
    // And none at all: as before.
    let notes = approve::collect_notes(&r, &[0, 1]);
    assert_eq!(p, approve::build_plan(&notes, approve::DEFAULT_INTRO, true));
}

#[test]
fn the_sheet_shows_the_suggestions_that_will_be_posted() {
    let mut d = Driver::with(pr_review(pr_info()), Default::default());
    d.keys(":w<cr>");
    assert_eq!(d.app.finish_step(), Some(Step::RequestChanges));
    // The body scrolls: every page of it.
    let mut text = String::new();
    loop {
        text.push_str(&d.render(100, 40));
        let at = d.app.finish.scroll;
        d.keys("<c-d>");
        if d.app.finish.scroll == at {
            break;
        }
    }
    for want in [
        "2 inline comments · 2 suggest a change",
        "shop.py:1-2 only the owner may see it",
        "```suggestion",
        "def show(id, user):",
        "    item = find_for(id)",
        "shop.py:4 say when there is none",
        "    return item or None",
        "### Proposed changes",
        "-    db.write(item)",
        "+    db.write(item, user)",
        "✎ change proposed: 1 hunk, 2 suggested",
        "✎ change proposed: 2 suggested",
    ] {
        assert!(text.contains(want), "{want}:\n{text}");
    }
}

// ---- gh -------------------------------------------------------------------------

/// A repository whose remote has the PR's head at `refs/pull/7/head`.
struct Origin {
    repo: FixtureRepo,
    _bare: tempfile::TempDir,
    head: String,
}

impl Origin {
    fn new() -> Origin {
        isolate_env();
        let repo = FixtureRepo::new();
        repo.write("shop.py", OLD).commit("init");
        repo.write("shop.py", NEW).commit("pr");
        let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
        let bare = tempfile::tempdir().unwrap();
        let st = std::process::Command::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(bare.path())
            .status()
            .unwrap();
        assert!(st.success());
        let url = bare.path().canonicalize().unwrap();
        repo.git(&["remote", "add", "origin", url.to_str().unwrap()]);
        repo.git(&["push", "-q", "origin", "main"]);
        repo.git(&["push", "-q", "origin", "HEAD:refs/pull/7/head"]);
        Origin {
            repo,
            _bare: bare,
            head,
        }
    }

    fn info(&self) -> PrInfo {
        PrInfo {
            head_sha: Some(self.head.clone()),
            repo_dir: Some(self.repo.path()),
            ..pr_info()
        }
    }
}

/// `gh` records each call's input (`stdin_<k>`) and answers per `mode`:
/// ok, or reject_lines (a 422 about lines the first time).
fn fake_gh(mode: &str) -> (tempfile::TempDir, Tools) {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("mode"), mode).unwrap();
    let gh = d.join("gh");
    std::fs::write(
        &gh,
        r#"#!/bin/sh
d=$(dirname "$0")
n=$(ls "$d" | grep -c '^stdin_')
n=$((n+1))
cat > "$d/stdin_$n"
case "$(cat "$d/mode")" in
  ok) echo '{"id":1}';;
  reject_lines) if [ "$n" = 1 ]; then echo "gh: Unprocessable Entity (HTTP 422)" >&2; echo "Pull request review thread line must be part of the diff" >&2; exit 1; fi; echo '{"id":2}';;
esac
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let tools = Tools {
        gh: gh.into(),
        opener: d.join("no-opener").into(),
        editor: d.join("no-editor").into(),
        claude: d.join("no-claude").into(),
        codex: d.join("no-codex").into(),
    };
    (dir, tools)
}

fn sent(dir: &Path, k: usize) -> serde_json::Value {
    let s = std::fs::read_to_string(dir.join(format!("stdin_{k}"))).unwrap();
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("{e}: {s}"))
}

async fn post(o: &Origin, tools: &Tools) -> Result<String, String> {
    let info = o.info();
    let r = pr_review(info.clone());
    let p = plan(&r, true);
    let flat = plan(&r, false).body;
    let post = approve::post(
        tools,
        &info,
        Verdict::RequestChanges,
        p.body,
        p.inline,
        flat,
    );
    tokio::time::timeout(Duration::from_secs(20), post)
        .await
        .expect("posted in time")
}

#[tokio::test]
async fn the_suggestions_go_out_as_review_comments() {
    let o = Origin::new();
    let (dir, tools) = fake_gh("ok");
    post(&o, &tools).await.unwrap();
    let v = sent(dir.path(), 1);
    assert_eq!(v["event"], "REQUEST_CHANGES");
    assert_eq!(
        v["comments"],
        serde_json::json!([
            {
                "path": "shop.py", "start_line": 1, "start_side": "RIGHT", "line": 2,
                "side": "RIGHT", "body": format!("only the owner may see it\n\n{TWO_LINES}")
            },
            {
                "path": "shop.py", "line": 4, "side": "RIGHT",
                "body": "say when there is none\n\n```suggestion\n    return item or None\n```"
            }
        ])
    );
    assert!(
        v["body"].as_str().unwrap().contains("### Proposed changes"),
        "{v}"
    );
}

#[tokio::test]
async fn refused_inline_comments_put_every_suggestion_in_the_summary() {
    let o = Origin::new();
    let (dir, tools) = fake_gh("reject_lines");
    let r = post(&o, &tools).await.unwrap();
    assert!(r.contains("posted with a summary comment instead"), "{r}");
    let v = sent(dir.path(), 2);
    assert!(v.get("comments").is_none(), "{v}");
    let body = v["body"].as_str().unwrap();
    assert!(
        body.contains("- show (shop.py:1): only the owner may see it"),
        "{body}"
    );
    let at = body.find("### Proposed changes").expect(body);
    let section = &body[at..];
    for want in [
        "- show (`shop.py:1-2`): find it for this user",
        "-    item = find(id)\n+    item = find_for(id)",
        "- show (`shop.py:4`): say when there is none",
        "- save (`shop.py:16`): write who saved it",
        "- save (`db.py:2`): take the user",
    ] {
        assert!(section.contains(want), "{want}:\n{section}");
    }
    assert!(
        !body.contains("suggestion\n") && !body.contains("audit"),
        "{body}"
    );
}

// ---- The flagged list -------------------------------------------------------------

#[test]
fn the_flagged_list_says_a_change_is_proposed() {
    use teer::session::flagged_list::proposal_line;
    let mut us = noted();
    us[0] = proposing(
        us[0].clone(),
        finding(),
        vec![
            owner_check(),
            owner_check(),
            state(owner_check(), HunkState::Applied),
        ],
    );
    us[2] = proposing(
        us[2].clone(),
        Reason::Note,
        vec![state(owner_check(), HunkState::Suggested)],
    );
    assert_eq!(
        proposal_line(&us[0]).as_deref(),
        Some("change proposed: 2 hunks")
    );
    assert_eq!(
        proposal_line(&us[2]).as_deref(),
        Some("change proposed: 1 suggested")
    );
    assert_eq!(proposal_line(&us[1]), None);
    let mut d = Driver::of(us);
    d.keys("<space>f");
    assert_eq!(d.app.screen, Screen::Flagged);
    let text = d.render(100, 40);
    let row = |s: &str| {
        text.lines()
            .position(|l| l.contains(s))
            .unwrap_or_else(|| panic!("{s}:\n{text}"))
    };
    assert_eq!(row("✎ change proposed: 2 hunks"), row(" s0") + 1, "{text}");
    // Once: the preview's own line ("a change is proposed") isn't repeated here.
    assert!(!text.contains("a change is proposed"), "{text}");
    assert_eq!(
        row("✎ change proposed: 1 suggested"),
        row(" s2") + 1,
        "{text}"
    );
    // Every hunk in the code: no line.
    for h in &mut proposal_mut(&mut d.app.review.units[0]).hunks {
        h.state = HunkState::Applied;
    }
    let text = d.render(100, 40);
    assert!(!text.contains("2 hunks"), "{text}");
}
