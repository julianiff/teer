//! SARIF 2.1.0 in and out: other tools' findings (CodeQL, Semgrep, Claude
//! Security…) pinned to the units they fall in as facts, and teer's own
//! facts and findings written out.
//!
//! **In.** Each result of each run becomes an [`Imported`]: the tool, the
//! rule, the file and lines, a [`Weight`] and a [`Class`]. GitHub
//! code-scanning alerts become the same ([`from_alerts`]). [`pin`] puts each
//! on the unit whose new lines hold its first line (the smallest such
//! unit), or on its file's whole-file unit, as a [`Fact`] with
//! [`Origin::Import`]. Only the change is reviewed: a result outside every
//! changed unit is dropped, counted and logged. Imports only add facts:
//! nothing they say lowers a level.
//!
//! *Weight*: a `security-severity` of 7.0 or more, or level `error`, needs
//! attention; 4.0 or more, or level `warning` (SARIF's default), is worth
//! a look; anything else is a note.
//!
//! *Class*: a dependency scanner's results (Dependabot, OSV, Trivy, Grype,
//! Snyk…, or a rule named for an advisory: `CVE-…`, `GHSA-…`) are supply
//! chain. Otherwise the first CWE in the tags that teer knows decides
//! ([`class_of_cwe`]). Without one, the fallback reads the rule's id,
//! name and tags: injection, xss, sql, path, command, traversal… make it
//! injection; secret, password, credential, hardcoded… (or a secret
//! scanner: gitleaks, trufflehog) a secret; anything else is data exposure.
//!
//! **Out.** [`write`]: one rule per class (`teer/<class id>`), a result for
//! each of teer's own facts (imported ones go back to their tools, not
//! out again) and for each standing finding, with its path from the source
//! to the sink as a code flow.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use super::model::{Audit, Class, Fact, Finding, Origin, Proof, Weight};
use crate::review::model::{Cite, DiveKind, Range, Review, Risk, Side, Unit, UnitKind};
use crate::review::validate::one_line;

/// Longest fact text kept from another tool's message, in columns.
const MAX_TEXT: usize = 160;

/// Most results read from one file: a scan of a whole repository can have
/// thousands, and only the ones in the change are kept anyway.
const MAX_RESULTS: usize = 20_000;

/// Another tool's finding, read from SARIF or GitHub code scanning, not
/// yet pinned to a unit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Imported {
    /// The tool's name: `CodeQL`, `Semgrep OSS`.
    pub tool: String,
    /// The rule that fired: `js/sql-injection`.
    pub rule: String,
    /// Relative to the repository root.
    pub file: PathBuf,
    /// The lines on the new side, when the result names them.
    pub lines: Option<Range>,
    pub weight: Weight,
    pub class: Class,
    /// The tool's message: one line, sanitized.
    pub text: String,
}

impl Imported {
    /// The fact it becomes on its unit.
    pub fn fact(&self) -> Fact {
        Fact {
            class: self.class,
            weight: self.weight,
            text: self.text.clone(),
            side: Side::New,
            lines: self.lines,
            subject: self.rule.clone(),
            origin: Origin::Import {
                tool: self.tool.clone(),
                rule: self.rule.clone(),
            },
        }
    }
}

// ------------------------------------------------------------------- in

/// Read a SARIF file: JSON with a `runs` array. The error says why not.
pub fn load(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("not JSON: {e}"))?;
    if !v["runs"].is_array() {
        return Err("not SARIF: no runs".into());
    }
    Ok(v)
}

/// Read every result of every run of `json` (SARIF 2.1.0) whose file is in
/// the repository at `repo_root`. Identical results come once.
pub fn read(json: &Value, repo_root: &Path) -> Vec<Imported> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut n = 0;
    for run in json["runs"].as_array().into_iter().flatten() {
        let tool = one_line(run["tool"]["driver"]["name"].as_str().unwrap_or(""), 60);
        let tool = if tool.is_empty() {
            "SARIF".to_string()
        } else {
            tool
        };
        for r in run["results"].as_array().into_iter().flatten() {
            n += 1;
            if n > MAX_RESULTS {
                crate::log::write(&format!("SARIF: only the first {MAX_RESULTS} results read"));
                return out;
            }
            if let Some(i) = result(run, r, &tool, repo_root)
                && seen.insert(i.clone())
            {
                out.push(i);
            }
        }
    }
    out
}

