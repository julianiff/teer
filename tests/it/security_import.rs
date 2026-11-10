//! Security lens, phases 4 and 5 (see docs/security-plan.md): other tools'
//! findings imported from SARIF (`--sarif`) and from GitHub code scanning
//! (a fake `gh`), pinned to the units they fall in; teer's own facts and
//! findings written out as SARIF (`teer notes --sarif`); and prove it
//! (`<space>p`), each finding's attacker case run old against new.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use common::sandbox::{Sandbox, script, stderr};
use common::{FixtureRepo, isolate_env, teer_bin};
use serde_json::{Value, json};
use teer::agent::mock::MockAdapter;
use teer::config::Config;
use teer::keymap::Keymap;
use teer::review::attention::{Level, assess};
use teer::review::dive::DiveEvent;
use teer::review::model::{Cite, DiveKind, DiveResult, Review, Risk, SessionRef, Side};
use teer::review::{BuildOptions, build_review};
use teer::security::sarif::{self, Imported};
use teer::security::{Audit, Class, Fact, Finding, Hop, Origin, Proof, Weight};
use teer::session::Runtime;
use teer::session::app::{AppEvent, Effect};
use teer::tui::app::App;

const VIEWS_OLD: &str = "import db


def search(q):
    return db.query(\"SELECT * FROM t WHERE name = ?\", [q])


def greet(name):
    return \"hi \" + name


def untouched(x):
    return x
";

/// `search` is lines 4-6, `greet` 9-10; `untouched` (13-14) is not changed.
const VIEWS_NEW: &str = "import db


def search(q):
    sql = \"SELECT * FROM t WHERE name = '\" + q + \"'\"
    return db.query(sql)


def greet(name):
    return \"<b>hi \" + name + \"</b>\"


def untouched(x):
    return x
";

/// A repository with `views.py` changed in the working tree.
fn repo() -> FixtureRepo {
    let r = FixtureRepo::new();
    r.write("views.py", VIEWS_OLD)
        .write("other.py", "def other():\n    return 1\n")
        .commit("init");
    r.write("views.py", VIEWS_NEW);
    r
}

fn build(r: &FixtureRepo, sarif: Vec<PathBuf>) -> Review {
    build_review(
        &r.path(),
        &BuildOptions {
            sarif,
            ..Default::default()
        },
    )
    .unwrap()
}

fn unit<'a>(r: &'a Review, symbol: &str) -> &'a teer::review::model::Unit {
    r.units.iter().find(|u| u.symbol == symbol).unwrap()
}

fn result(rule: Value, uri: &str, line: u32, extra: Value) -> Value {
    let mut r = json!({
        "message": { "text": "This query depends on a [user-provided value](1)." },
        "locations": [{
            "physicalLocation": {
                "artifactLocation": { "uri": uri, "uriBaseId": "%SRCROOT%" },
                "region": { "startLine": line, "startColumn": 5, "endLine": line }
            }
        }]
    });
    for (k, v) in rule
        .as_object()
        .unwrap()
        .iter()
        .chain(extra.as_object().unwrap())
    {
        r[k] = v.clone();
    }
    r
}

/// CodeQL's way: rules in the query pack's extension (results point there
/// through `toolComponent`), CWE tags, a `security-severity`, URIs relative
/// to `%SRCROOT%` or absolute `file://`.
fn codeql(repo_root: &Path) -> Value {
    let rule = |id: &str, cwe: &str, sev: &str| {
        json!({
            "id": id,
            "name": id,
            "shortDescription": { "text": format!("{id} rule") },
            "defaultConfiguration": { "level": "error" },
            "properties": { "tags": ["security", format!("external/cwe/cwe-{cwe}")], "security-severity": sev }
        })
    };
    let at = |index: u64, id: &str| json!({"ruleId": id, "rule": {"id": id, "index": index, "toolComponent": {"index": 0}}});
    let absolute = format!("file://{}/views.py", repo_root.display());
    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": { "name": "CodeQL", "rules": [] },
                "extensions": [{
                    "name": "codeql/python-queries",
                    "rules": [
                        rule("py/sql-injection", "089", "8.8"),
                        rule("py/reflective-xss", "079", "6.1"),
                        rule("py/path-injection", "022", "7.5"),
                    ]
                }]
            },
            "results": [
                // In `search`: attention.
                result(at(0, "py/sql-injection"), "views.py", 6, json!({})),
                // In `greet`, by an absolute path: worth a look (a warning
                // with severity 6.1).
                {
                    "ruleId": "py/reflective-xss",
                    "rule": {"id": "py/reflective-xss", "index": 1, "toolComponent": {"index": 0}},
                    "level": "warning",
                    "message": { "text": "Cross-site scripting from {0}.", "arguments": ["name"] },
                    "locations": [{
                        "physicalLocation": {
                            "artifactLocation": { "uri": absolute },
                            "region": { "startLine": 10 }
                        }
                    }]
                },
                // In code the change didn't touch, and in a file it didn't.
                result(at(2, "py/path-injection"), "views.py", 14, json!({})),
                result(at(2, "py/path-injection"), "other.py", 2, json!({})),
                // Suppressed in the source: no finding.
                result(at(0, "py/sql-injection"), "views.py", 5, json!({"suppressions": [{"kind": "inSource"}]})),
            ]
        }]
    })
}

