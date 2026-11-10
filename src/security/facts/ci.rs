//! CI workflows and infrastructure as code: GitHub Actions workflows (and
//! composite actions), `.gitlab-ci.yml`, Terraform. Read from the added
//! lines, with the file around them.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::fact;
use super::lines::Added;
use crate::security::model::{Class, Fact, Weight};

/// A GitHub Actions workflow or composite action.
fn is_github_workflow(path: &Path) -> bool {
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let yaml = path.extension().is_some_and(|e| e == "yml" || e == "yaml");
    let in_github = parts.iter().any(|p| p == ".github");
    yaml && in_github
        && (parts.iter().any(|p| p == "workflows")
            || parts.last().is_some_and(|n| n.starts_with("action.")))
}

fn is_gitlab_ci(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == ".gitlab-ci.yml")
}

fn is_terraform(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "tf")
}

/// Is this file one [`facts`] reads?
pub(crate) fn applies(path: &Path) -> bool {
    is_github_workflow(path)
        || is_gitlab_ci(path)
        || is_terraform(path)
        || path.extension().is_some_and(|e| e == "json")
}

/// Text fields of an event someone outside the repository writes.
static UNTRUSTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(github\.event\.(?:issue|pull_request|comment|review|review_comment|discussion|discussion_comment|pages|commits|head_commit|workflow_run|release)[\w.*\[\]'-]*|github\.head_ref)").unwrap()
});
const TEXT_FIELDS: [&str; 13] = [
    "title",
    "body",
    "ref",
    "label",
    "message",
    "name",
    "email",
    "page_name",
    "head_branch",
    "default_branch",
    "description",
    "tag_name",
    "head_ref",
];
static EXPRESSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$\{\{([^}]*)\}\}").unwrap());
static PERMISSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:permissions:\s*(write-all)\b|((?:contents|id-token|packages|actions|pull-requests|issues|deployments|security-events|statuses|checks|pages|attestations|discussions|repository-projects):\s*write)\b)").unwrap()
});
static USES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r##"^\s*(?:-\s*)?uses:\s*['"]?([^@\s'"#]+)@([^\s'"#]+)"##).unwrap()
});
static SHA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{40}$").unwrap());
static SECRET_USE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bsecrets\.([A-Za-z_][A-Za-z0-9_]*)").unwrap());
static PR_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"github\.event\.pull_request\.head\.(?:sha|ref)|github\.head_ref").unwrap()
});
static OPEN_CIDR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""(?:0\.0\.0\.0/0|::/0)""#).unwrap());
static PUBLIC_ACL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bacl\s*=\s*"(public-read(?:-write)?)""#).unwrap());
static IAM_WILDCARD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)"?\bactions?"?\s*[:=]\s*\[?\s*"\*""#).unwrap());
static INGRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)^\s*ingress\s*[{=]|resource\s+"[^"]*ingress[^"]*"|type\s*=\s*"ingress"|source_ranges\s*="#).unwrap()
});
static EGRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)^\s*egress\s*[{=]|resource\s+"[^"]*egress[^"]*"|type\s*=\s*"egress"|destination_ranges\s*="#).unwrap()
});

/// Which lines of a workflow are inside a `run:` (or `script:`) block.
fn script_lines(lines: &[&str]) -> Vec<bool> {
    let mut out = vec![false; lines.len()];
    let mut block: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        let indent = l.len() - l.trim_start().len();
        if let Some(b) = block {
            if l.trim().is_empty() || indent > b {
                out[i] = true;
                continue;
            }
            block = None;
        }
        let key = l.trim_start().trim_start_matches("- ").trim_start();
        if let Some(rest) = key
            .strip_prefix("run:")
            .or_else(|| key.strip_prefix("script:"))
        {
            out[i] = true;
            let rest = rest.trim();
            if rest.is_empty() || rest.starts_with('|') || rest.starts_with('>') {
                // Its text is indented past the key: `env:` beside it isn't.
                block = Some(l.len() - key.len());
            }
        }
    }
    out
}