/// One result, or `None` when it is no finding (passed, suppressed, gone)
/// or not in a file of the repository.
fn result(run: &Value, r: &Value, tool: &str, repo_root: &Path) -> Option<Imported> {
    if matches!(r["kind"].as_str(), Some("pass" | "notApplicable")) {
        return None;
    }
    if r["baselineState"].as_str() == Some("absent") {
        return None;
    }
    // Suppressed in the source or by the tool: only a rejected suppression
    // leaves it standing.
    let suppressed = r["suppressions"]
        .as_array()
        .is_some_and(|s| s.iter().any(|x| x["status"].as_str() != Some("rejected")));
    if suppressed {
        return None;
    }
    let rule = rule_of(run, r);
    let rule_id = r["ruleId"]
        .as_str()
        .or_else(|| r["rule"]["id"].as_str())
        .or_else(|| rule["id"].as_str())
        .unwrap_or("");
    let rule_id = one_line(rule_id, 120);
    let (file, lines) = location(run, r, repo_root)?;
    let severity = severity(&r["properties"]).or_else(|| severity(&rule["properties"]));
    let level = r["level"]
        .as_str()
        .or_else(|| rule["defaultConfiguration"]["level"].as_str())
        .unwrap_or("warning");
    let mut tags = strings(&rule["properties"]["tags"]);
    tags.extend(strings(&r["properties"]["tags"]));
    let name = rule["name"].as_str().unwrap_or("");
    let short = rule["shortDescription"]["text"].as_str().unwrap_or("");
    let text = message(&r["message"])
        .filter(|t| !t.is_empty())
        .or_else(|| (!short.is_empty()).then(|| short.to_string()))
        .unwrap_or_else(|| rule_id.clone());
    Some(Imported {
        tool: tool.to_string(),
        class: classify(tool, &rule_id, &format!("{name} {short}"), &tags),
        rule: rule_id,
        file,
        lines,
        weight: weight(severity, Some(level)),
        text: one_line(&text, MAX_TEXT),
    })
}

/// The rule a result names: by index into the driver's rules (or an
/// extension's), else by id among all of them. `Null` when there is none.
fn rule_of<'a>(run: &'a Value, r: &Value) -> &'a Value {
    const NONE: &Value = &Value::Null;
    let tool = &run["tool"];
    let index = r["ruleIndex"]
        .as_u64()
        .or_else(|| r["rule"]["index"].as_u64());
    let rules = match r["rule"]["toolComponent"]["index"].as_u64() {
        Some(k) => &tool["extensions"][k as usize]["rules"],
        None => &tool["driver"]["rules"],
    };
    if let Some(i) = index
        && let Some(rule) = rules.get(i as usize)
    {
        return rule;
    }
    let Some(id) = r["ruleId"].as_str().or_else(|| r["rule"]["id"].as_str()) else {
        return NONE;
    };
    let all = std::iter::once(&tool["driver"]["rules"]).chain(
        tool["extensions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| &e["rules"]),
    );
    all.flat_map(|rs| rs.as_array().into_iter().flatten())
        .find(|rule| rule["id"].as_str() == Some(id))
        .unwrap_or(NONE)
}

/// A `security-severity` property: a number, or a number in a string.
fn severity(props: &Value) -> Option<f64> {
    let v = &props["security-severity"];
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(String::from))
        .collect()
}

/// A result's message: its text (else its Markdown), placeholders
/// (`{0}`) filled from its arguments, links (`[a value](1)`) made plain.
fn message(m: &Value) -> Option<String> {
    let raw = m["text"].as_str().or_else(|| m["markdown"].as_str())?;
    let mut text = raw.to_string();
    for (i, a) in strings(&m["arguments"]).iter().enumerate() {
        text = text.replace(&format!("{{{i}}}"), a);
    }
    Some(plain_links(&text))
}