/// Semgrep's way: rules in the driver, CWE tags as words, no severity, a
/// level per result; the same result twice.
fn semgrep() -> Value {
    let rule = |id: &str, tags: Value| {
        json!({
            "id": id,
            "name": id,
            "shortDescription": { "text": "Semgrep rule" },
            "properties": { "tags": tags }
        })
    };
    let hit = |index: u64, id: &str, line: u32, level: &str, text: &str| {
        json!({
            "ruleId": id,
            "ruleIndex": index,
            "level": level,
            "message": { "text": text },
            "locations": [{
                "physicalLocation": {
                    "artifactLocation": { "uri": "./views.py" },
                    "region": { "startLine": line, "endLine": line + 1 }
                }
            }]
        })
    };
    json!({
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": { "name": "Semgrep OSS", "rules": [
                rule("python.lang.security.audit.formatted-sql-query", json!(["CWE-798: Use of Hard-coded Credentials", "security"])),
                rule("python.lang.best-practice.print-used", json!(["best-practice"])),
            ]}},
            "results": [
                hit(0, "python.lang.security.audit.formatted-sql-query", 5, "warning", "Hard-coded \u{1b}[31mcredential in\nSQL"),
                hit(0, "python.lang.security.audit.formatted-sql-query", 5, "warning", "Hard-coded \u{1b}[31mcredential in\nSQL"),
                hit(1, "python.lang.best-practice.print-used", 9, "note", "Markup built from input"),
                { "ruleId": "python.lang.best-practice.print-used", "kind": "pass", "message": {"text": "fine"} },
            ]
        }]
    })
}

fn imported<'a>(u: &'a teer::review::model::Unit, tool: &str) -> Vec<&'a Fact> {
    u.security
        .iter()
        .filter(|f| matches!(&f.origin, Origin::Import { tool: t, .. } if t == tool))
        .collect()
}

#[test]
fn sarif_results_are_pinned_to_the_units_they_fall_in() {
    isolate_env();
    let r = repo();
    let mut review = build(&r, Vec::new());
    // Imports alone: whatever teer finds itself is another test's.
    for u in &mut review.units {
        u.security.clear();
    }
    let before_search = assess(&review, unit(&review, "search")).level;
    assert!(before_search < Level::Attention, "{before_search:?}");

    let mut all = sarif::read(&codeql(&r.path()), &r.path());
    all.extend(sarif::read(&semgrep(), &r.path()));
    // Two CodeQL results in place, two in code the change didn't touch;
    // Semgrep's duplicate and its passing result never come.
    assert_eq!(all.len(), 6, "{all:#?}");
    assert_eq!(sarif::pin(&mut review, all), 2, "outside the change");

    let search = unit(&review, "search");
    let codeql_facts = imported(search, "CodeQL");
    assert_eq!(codeql_facts.len(), 1, "{:#?}", search.security);
    let f = codeql_facts[0];
    assert_eq!(f.class, Class::Injection);
    assert_eq!(f.weight, Weight::Attention);
    assert_eq!(f.text, "This query depends on a user-provided value.");
    assert_eq!(f.subject, "py/sql-injection");
    assert_eq!(f.side, Side::New);
    assert_eq!(f.lines.map(|l| (l.start, l.end)), Some((6, 6)));
    assert_eq!(
        f.origin,
        Origin::Import {
            tool: "CodeQL".into(),
            rule: "py/sql-injection".into()
        }
    );
    let semgrep_facts = imported(search, "Semgrep OSS");
    assert_eq!(semgrep_facts.len(), 1, "duplicates come once");
    assert_eq!(semgrep_facts[0].class, Class::Secret, "CWE-798");
    assert_eq!(semgrep_facts[0].weight, Weight::Look, "a warning");
    // Sanitized: no escape, one line.
    assert_eq!(semgrep_facts[0].text, "Hard-coded credential in SQL");
    assert_eq!(
        semgrep_facts[0].lines.map(|l| (l.start, l.end)),
        Some((5, 6))
    );

    let greet = unit(&review, "greet");
    let xss = imported(greet, "CodeQL");
    assert_eq!(xss.len(), 1, "the file:// path is in the repository");
    assert_eq!(
        (xss[0].class, xss[0].weight),
        (Class::Injection, Weight::Look)
    );
    assert_eq!(xss[0].text, "Cross-site scripting from name.");
    let note = imported(greet, "Semgrep OSS");
    assert_eq!(
        (note[0].class, note[0].weight),
        (Class::Exposure, Weight::Note),
        "no CWE, no injection word: data exposure"
    );

    // Imports raise levels, with their text as the reason.
    let a = assess(&review, unit(&review, "search"));
    assert_eq!(a.level, Level::Attention);
    assert!(
        a.reasons
            .contains(&"security: This query depends on a user-provided value.".to_string()),
        "{:?}",
        a.reasons
    );
    assert!(assess(&review, unit(&review, "greet")).level >= Level::Look);
}