/// The workflow runs on `pull_request_target` (outside comments).
fn on_pr_target(text: &str) -> bool {
    text.lines().any(|l| {
        l.split('#')
            .next()
            .unwrap_or("")
            .contains("pull_request_target")
    })
}

/// Facts about CI and infrastructure from a unit's added lines.
pub(crate) fn facts(a: &Added) -> Vec<Fact> {
    let mut out = Vec::new();
    if is_github_workflow(a.path) {
        workflow(a, &mut out);
    }
    if is_terraform(a.path) {
        terraform(a, &mut out);
    }
    if is_terraform(a.path) || a.path.extension().is_some_and(|e| e == "json") {
        for &l in &a.lines {
            if IAM_WILDCARD.is_match(a.text(l)) {
                out.push(fact(
                    Class::Ci,
                    Weight::Look,
                    "IAM policy allows every action (*)".into(),
                    l,
                    "iam",
                ));
            }
        }
    }
    out
}

fn workflow(a: &Added, out: &mut Vec<Fact>) {
    let target = on_pr_target(a.new_text);
    let target_added = target && !on_pr_target(a.old_text);
    if target_added {
        let at = a
            .lines
            .iter()
            .copied()
            .find(|&l| a.text(l).contains("pull_request_target"))
            .or_else(|| {
                (1..=a.new_lines.len() as u32).find(|&l| a.text(l).contains("pull_request_target"))
            })
            .unwrap_or(1);
        out.push(fact(
            Class::Ci,
            Weight::Attention,
            "pull_request_target added: a fork's PR runs with this repo's secrets and write token"
                .into(),
            at,
            "pull_request_target",
        ));
    }
    let scripts = script_lines(&a.new_lines);
    let mut unpinned: Vec<String> = Vec::new();
    for &l in &a.lines {
        let text = a.text(l);
        let code = text.split(" #").next().unwrap_or(text);
        let in_script = scripts.get(l as usize - 1).copied().unwrap_or(false);
        // `${{ github.event.pull_request.title }}` pasted into a script.
        if in_script {
            for e in EXPRESSION.captures_iter(code) {
                if let Some(m) = UNTRUSTED.find(&e[1]) {
                    let expr = m.as_str().trim_end_matches(['.', '\'', '"']);
                    let last = expr.rsplit('.').next().unwrap_or(expr);
                    if TEXT_FIELDS.contains(&last) {
                        out.push(fact(
                            Class::Ci,
                            Weight::Attention,
                            format!("{expr} is pasted into a run: script (script injection)"),
                            l,
                            expr,
                        ));
                    }
                }
            }
        }
        if target {
            if let Some(c) = SECRET_USE.captures(code) {
                let name = format!("secrets.{}", &c[1]);
                out.push(fact(
                    Class::Ci,
                    Weight::Attention,
                    format!("{name} used in a pull_request_target workflow"),
                    l,
                    &name,
                ));
            }
            if !in_script && PR_HEAD.is_match(code) {
                out.push(fact(
                    Class::Ci,
                    Weight::Attention,
                    "checks out the PR's head in a pull_request_target workflow".into(),
                    l,
                    "pull_request_target",
                ));
            }
        }
        if let Some(c) = PERMISSION.captures(code) {
            let what = c
                .get(1)
                .or_else(|| c.get(2))
                .map(|m| m.as_str())
                .unwrap_or("");
            out.push(fact(
                Class::Ci,
                Weight::Look,
                format!("permissions widened: {what}"),
                l,
                what,
            ));
        }
        if let Some(c) = USES.captures(code) {
            let action = &c[1];
            let reference = &c[2];
            if !action.starts_with("./")
                && !action.starts_with("docker://")
                && !SHA.is_match(reference)
                && !unpinned.iter().any(|u| u == action)
            {
                unpinned.push(action.to_string());
                out.push(fact(
                    Class::Ci,
                    Weight::Note,
                    format!("{action}@{reference} is not pinned to a commit sha"),
                    l,
                    action,
                ));
            }
        }
    }
}