/// `[label](target)` becomes `label`.
fn plain_links(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let link = after.find("](").and_then(|close| {
            let target = &after[close + 2..];
            target.find(')').map(|end| (close, close + 2 + end + 1))
        });
        match link {
            Some((close, next)) if !after[..close].contains('[') => {
                out.push_str(&after[..close]);
                rest = &after[next..];
            }
            _ => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Where a result is: its first location's file, relative to the
/// repository root, and its lines.
fn location(run: &Value, r: &Value, repo_root: &Path) -> Option<(PathBuf, Option<Range>)> {
    let phys = &r["locations"][0]["physicalLocation"];
    let mut art = &phys["artifactLocation"];
    if art["uri"].as_str().is_none()
        && let Some(i) = art["index"].as_u64()
    {
        art = &run["artifacts"][i as usize]["location"];
    }
    let uri = art["uri"].as_str()?;
    let file = repo_path(uri, art["uriBaseId"].as_str(), run, repo_root)?;
    let region = &phys["region"];
    let lines = region["startLine"].as_u64().filter(|&s| s >= 1).map(|s| {
        let s = s.min(u32::MAX as u64) as u32;
        let e = region["endLine"]
            .as_u64()
            .map_or(s, |e| e.min(u32::MAX as u64) as u32)
            .max(s);
        Range::new(s, e)
    });
    Some((file, lines))
}

/// `uri` as a path relative to `repo_root`: a relative reference (to its
/// base, which is the repository root unless `originalUriBaseIds` puts the
/// base in a folder of it), or a `file://` path inside the repository.
/// `None` for anything else: another scheme, a path outside, `..` out.
fn repo_path(uri: &str, base_id: Option<&str>, run: &Value, repo_root: &Path) -> Option<PathBuf> {
    let uri = uri.split(['?', '#']).next().unwrap_or("");
    if let Some(abs) = file_uri(uri) {
        return inside(&abs, repo_root);
    }
    if uri.contains("://") || uri.starts_with('/') {
        return None;
    }
    let rel = plain(Path::new(&decode(uri)))?;
    let base = base_id
        .and_then(|b| run["originalUriBaseIds"][b]["uri"].as_str())
        .and_then(file_uri)
        .and_then(|dir| inside(&dir, repo_root));
    Some(match base {
        Some(dir) => dir.join(rel),
        None => rel,
    })
}

/// The absolute path of a `file:` URI (`file:///x`, `file://localhost/x`,
/// `file:/x`), percent-decoded.
fn file_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri
        .strip_prefix("file://")
        .or_else(|| uri.strip_prefix("file:"))?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    rest.starts_with('/').then(|| PathBuf::from(decode(rest)))
}

/// `abs` relative to `root` (as given, or with its links resolved).
fn inside(abs: &Path, root: &Path) -> Option<PathBuf> {
    let rel = abs
        .strip_prefix(root)
        .ok()
        .map(Path::to_path_buf)
        .or_else(|| {
            let canon = root.canonicalize().ok()?;
            abs.strip_prefix(&canon).ok().map(Path::to_path_buf)
        })?;
    if rel.as_os_str().is_empty() {
        return Some(rel);
    }
    plain(&rel)
}

/// `p` made of plain names (`./` dropped); `None` with `..` or a root.
fn plain(p: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Percent-decoding (`%20` → a space); a bad escape stays as written.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(x) = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(x);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// How much a result weighs: by its `security-severity` (0-10) and level.
pub fn weight(severity: Option<f64>, level: Option<&str>) -> Weight {
    let s = severity.unwrap_or(0.0);
    match level.unwrap_or("") {
        _ if s >= 7.0 => Weight::Attention,
        "error" => Weight::Attention,
        _ if s >= 4.0 => Weight::Look,
        "warning" => Weight::Look,
        _ => Weight::Note,
    }
}

/// The class of a CWE teer knows, by its number.
pub fn class_of_cwe(cwe: u32) -> Option<Class> {
    Some(match cwe {
        22 | 23 | 36 | 73 | 77 | 78 | 79 | 80 | 89 | 90 | 91 | 94 | 95 | 117 | 501 | 502 | 601
        | 611 | 643 | 776 | 918 | 943 | 1336 => Class::Injection,
        259 | 321 | 798 => Class::Secret,
        200 | 201 | 209 | 312 | 313 | 315 | 359 | 497 | 532 | 538 => Class::Exposure,
        284 | 285 | 287 | 306 | 639 | 862 | 863 => Class::Authorization,
        326 | 327 | 328 | 330 | 338 | 759 | 760 | 916 => Class::Crypto,
        16 | 295 | 614 | 942 | 1004 => Class::Config,
        _ => return None,
    })
}

/// The CWE numbers in `tag`: `external/cwe/cwe-089`, `CWE-79: Improper…`.
fn cwes(tag: &str) -> Vec<u32> {
    let lower = tag.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut rest = lower.as_str();
    while let Some(i) = rest.find("cwe-") {
        rest = &rest[i + 4..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse() {
            out.push(n);
        }
    }
    out
}

/// Tools that scan dependencies, and that scan for secrets, by a word of
/// their name.
const DEPENDENCY_TOOLS: &[&str] = &[
    "dependabot",
    "dependency",
    "osv",
    "trivy",
    "grype",
    "snyk",
    "npm audit",
    "yarn audit",
    "pip-audit",
    "safety",
    "govulncheck",
    "cargo-audit",
    "cargo audit",
    "cargo-deny",
    "retire",
    "socket",
    "mend",
    "whitesource",
];
const SECRET_TOOLS: &[&str] = &["gitleaks", "trufflehog", "detect-secrets", "secret"];
const ADVISORY_PREFIXES: &[&str] = &["cve-", "ghsa-", "rustsec-", "pysec-", "go-20", "osv-"];
const INJECTION_WORDS: &[&str] = &[
    "injection",
    "inject",
    "xss",
    "cross-site scripting",
    "sql",
    "path",
    "traversal",
    "command",
    "exec",
    "ssrf",
    "deserializ",
    "template",
    "redirect",
    "xxe",
    "eval",
];
const SECRET_WORDS: &[&str] = &[
    "secret",
    "password",
    "credential",
    "hardcoded",
    "hard-coded",
    "api-key",
    "api key",
    "apikey",
    "private-key",
    "private key",
];

/// The class of a result of `tool`'s rule `rule`, its `about` (the rule's
/// name and description) and its tags: see the module doc.
pub fn classify(tool: &str, rule: &str, about: &str, tags: &[String]) -> Class {
    let tool = tool.to_ascii_lowercase();
    let rule_l = rule.to_ascii_lowercase();
    let tags_l: Vec<String> = tags.iter().map(|t| t.to_ascii_lowercase()).collect();
    let dependency = DEPENDENCY_TOOLS.iter().any(|w| tool.contains(w))
        || ADVISORY_PREFIXES.iter().any(|p| rule_l.starts_with(p))
        || tags_l
            .iter()
            .any(|t| t == "dependency" || t == "dependencies" || t == "supply-chain" || t == "sca");
    if dependency {
        return Class::SupplyChain;
    }
    if let Some(c) = tags.iter().flat_map(|t| cwes(t)).find_map(class_of_cwe) {
        return c;
    }
    if SECRET_TOOLS.iter().any(|w| tool.contains(w)) {
        return Class::Secret;
    }
    let words = format!(
        "{rule_l} {} {}",
        about.to_ascii_lowercase(),
        tags_l.join(" ")
    );
    if INJECTION_WORDS.iter().any(|w| words.contains(w)) {
        Class::Injection
    } else if SECRET_WORDS.iter().any(|w| words.contains(w)) {
        Class::Secret
    } else {
        Class::Exposure
    }
}

/// GitHub code-scanning alerts (`gh api …/code-scanning/alerts`), each as
/// an [`Imported`]: `rule.security_severity_level` (critical and high need
/// attention, medium is worth a look) else `rule.severity` as a SARIF
/// level, `rule.tags`, `most_recent_instance.location` and `tool.name`.
/// Paths are the repository's own; anything that isn't one is left out.
pub fn from_alerts(alerts: &Value) -> Vec<Imported> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for a in alerts.as_array().into_iter().flatten().take(MAX_RESULTS) {
        if a["state"].as_str().is_some_and(|s| s != "open") {
            continue;
        }
        let rule = &a["rule"];
        let inst = &a["most_recent_instance"];
        let loc = &inst["location"];
        let Some(file) = loc["path"]
            .as_str()
            .filter(|p| !p.starts_with('/'))
            .and_then(|p| plain(Path::new(p)))
        else {
            continue;
        };
        let lines = loc["start_line"].as_u64().filter(|&s| s >= 1).map(|s| {
            let s = s.min(u32::MAX as u64) as u32;
            let e = loc["end_line"]
                .as_u64()
                .map_or(s, |e| e.min(u32::MAX as u64) as u32);
            Range::new(s, e.max(s))
        });
        let weight = match rule["security_severity_level"].as_str() {
            Some("critical" | "high") => Weight::Attention,
            Some("medium") => Weight::Look,
            Some(_) => Weight::Note,
            None => weight(None, rule["severity"].as_str()),
        };
        let tool = one_line(a["tool"]["name"].as_str().unwrap_or("code scanning"), 60);
        let rule_id = one_line(
            rule["id"]
                .as_str()
                .or_else(|| rule["name"].as_str())
                .unwrap_or(""),
            120,
        );
        let about = rule["description"].as_str().unwrap_or("");
        let text = inst["message"]["text"]
            .as_str()
            .map(plain_links)
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| about.to_string());
        let i = Imported {
            class: classify(&tool, &rule_id, about, &strings(&rule["tags"])),
            tool,
            rule: rule_id,
            file,
            lines,
            weight,
            text: one_line(&text, MAX_TEXT),
        };
        if seen.insert(i.clone()) {
            out.push(i);
        }
    }
    out
}

/// Pin each import to its unit as a fact (see the module doc). How many
/// fell outside every changed unit, and were dropped.
pub fn pin(review: &mut Review, imported: Vec<Imported>) -> usize {
    let mut outside = Vec::new();
    for imp in imported {
        let Some(i) = home(review, &imp) else {
            outside.push(imp);
            continue;
        };
        let fact = imp.fact();
        let u = &mut review.units[i];
        if !u.security.contains(&fact) {
            u.security.push(fact);
        }
    }
    if !outside.is_empty() {
        let some: Vec<String> = outside
            .iter()
            .take(5)
            .map(|i| {
                let at = i.lines.map_or(String::new(), |r| format!(":{}", r.start));
                format!("{} {}{at}", i.rule, i.file.display())
            })
            .collect();
        crate::log::write(&format!(
            "imported findings outside the change, left out: {} ({}{})",
            outside.len(),
            some.join(", "),
            if outside.len() > 5 { ", …" } else { "" }
        ));
    }
    outside.len()
}

/// The unit an import goes on: in its file, the smallest unit whose new
/// lines hold its first line, else one its lines overlap, else the file's
/// whole-file unit. Without lines: the whole-file unit, else the file's
/// first unit.
fn home(review: &Review, imp: &Imported) -> Option<usize> {
    let in_file: Vec<(usize, &Unit, Range)> = review
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.file == imp.file)
        .filter_map(|(i, u)| u.new_range.map(|r| (i, u, r)))
        .collect();
    let whole = || {
        in_file
            .iter()
            .find(|(_, u, _)| u.kind == UnitKind::File)
            .map(|(i, _, _)| *i)
    };
    let Some(lines) = imp.lines else {
        return whole().or_else(|| in_file.first().map(|(i, _, _)| *i));
    };
    in_file
        .iter()
        .filter(|(_, _, r)| r.contains(lines.start))
        .min_by_key(|(_, _, r)| r.len())
        .or_else(|| {
            in_file
                .iter()
                .find(|(_, _, r)| r.start <= lines.end && lines.start <= r.end)
        })
        .map(|(i, _, _)| *i)
        .or_else(whole)
}