#[test]
fn an_import_without_a_unit_of_its_own_goes_to_the_whole_file() {
    isolate_env();
    let r = FixtureRepo::new();
    r.write("deploy.yml", "on: push\n").commit("init");
    r.write("deploy.yml", "on: pull_request_target\njobs: {}\n");
    let mut review = build(&r, Vec::new());
    let whole = review
        .units
        .iter()
        .position(|u| u.kind == teer::review::model::UnitKind::File)
        .expect("a whole-file unit");
    let found = |lines| Imported {
        tool: "zizmor".into(),
        rule: "dangerous-triggers".into(),
        file: "deploy.yml".into(),
        lines,
        weight: Weight::Attention,
        class: Class::Ci,
        text: "pull_request_target is almost always used insecurely".into(),
    };
    let n = sarif::pin(
        &mut review,
        vec![
            found(None),
            found(Some(teer::review::model::Range::new(1, 1))),
        ],
    );
    assert_eq!(n, 0);
    assert_eq!(imported(&review.units[whole], "zizmor").len(), 2);
}

/// `teer --json --sarif <file>`: what the review prints carries the
/// imports; a file that isn't SARIF stops teer before anything runs.
#[test]
fn the_sarif_flag_imports_into_the_review() {
    let env = isolate_env();
    let r = repo();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("codeql.sarif");
    std::fs::write(&file, codeql(&r.path()).to_string()).unwrap();
    let run = |args: &[&str]| {
        Command::new(teer_bin())
            .args(args)
            .arg("-C")
            .arg(r.path())
            .env("TEER_DATA_DIR", env.path().join("data"))
            .env("TEER_CONFIG", env.path().join("config.toml"))
            .env("TEER_GH", env.path().join("no-gh"))
            .output()
            .unwrap()
    };
    let out = run(&["--json", "--no-agent", "--sarif", file.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let search = v["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["symbol"] == "search")
        .unwrap();
    let fact = search["security"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["origin"]["from"] == "import")
        .unwrap_or_else(|| panic!("no import: {search}"));
    assert_eq!(fact["origin"]["tool"], "CodeQL");
    assert_eq!(fact["origin"]["rule"], "py/sql-injection");
    assert_eq!(fact["weight"], "attention");
    assert_eq!(fact["class"], "injection");
    assert_eq!(fact["lines"], json!({"start": 6, "end": 6}));
    // Outside the change: not in the review.
    let all = v["units"].to_string();
    assert!(!all.contains("py/path-injection"), "{all}");

    std::fs::write(dir.path().join("bad.sarif"), "{\"version\": \"2.1.0\"}").unwrap();
    let out = run(&[
        "--json",
        "--no-agent",
        "--sarif",
        dir.path().join("bad.sarif").to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--sarif"), "{}", stderr(&out));
    assert!(stderr(&out).contains("not SARIF"), "{}", stderr(&out));
}

/// A re-run builds the review again with the same options: the SARIF
/// files are read again and their facts are back.
#[tokio::test]
async fn imports_survive_a_rerun() {
    isolate_env();
    let r = repo();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("codeql.sarif");
    std::fs::write(&file, codeql(&r.path()).to_string()).unwrap();
    let opts = BuildOptions {
        sarif: vec![file.clone()],
        ..Default::default()
    };
    let review = build_review(&r.path(), &opts).unwrap();
    assert_eq!(imported(unit(&review, "search"), "CodeQL").len(), 1);
    let mut app = App::new(review, Config::default(), Keymap::default());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let rt = Runtime {
        adapter: None,
        tx,
        build: opts,
        tools: Default::default(),
    };
    // The fix: `greet` goes back, and the SARIF file names it no more.
    r.write(
        "views.py",
        &VIEWS_NEW.replace("\"<b>hi \" + name + \"</b>\"", "\"hi \" + name"),
    );
    rt.exec(&mut app, Effect::Rerun);
    let ev = tokio::time::timeout(Duration::from_secs(60), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let AppEvent::Rerun(Ok(new)) = ev else {
        panic!("{ev:?}")
    };
    assert_eq!(imported(unit(&new, "search"), "CodeQL").len(), 1);
    assert!(new.units.iter().all(|u| u.symbol != "greet"));
}

// ------------------------------------------------------------ code scanning

const ALERTS: &str = r#"[
  {"number": 4, "state": "open",
   "rule": {"id": "py/path-injection", "name": "py/path-injection", "severity": "error",
            "security_severity_level": "critical", "description": "Uncontrolled data used in path expression",
            "tags": ["security", "external/cwe/cwe-022"]},
   "tool": {"name": "CodeQL"},
   "most_recent_instance": {"ref": "refs/pull/7/head", "state": "open",
     "location": {"path": "shop.py", "start_line": 2, "end_line": 2},
     "message": {"text": "This path depends on a [user-provided value](1)."}}},
  {"number": 5, "state": "open",
   "rule": {"id": "py/weak-crypto", "severity": "warning", "security_severity_level": "medium",
            "description": "Weak hashing", "tags": ["security", "external/cwe/cwe-327"]},
   "tool": {"name": "CodeQL"},
   "most_recent_instance": {"location": {"path": "shop.py", "start_line": 2}, "message": {"text": ""}}},
  {"number": 6, "state": "open",
   "rule": {"id": "js/log", "severity": "note", "security_severity_level": "low", "tags": []},
   "tool": {"name": "CodeQL"},
   "most_recent_instance": {"location": {"path": "../outside.py", "start_line": 1}, "message": {"text": "x"}}}
]"#;

fn pr_info(host: &str) -> teer::review::model::PrInfo {
    teer::pr::parse_url(&format!("https://{host}/acme/shop/pull/7")).unwrap()
}

#[test]
fn code_scanning_alerts_become_imports() {
    let d = tempfile::tempdir().unwrap();
    let gh = d.path().join("gh");
    let log = d.path().join("gh.log");
    script(
        &gh,
        &format!(
            "echo \"$@\" >> '{}'\ncat <<'EOF'\n{ALERTS}\nEOF",
            log.display()
        ),
    );
    let found = teer::pr::fetch_alerts(
        gh.as_os_str(),
        d.path(),
        &pr_info("github.com"),
        Duration::from_secs(10),
    )
    .unwrap();
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls.trim(),
        "api repos/acme/shop/code-scanning/alerts?ref=refs/pull/7/head&state=open&per_page=100"
    );
    // The third is outside the repository.
    assert_eq!(found.len(), 2, "{found:#?}");
    assert_eq!(found[0].tool, "CodeQL");
    assert_eq!(found[0].rule, "py/path-injection");
    assert_eq!(found[0].file, PathBuf::from("shop.py"));
    assert_eq!(found[0].weight, Weight::Attention, "critical");
    assert_eq!(found[0].class, Class::Injection, "CWE-22");
    assert_eq!(found[0].text, "This path depends on a user-provided value.");
    assert_eq!(
        (found[1].weight, found[1].class),
        (Weight::Look, Class::Crypto)
    );
    assert_eq!(
        found[1].text, "Weak hashing",
        "no message: the rule's description"
    );

    // GitHub Enterprise: the host goes to gh.
    std::fs::remove_file(&log).unwrap();
    teer::pr::fetch_alerts(
        gh.as_os_str(),
        d.path(),
        &pr_info("git.corp.example"),
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(
        std::fs::read_to_string(&log)
            .unwrap()
            .starts_with("api --hostname git.corp.example repos/acme/shop/"),
    );

    // Code scanning off (404) or not allowed (403), or no gh: an error to
    // log, nothing else.
    script(&gh, "echo 'gh: Not Found (HTTP 404)' >&2\nexit 1");
    let e = teer::pr::fetch_alerts(
        gh.as_os_str(),
        d.path(),
        &pr_info("github.com"),
        Duration::from_secs(10),
    )
    .unwrap_err();
    assert!(e.contains("404"), "{e}");
    let e = teer::pr::fetch_alerts(
        d.path().join("no-gh").as_os_str(),
        d.path(),
        &pr_info("github.com"),
        Duration::from_secs(10),
    )
    .unwrap_err();
    assert!(e.contains("not installed"), "{e}");
}

/// A pull request on a local "GitHub": a bare repository reached through
/// `url.<path>.insteadOf`, the PR at `refs/pull/<n>/head` and its merge
/// ref. `shop.py`'s `price` changes.
struct PrFixture {
    repo: FixtureRepo,
    _bare: tempfile::TempDir,
    url: String,
}

fn pr_fixture(n: u32) -> PrFixture {
    let repo = FixtureRepo::new();
    repo.write("shop.py", "def price(qty):\n    return qty * 10\n")
        .commit("init");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("shop.py", "def price(qty):\n    return open(qty).read()\n")
        .commit("pr change");
    repo.git(&["checkout", "-q", "main"]);
    let bare = tempfile::tempdir().unwrap();
    let out = Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(bare.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let bare_path = bare.path().canonicalize().unwrap();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/shop.git",
    ]);
    repo.git(&[
        "config",
        &format!("url.{}.insteadOf", bare_path.display()),
        "https://github.com/acme/shop.git",
    ]);
    repo.git(&["push", "-q", "origin", "main"]);
    repo.git(&[
        "push",
        "-q",
        "origin",
        &format!("feature:refs/pull/{n}/head"),
    ]);
    repo.git(&["checkout", "-q", "--detach", "main"]);
    repo.git(&["merge", "-q", "--no-ff", "feature", "-m", "merge"]);
    repo.git(&["push", "-q", "origin", &format!("HEAD:refs/pull/{n}/merge")]);
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["branch", "-q", "-D", "feature"]);
    PrFixture {
        repo,
        _bare: bare,
        url: format!("https://github.com/acme/shop/pull/{n}"),
    }
}

/// `teer <url> --json`: the PR's code-scanning alerts are pinned to its
/// units; without them (gh fails) the review goes on.
#[test]
fn a_pull_requests_code_scanning_alerts_are_pinned_to_its_units() {
    // A PR number no other test uses: tests share one data dir.
    let fx = pr_fixture(907);
    let sb = Sandbox::new();
    let alerts = ALERTS.replace("refs/pull/7/head", "refs/pull/907/head");
    sb.gh(&format!(
        "echo \"$@\" >> \"$(dirname \"$0\")/gh.log\"\ncase \"$1\" in\n  api) cat <<'EOF'\n{alerts}\nEOF\n  ;;\n  *) exit 1 ;;\nesac"
    ));
    let v = sb.json(&fx.repo.path(), &[&fx.url, "--json", "--no-agent"]);
    assert!(
        sb.gh_calls()
            .contains("api repos/acme/shop/code-scanning/alerts?ref=refs/pull/907/head"),
        "{}",
        sb.gh_calls()
    );
    let price = v["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["symbol"] == "price")
        .unwrap();
    let facts = price["security"].as_array().unwrap();
    let rules: Vec<&str> = facts
        .iter()
        .filter(|f| f["origin"]["from"] == "import")
        .map(|f| f["origin"]["rule"].as_str().unwrap())
        .collect();
    assert_eq!(rules, ["py/path-injection", "py/weak-crypto"], "{price}");

    // No code scanning: nothing imported, the review is there all the same.
    sb.gh("echo 'gh: Not Found (HTTP 404)' >&2\nexit 1");
    let v = sb.json(&fx.repo.path(), &[&fx.url, "--json", "--no-agent"]);
    let price = v["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["symbol"] == "price")
        .unwrap();
    assert!(
        price["security"]
            .as_array()
            .is_none_or(|f| f.iter().all(|f| f["origin"]["from"] != "import")),
        "{price}"
    );
}

// ------------------------------------------------------------ SARIF out

fn cite(file: &str, line: u32) -> Cite {
    Cite {
        file: PathBuf::from(file),
        start: line,
        end: line,
    }
}

fn finding(title: &str, verified: bool) -> Finding {
    Finding {
        class: Class::Injection,
        severity: Risk::High,
        confidence: Risk::Medium,
        title: title.into(),
        source: cite("views.py", 4),
        path: vec![Hop {
            symbol: "search".into(),
            cite: cite("views.py", 5),
        }],
        sink: cite("views.py", 6),
        missing: "a bound parameter".into(),
        exploit: "q = \"' OR 1=1 --\" returns every row".into(),
        path_verified: verified,
        second: None,
        proof: None,
    }
}

/// The parts of SARIF 2.1.0 a consumer needs, checked one by one.
fn assert_sarif_shape(v: &Value) {
    assert_eq!(v["version"], "2.1.0");
    assert!(v["$schema"].as_str().unwrap().contains("sarif-2.1.0"));
    let runs = v["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    let driver = &runs[0]["tool"]["driver"];
    assert_eq!(driver["name"], "teer");
    let rules = driver["rules"].as_array().unwrap();
    assert_eq!(rules.len(), Class::ALL.len(), "one rule per class");
    let ids: Vec<&str> = rules.iter().map(|r| r["id"].as_str().unwrap()).collect();
    for (rule, class) in rules.iter().zip(Class::ALL) {
        assert_eq!(rule["id"], format!("teer/{}", class.id()));
        assert!(
            !rule["shortDescription"]["text"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        assert!(!rule["fullDescription"]["text"].as_str().unwrap().is_empty());
    }
    for r in runs[0]["results"].as_array().unwrap() {
        let id = r["ruleId"].as_str().unwrap();
        let i = r["ruleIndex"].as_u64().unwrap() as usize;
        assert_eq!(ids[i], id, "{r}");
        assert!(
            ["error", "warning", "note"].contains(&r["level"].as_str().unwrap()),
            "{r}"
        );
        assert!(!r["message"]["text"].as_str().unwrap().is_empty(), "{r}");
        let loc = &r["locations"][0]["physicalLocation"];
        let check = |loc: &Value| {
            let uri = loc["artifactLocation"]["uri"].as_str().unwrap();
            assert!(
                !uri.is_empty() && !uri.starts_with('/') && !uri.contains("://"),
                "{uri}"
            );
            if !loc["region"].is_null() {
                let s = loc["region"]["startLine"].as_u64().unwrap();
                assert!(
                    s >= 1 && loc["region"]["endLine"].as_u64().unwrap() >= s,
                    "{loc}"
                );
            }
        };
        check(loc);
        for flow in r["codeFlows"].as_array().into_iter().flatten() {
            let steps = flow["threadFlows"][0]["locations"].as_array().unwrap();
            assert!(!steps.is_empty());
            for s in steps {
                check(&s["location"]["physicalLocation"]);
            }
        }
    }
}

#[test]
fn teer_notes_sarif_writes_teers_facts_and_standing_findings() {
    let env = isolate_env();
    let r = repo();
    let mut review = build(&r, Vec::new());
    // Only the facts set here.
    for u in &mut review.units {
        u.security.clear();
    }
    // Nothing found yet: a valid file without results.
    let empty = sarif::write(&review);
    assert_sarif_shape(&empty);
    assert_eq!(empty["runs"][0]["results"], json!([]));

    let search = review
        .units
        .iter_mut()
        .find(|u| u.symbol == "search")
        .unwrap();
    search.security = vec![
        Fact {
            class: Class::Injection,
            weight: Weight::Look,
            text: "user input reaches a raw query".into(),
            side: Side::New,
            lines: Some(teer::review::model::Range::new(5, 6)),
            subject: "db.query".into(),
            origin: Origin::Teer,
        },
        Fact {
            class: Class::Authorization,
            weight: Weight::Attention,
            text: "isGranted is gone".into(),
            side: Side::Old,
            lines: Some(teer::review::model::Range::new(5, 5)),
            subject: String::new(),
            origin: Origin::Teer,
        },
        // Another tool's: it goes back to that tool, not out again.
        Imported {
            tool: "CodeQL".into(),
            rule: "py/sql-injection".into(),
            file: "views.py".into(),
            lines: None,
            weight: Weight::Attention,
            class: Class::Injection,
            text: "from CodeQL".into(),
        }
        .fact(),
    ];
    let mut proven = finding("the search query is built from the input", true);
    proven.proof = Some(Proof::Proven {
        case: "or: ' OR 1=1".into(),
        before: "[]".into(),
        after: "[all]".into(),
    });
    search.dive.results.insert(
        DiveKind::Security,
        DiveResult {
            audit: Some(Audit {
                findings: vec![proven, finding("a path no graph shows", false)],
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    teer::review::store::save(&review).unwrap();

    let out = Command::new(teer_bin())
        .args(["notes", "--sarif", "-C"])
        .arg(r.path())
        .env("TEER_DATA_DIR", env.path().join("data"))
        .env("TEER_CONFIG", env.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_sarif_shape(&v);
    let results = v["runs"][0]["results"].as_array().unwrap();
    let texts: Vec<&str> = results
        .iter()
        .map(|r| r["message"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        texts.len(),
        3,
        "two facts of teer's and the standing finding: {texts:?}"
    );
    assert!(!texts.iter().any(|t| t.contains("CodeQL")), "{texts:?}");

    let fact = &results[0];
    assert_eq!(fact["ruleId"], "teer/injection");
    assert_eq!(fact["level"], "warning");
    assert_eq!(
        fact["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        "views.py"
    );
    assert_eq!(
        fact["locations"][0]["physicalLocation"]["region"]["startLine"],
        5
    );
    // A check that is gone: at the unit's new lines, the old ones named.
    let gone = &results[1];
    assert_eq!(gone["ruleId"], "teer/authorization");
    assert_eq!(gone["level"], "error");
    assert_eq!(gone["properties"]["oldLines"], "5-5");
    assert_eq!(
        gone["locations"][0]["physicalLocation"]["region"]["startLine"],
        4
    );

    let f = &results[2];
    assert_eq!(f["level"], "error");
    assert_eq!(f["properties"]["proof"], "proven");
    assert!(
        f["message"]["text"]
            .as_str()
            .unwrap()
            .starts_with("the search query is built from the input. Missing: a bound parameter."),
        "{f}"
    );
    assert_eq!(
        f["locations"][0]["physicalLocation"]["region"]["startLine"], 6,
        "at the sink"
    );
    let steps = f["codeFlows"][0]["threadFlows"][0]["locations"]
        .as_array()
        .unwrap();
    let said: Vec<&str> = steps
        .iter()
        .map(|s| s["location"]["message"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(said, ["source", "search", "sink"]);
}

// ------------------------------------------------------------ prove it

const ASSETS_OLD: &str = "ASSETS = {\"a1\": {\"owner\": \"acme\", \"name\": \"plan-a\"}, \"b1\": {\"owner\": \"globex\", \"name\": \"plan-b\"}}


def load(asset_id, tenant):
    asset = ASSETS[asset_id]
    if asset[\"owner\"] != tenant:
        raise PermissionError(\"not yours\")
    return asset[\"name\"]
";

const ASSETS_NEW: &str = "ASSETS = {\"a1\": {\"owner\": \"acme\", \"name\": \"plan-a\"}, \"b1\": {\"owner\": \"globex\", \"name\": \"plan-b\"}}


def load(asset_id, tenant):
    asset = ASSETS[asset_id]
    return asset[\"name\"]
";

const ATTACK: &str = r#"import json, sys
sys.path.insert(0, ".")
from assets import load

for name, args in [("other_tenant", ("b1", "acme")), ("own_asset", ("a1", "acme"))]:
    try:
        print("TEER_CASE " + json.dumps({"case": name, "output": load(*args)}))
    except Exception as e:
        print("TEER_CASE " + json.dumps({"case": name, "error": type(e).__name__ + ": " + str(e)}))
"#;

/// A review of the owner check taken out of `load`, with three findings
/// on it, in an app with an agent.
fn proving_app() -> (FixtureRepo, App) {
    isolate_env();
    let r = FixtureRepo::new();
    r.write("assets.py", ASSETS_OLD).commit("base");
    r.write("assets.py", ASSETS_NEW);
    let mut review = build_review(&r.path(), &BuildOptions::default()).unwrap();
    review.agent_session = Some(SessionRef::fresh("mock", review.repo_root.clone()));
    let f = |class: Class, title: &str| Finding {
        class,
        severity: Risk::High,
        confidence: Risk::High,
        title: title.into(),
        source: cite("assets.py", 4),
        path: Vec::new(),
        sink: cite("assets.py", 6),
        missing: "the owner check".into(),
        exploit: "tenant acme loads globex's asset b1".into(),
        path_verified: true,
        second: None,
        proof: None,
    };
    let load = review
        .units
        .iter_mut()
        .find(|u| u.symbol == "load")
        .unwrap();
    load.dive.results.insert(
        DiveKind::Security,
        DiveResult {
            audit: Some(Audit {
                findings: vec![
                    f(Class::Authorization, "another tenant's asset loads"),
                    f(Class::Exposure, "an own asset leaks its name"),
                    f(Class::Injection, "the asset id reaches the database"),
                ],
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let mut app = App::new(review, Config::default(), Keymap::default());
    app.has_agent = true;
    app.sel = app
        .order
        .iter()
        .position(|&i| app.review.units[i].symbol == "load")
        .unwrap();
    (r, app)
}

/// `<space>p` on `app`, carried out by a runtime with `m`, until what it
/// showed comes back and is applied.
async fn prove(app: &mut App, m: &MockAdapter) -> Vec<(usize, Proof)> {
    let effects = common::v2::press(app, "<space>p");
    assert!(
        matches!(&effects[..], [Effect::ProveFindings { .. }]),
        "{effects:?} ({})",
        app.status
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let rt = Runtime {
        adapter: Some(Arc::new(m.clone())),
        tx,
        build: BuildOptions::default(),
        tools: Default::default(),
    };
    for e in effects {
        rt.exec(app, e);
    }
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(120), rx.recv())
            .await
            .expect("the proof finishes")
            .unwrap();
        let proofs = match &ev {
            AppEvent::Dive(DiveEvent::Proved { proofs, .. }) => Some(proofs.clone()),
            _ => None,
        };
        app.on_app_event(ev);
        if let Some(p) = proofs {
            return p;
        }
    }
}

fn audit_of(app: &App) -> Audit {
    let u = app
        .review
        .units
        .iter()
        .find(|u| u.symbol == "load")
        .unwrap();
    u.dive.results[&DiveKind::Security].audit.clone().unwrap()
}

/// Whatever stops the whole run (no sandbox off macOS, the agent failing
/// on it), every finding comes back unproven with why, and `proving` clears.
#[tokio::test]
async fn a_proof_that_cannot_run_leaves_every_finding_unproven() {
    let (_r, mut app) = proving_app();
    let m = MockAdapter::new(|_, _| anyhow::bail!("the agent is out of credit"));
    let proofs = prove(&mut app, &m).await;
    assert_eq!(proofs.len(), 3);
    let why = if teer::sandbox::available() {
        "the agent is out of credit"
    } else {
        teer::sandbox::UNAVAILABLE
    };
    for (i, p) in proofs.iter().enumerate() {
        assert_eq!(*p, (i, Proof::Unproven { why: why.into() }));
    }
    let audit = audit_of(&app);
    assert!(!audit.proving);
    assert!(
        audit
            .findings
            .iter()
            .all(|f| matches!(&f.proof, Some(Proof::Unproven { .. })))
    );
}

fn has_python() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The attacker's cases run old against new in the sandbox: one comes out
/// as the agent predicted (proven), one doesn't (not shown), and one the
/// agent couldn't write (unproven, with its reason). macOS only.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn attacker_cases_prove_what_they_show() {
    if !has_python() {
        return;
    }
    let env = isolate_env();
    let (r, mut app) = proving_app();
    let m = MockAdapter::new(|prompt, schema| {
        assert!(prompt.starts_with("DIVE:Prove"), "{prompt}");
        assert!(
            prompt.contains("TEER_CASE"),
            "the protocol is in the prompt"
        );
        assert!(
            prompt.contains("1. [auth, high] another tenant's asset loads"),
            "{prompt}"
        );
        assert!(prompt.contains("3. [injection, high]"), "{prompt}");
        assert!(schema["properties"]["cannot"].is_object());
        Ok(json!({
            "files": [{"path": "teer_attack.py", "content": ATTACK}],
            "command": ["python3", "teer_attack.py"],
            "cases": [
                {"finding": 1, "name": "other_tenant", "input": "acme asks for b1", "expect": "different",
                 "why": "with the change it returns plan-b; before, PermissionError"},
                {"finding": 2, "name": "own_asset", "input": "acme asks for a1", "expect": "different",
                 "why": "with the change the name leaks"}
            ],
            "cannot": [{"finding": 3, "why": "needs a database"}]
        }))
    });
    let proofs = prove(&mut app, &m).await;
    assert_eq!(proofs.len(), 3, "{proofs:#?}");
    let audit = audit_of(&app);
    assert!(!audit.proving);
    let proof = |i: usize| audit.findings[i].proof.clone().unwrap();
    match proof(0) {
        Proof::Proven {
            case,
            before,
            after,
        } => {
            assert!(case.starts_with("other_tenant: acme asks for b1"), "{case}");
            assert_eq!(before, "error: PermissionError: not yours");
            assert_eq!(after, "\"plan-b\"");
        }
        other => panic!("{other:?}"),
    }
    match proof(1) {
        Proof::NotShown { before, after, .. } => {
            assert_eq!(
                (before.as_str(), after.as_str()),
                ("\"plan-a\"", "\"plan-a\"")
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        proof(2),
        Proof::Unproven {
            why: "needs a database".into()
        }
    );
    // Never the web; the receipt is kept; the user's checkout untouched.
    assert_eq!(*m.web_calls.lock().unwrap(), [false]);
    let receipts = env.path().join("data/proofs");
    assert!(receipts.is_dir(), "{}", receipts.display());
    assert!(!r.path().join("teer_attack.py").exists());
    assert_eq!(r.file("assets.py"), ASSETS_NEW);

    // Every finding ran: nothing is left to prove.
    assert!(common::v2::press(&mut app, "<space>p").is_empty());
    assert!(
        app.status.starts_with("Nothing left to prove"),
        "{}",
        app.status
    );
}