fn terraform(a: &Added, out: &mut Vec<Fact>) {
    for &l in &a.lines {
        let text = a.text(l);
        if OPEN_CIDR.is_match(text) {
            // The block it is in: the nearest ingress or egress above.
            let from = l.saturating_sub(15).max(1);
            let ingress = (from..=l).rev().find_map(|i| {
                let t = a.text(i);
                if INGRESS.is_match(t) || (i == l && t.contains("source_ranges")) {
                    Some(true)
                } else if EGRESS.is_match(t) {
                    Some(false)
                } else {
                    None
                }
            });
            if ingress == Some(true) {
                out.push(fact(
                    Class::Ci,
                    Weight::Look,
                    "ingress open to the internet (0.0.0.0/0)".into(),
                    l,
                    "0.0.0.0/0",
                ));
            }
        }
        if let Some(c) = PUBLIC_ACL.captures(text) {
            out.push(fact(
                Class::Ci,
                Weight::Look,
                format!("bucket made public (acl = \"{}\")", &c[1]),
                l,
                "acl",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(path: &str, old: &str, new: &str) -> Vec<Fact> {
        facts(&Added::of(Path::new(path), old, new))
    }

    const OLD: &str = "on:\n  pull_request:\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@8e5e7e5ab8b370d6c329ec480221332ada57f0ab\n      - run: make test\n";

    #[test]
    fn pull_request_target_and_its_dangers() {
        let new = "on:\n  pull_request_target:\njobs:\n  test:\n    runs-on: ubuntu-latest\n    permissions:\n      contents: write\n    steps:\n      - uses: actions/checkout@v4\n        with:\n          ref: ${{ github.event.pull_request.head.sha }}\n      - run: |\n          echo \"${{ github.event.pull_request.title }}\"\n          npm publish\n        env:\n          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}\n";
        let f = run(".github/workflows/ci.yml", OLD, new);
        let got: Vec<(&str, Weight)> = f.iter().map(|x| (x.subject.as_str(), x.weight)).collect();
        assert_eq!(
            got,
            [
                ("pull_request_target", Weight::Attention),
                ("contents: write", Weight::Look),
                ("actions/checkout", Weight::Note),
                ("pull_request_target", Weight::Attention),
                ("github.event.pull_request.title", Weight::Attention),
                ("secrets.NPM_TOKEN", Weight::Attention),
            ],
            "{f:#?}"
        );
        assert_eq!(f[0].lines.unwrap().start, 2);
    }

    #[test]
    fn a_plain_workflow_change_says_nothing() {
        let new = OLD.replace("make test", "make test lint");
        assert!(run(".github/workflows/ci.yml", OLD, &new).is_empty());
        // Numbers and shas aren't text someone writes.
        let safe = OLD.replace("make test", "echo ${{ github.event.pull_request.number }}");
        assert!(run(".github/workflows/ci.yml", OLD, &safe).is_empty());
    }

    #[test]
    fn terraform_opened_up() {
        let new = "resource \"aws_security_group\" \"web\" {\n  ingress {\n    from_port = 22\n    cidr_blocks = [\"0.0.0.0/0\"]\n  }\n  egress {\n    cidr_blocks = [\"0.0.0.0/0\"]\n  }\n}\nresource \"aws_s3_bucket_acl\" \"b\" {\n  acl = \"public-read\"\n}\n";
        let f = run("infra/main.tf", "", new);
        let got: Vec<&str> = f.iter().map(|x| x.subject.as_str()).collect();
        assert_eq!(got, ["0.0.0.0/0", "acl"], "{f:#?}");
        assert_eq!(f[0].lines.unwrap().start, 4);
        let iam = run(
            "policy.json",
            "",
            "{ \"Effect\": \"Allow\", \"Action\": \"*\" }\n",
        );
        assert_eq!(iam[0].subject, "iam");
    }
}