/// Read the SARIF files at `paths` and pin what they found to the units of
/// `review` ([`pin`]). A file that can't be read is logged and skipped:
/// teer checks them when it starts, and a re-run goes on without one that
/// went since. How many results fell outside the change.
pub fn import_files(review: &mut Review, paths: &[PathBuf]) -> usize {
    let mut all = Vec::new();
    for p in paths {
        match load(p) {
            Ok(v) => all.extend(read(&v, &review.repo_root)),
            Err(e) => crate::log::write(&format!("SARIF {}: {e}", p.display())),
        }
    }
    pin(review, all)
}

// ------------------------------------------------------------------ out

const SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";

/// What each class's rule is about, in one sentence.
fn about(c: Class) -> &'static str {
    match c {
        Class::Authorization => {
            "An authorization or authentication check removed, weakened, or missing where its siblings have it."
        }
        Class::EntryPoint => {
            "A route, controller action, command, consumer or webhook that is new or has a new signature."
        }
        Class::Injection => {
            "Untrusted input reaching a sink: SQL, shell, eval, deserialization, file paths, outgoing HTTP, templates, redirects."
        }
        Class::Secret => "A credential in the code.",
        Class::Exposure => "Sensitive data leaving the process: logs, responses, events.",
        Class::SupplyChain => {
            "A package added, bumped or locked differently; an install script; a git or URL source."
        }
        Class::Ci => {
            "A CI workflow or infrastructure as code that widens what a change or the internet can do."
        }
        Class::Config => {
            "Security configuration loosened: CORS, cookies, TLS verification, CSP, debug."
        }
        Class::Crypto => "Weak randomness, hashing or comparison where a secret is involved.",
        Class::Llm => {
            "Model output reaching a sink, tools handed to an agent, untrusted text in a prompt."
        }
    }
}

/// A class's rule name: `SupplyChain`.
fn rule_name(c: Class) -> String {
    c.id()
        .split('_')
        .map(|w| {
            let mut cs = w.chars();
            cs.next()
                .map(|f| f.to_ascii_uppercase().to_string() + cs.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn rule_id(c: Class) -> String {
    format!("teer/{}", c.id())
}

fn rule_index(c: Class) -> usize {
    Class::ALL.iter().position(|&x| x == c).unwrap_or(0)
}

fn level_of_weight(w: Weight) -> &'static str {
    match w {
        Weight::Attention => "error",
        Weight::Look => "warning",
        Weight::Note => "note",
    }
}

fn level_of_risk(r: Risk) -> &'static str {
    match r {
        Risk::High => "error",
        Risk::Medium => "warning",
        Risk::Low => "note",
    }
}

/// `path` as a relative URI reference: `/` between names, anything but
/// unreserved characters percent-encoded.
fn uri(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A physical location: the file, and its lines when there are some.
fn physical(file: &Path, lines: Option<Range>) -> Value {
    let mut loc = json!({
        "artifactLocation": { "uri": uri(file), "uriBaseId": "%SRCROOT%" }
    });
    if let Some(r) = lines.filter(|r| r.start >= 1) {
        loc["region"] = json!({ "startLine": r.start, "endLine": r.end.max(r.start) });
    }
    loc
}

fn cite_location(c: &Cite) -> Value {
    physical(&c.file, Some(Range::new(c.start, c.end)))
}

/// Teer's fact on `u` as a result. A fact on the old side (a check that is
/// gone) points at the unit's new lines, and names the old ones.
fn fact_result(u: &Unit, f: &Fact) -> Value {
    let (lines, old) = match f.side {
        Side::New => (f.lines.or(u.new_range), None),
        Side::Old => (u.new_range, f.lines.or(u.old_range)),
    };
    let mut props = json!({
        "unit": u.symbol,
        "side": match f.side { Side::Old => "old", Side::New => "new" },
    });
    if !f.subject.is_empty() {
        props["subject"] = json!(f.subject);
    }
    if let Some(r) = old {
        props["oldLines"] = json!(format!("{}-{}", r.start, r.end));
    }
    json!({
        "ruleId": rule_id(f.class),
        "ruleIndex": rule_index(f.class),
        "level": level_of_weight(f.weight),
        "message": { "text": f.text },
        "locations": [{
            "physicalLocation": physical(&u.file, lines),
            "logicalLocations": [{ "fullyQualifiedName": u.symbol }],
        }],
        "partialFingerprints": {
            "teerFact/v1": crate::util::hash_hex(&[
                f.class.id().as_bytes(),
                u.file.to_string_lossy().as_bytes(),
                u.symbol.as_bytes(),
                f.text.as_bytes(),
            ]),
        },
        "properties": props,
    })
}

fn flow_step(c: &Cite, what: &str) -> Value {
    json!({
        "location": {
            "physicalLocation": cite_location(c),
            "message": { "text": what },
        }
    })
}

/// A standing finding of `u` as a result: at its sink, with the path from
/// the source to the sink as its code flow.
fn finding_result(u: &Unit, f: &Finding) -> Value {
    let mut steps = vec![flow_step(&f.source, "source")];
    steps.extend(f.path.iter().map(|h| flow_step(&h.cite, &h.symbol)));
    steps.push(flow_step(&f.sink, "sink"));
    let sentence = |s: &str| {
        let s = s.trim().trim_end_matches('.');
        if s.is_empty() {
            String::new()
        } else {
            format!("{s}.")
        }
    };
    let text = [
        sentence(&f.title),
        match sentence(&f.missing) {
            m if m.is_empty() => m,
            m => format!("Missing: {m}"),
        },
        sentence(&f.exploit),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    let mut props = json!({
        "unit": u.symbol,
        "severity": f.severity.word(),
        "confidence": f.confidence.word(),
        "pathVerified": f.path_verified,
    });
    if let Some(s) = &f.second {
        props["secondModel"] = json!(if s.agree { "agrees" } else { "disputes" });
    }
    if let Some(p) = &f.proof {
        props["proof"] = json!(match p {
            Proof::Proven { .. } => "proven",
            Proof::NotShown { .. } => "not shown",
            Proof::Unproven { .. } => "unproven",
        });
    }
    json!({
        "ruleId": rule_id(f.class),
        "ruleIndex": rule_index(f.class),
        "level": level_of_risk(f.severity),
        "message": { "text": text },
        "locations": [{
            "physicalLocation": cite_location(&f.sink),
            "logicalLocations": [{ "fullyQualifiedName": u.symbol }],
        }],
        "codeFlows": [{ "threadFlows": [{ "locations": steps }] }],
        "partialFingerprints": {
            "teerFinding/v1": crate::util::hash_hex(&[
                f.class.id().as_bytes(),
                u.file.to_string_lossy().as_bytes(),
                u.symbol.as_bytes(),
                f.title.as_bytes(),
            ]),
        },
        "properties": props,
    })
}

fn audit(u: &Unit) -> Option<&Audit> {
    u.dive
        .results
        .get(&DiveKind::Security)
        .and_then(|r| r.audit.as_ref())
}

/// The review's security facts (teer's own) and standing findings as SARIF
/// 2.1.0, from the tool `teer`, one rule per class.
pub fn write(review: &Review) -> Value {
    let rules: Vec<Value> = Class::ALL
        .iter()
        .map(|&c| {
            json!({
                "id": rule_id(c),
                "name": rule_name(c),
                "shortDescription": { "text": format!("Security: {}", c.word()) },
                "fullDescription": { "text": about(c) },
                "defaultConfiguration": { "level": "warning" },
                "properties": { "tags": ["security", c.id()] },
            })
        })
        .collect();
    let mut results = Vec::new();
    for u in &review.units {
        for f in u.security.iter().filter(|f| f.origin.is_teer()) {
            results.push(fact_result(u, f));
        }
        for f in audit(u)
            .iter()
            .flat_map(|a| &a.findings)
            .filter(|f| f.stands())
        {
            results.push(finding_result(u, f));
        }
    }
    json!({
        "$schema": SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "teer",
                    "version": env!("CARGO_PKG_VERSION"),
                    "rules": rules,
                }
            },
            "columnKind": "unicodeCodePoints",
            "results": results,
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_follow_severity_then_level() {
        assert_eq!(weight(Some(9.8), Some("note")), Weight::Attention);
        assert_eq!(weight(Some(7.0), None), Weight::Attention);
        assert_eq!(weight(None, Some("error")), Weight::Attention);
        assert_eq!(weight(Some(5.0), Some("note")), Weight::Look);
        assert_eq!(weight(None, Some("warning")), Weight::Look);
        assert_eq!(weight(Some(2.0), Some("note")), Weight::Note);
        assert_eq!(weight(None, Some("none")), Weight::Note);
    }

    #[test]
    fn classes_come_from_cwes_then_words() {
        let t = |tags: &[&str]| tags.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let c = |tool: &str, rule: &str, tags: &[&str]| classify(tool, rule, "", &t(tags));
        assert_eq!(
            c("CodeQL", "js/x", &["security", "external/cwe/cwe-089"]),
            Class::Injection
        );
        assert_eq!(
            c("CodeQL", "x", &["external/cwe/cwe-022"]),
            Class::Injection
        );
        assert_eq!(c("Semgrep", "x", &["CWE-798: Hard-coded"]), Class::Secret);
        assert_eq!(c("x", "x", &["cwe-532"]), Class::Exposure);
        assert_eq!(c("x", "x", &["cwe-639"]), Class::Authorization);
        assert_eq!(c("x", "x", &["cwe-327"]), Class::Crypto);
        assert_eq!(c("x", "x", &["cwe-1004"]), Class::Config);
        assert_eq!(c("Trivy", "x", &["cwe-79"]), Class::SupplyChain);
        assert_eq!(c("x", "GHSA-1234", &[]), Class::SupplyChain);
        assert_eq!(c("x", "python.flask.xss-reflected", &[]), Class::Injection);
        assert_eq!(
            c("x", "generic.secrets.password-in-code", &[]),
            Class::Secret
        );
        assert_eq!(c("gitleaks", "aws-key", &[]), Class::Secret);
        assert_eq!(c("x", "debug-logging", &[]), Class::Exposure);
        assert_eq!(cwes("CWE-89 and cwe-0079"), [89, 79]);
    }

    #[test]
    fn uris_resolve_into_the_repository() {
        let root = Path::new("/work/repo");
        let run = json!({"originalUriBaseIds": {
            "SRC": {"uri": "file:///work/repo/src/"},
            "ELSE": {"uri": "file:///elsewhere/"}
        }});
        let p = |uri: &str, base: Option<&str>| repo_path(uri, base, &run, root);
        assert_eq!(p("a/b.py", None), Some("a/b.py".into()));
        assert_eq!(p("./a/b%20c.py", None), Some("a/b c.py".into()));
        assert_eq!(p("b.py", Some("SRC")), Some("src/b.py".into()));
        assert_eq!(p("a/b.py", Some("ELSE")), Some("a/b.py".into()));
        assert_eq!(p("a/b.py", Some("%SRCROOT%")), Some("a/b.py".into()));
        assert_eq!(p("file:///work/repo/a/b.py", None), Some("a/b.py".into()));
        assert_eq!(p("file:///other/a.py", None), None);
        assert_eq!(p("../a.py", None), None);
        assert_eq!(p("/etc/passwd", None), None);
        assert_eq!(p("https://x/a.py", None), None);
    }

    #[test]
    fn messages_lose_their_links_and_get_their_arguments() {
        let m = json!({"text": "This depends on a [user-provided value](1) in {0}.", "arguments": ["q"]});
        assert_eq!(
            message(&m).unwrap(),
            "This depends on a user-provided value in q."
        );
        assert_eq!(plain_links("a [b] c [d](e"), "a [b] c [d](e");
    }

    #[test]
    fn rule_names_are_identifiers() {
        assert_eq!(rule_name(Class::SupplyChain), "SupplyChain");
        assert_eq!(rule_name(Class::Ci), "Ci");
        assert_eq!(uri(Path::new("a dir/b.py")), "a%20dir/b.py");
    }
}
