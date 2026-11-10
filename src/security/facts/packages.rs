//! Supply chain, from manifests and locks: packages added or bumped, from
//! a git or URL source, a lock changed without its manifest, install
//! scripts.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::{fact, fact_at};
use crate::git::ChangedFile;
use crate::review::model::Review;
use crate::security::model::{Class, Fact, Weight};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Npm,
    Composer,
    GoMod,
    Cargo,
    Pyproject,
    Requirements,
}

fn manifest_kind(name: &str) -> Option<Kind> {
    Some(match name {
        "package.json" => Kind::Npm,
        "composer.json" => Kind::Composer,
        "go.mod" => Kind::GoMod,
        "Cargo.toml" => Kind::Cargo,
        "pyproject.toml" => Kind::Pyproject,
        n if n.starts_with("requirements") && n.ends_with(".txt") => Kind::Requirements,
        _ => return None,
    })
}

/// The manifests a lock goes with.
fn manifests_of(lock: &str) -> Option<&'static [&'static str]> {
    Some(match lock {
        "package-lock.json" | "npm-shrinkwrap.json" | "yarn.lock" | "pnpm-lock.yaml" => {
            &["package.json"]
        }
        "composer.lock" => &["composer.json"],
        "go.sum" => &["go.mod"],
        "Cargo.lock" => &["Cargo.toml"],
        "poetry.lock" | "uv.lock" => &["pyproject.toml"],
        "Pipfile.lock" => &["Pipfile"],
        "Gemfile.lock" => &["Gemfile"],
        _ => return None,
    })
}

/// Is this file one [`facts`] reads?
pub(crate) fn applies(path: &Path) -> bool {
    let name = file_name(path);
    manifest_kind(&name).is_some() || manifests_of(&name).is_some()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[derive(Debug, Default)]
struct Manifest {
    /// Package name → version spec (or source: `git+…`, `path:…`).
    deps: BTreeMap<String, String>,
    /// Scripts that run on install, by name.
    scripts: BTreeMap<String, String>,
    /// Go's `replace`: module → what replaces it.
    replaces: BTreeMap<String, String>,
    /// Composer repositories: url → type.
    repos: BTreeMap<String, String>,
}

const NPM_INSTALL: [&str; 6] = [
    "preinstall",
    "install",
    "postinstall",
    "prepare",
    "preprepare",
    "postprepare",
];
const COMPOSER_INSTALL: [&str; 5] = [
    "pre-install-cmd",
    "post-install-cmd",
    "pre-update-cmd",
    "post-update-cmd",
    "post-autoload-dump",
];

fn parse(kind: Kind, text: &str) -> Manifest {
    let mut m = Manifest::default();
    match kind {
        Kind::Npm | Kind::Composer => {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
                return m;
            };
            let sections: &[&str] = if kind == Kind::Npm {
                &[
                    "dependencies",
                    "devDependencies",
                    "optionalDependencies",
                    "peerDependencies",
                ]
            } else {
                &["require", "require-dev"]
            };
            for s in sections {
                for (k, val) in v.get(*s).and_then(|x| x.as_object()).into_iter().flatten() {
                    let platform = kind == Kind::Composer
                        && (k == "php"
                            || k.starts_with("ext-")
                            || k.starts_with("lib-")
                            || k == "composer-plugin-api");
                    if let Some(spec) = val.as_str()
                        && !platform
                    {
                        m.deps.insert(k.clone(), spec.to_string());
                    }
                }
            }
            let install: &[&str] = if kind == Kind::Npm {
                &NPM_INSTALL
            } else {
                &COMPOSER_INSTALL
            };
            for (k, val) in v
                .get("scripts")
                .and_then(|x| x.as_object())
                .into_iter()
                .flatten()
            {
                if !install.contains(&k.as_str()) {
                    continue;
                }
                let cmd = match val {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Array(a) => a
                        .iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join("; "),
                    _ => continue,
                };
                m.scripts.insert(k.clone(), cmd);
            }
            for r in v
                .get("repositories")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
            {
                if let Some(url) = r.get("url").and_then(|u| u.as_str()) {
                    let t = r.get("type").and_then(|t| t.as_str()).unwrap_or("vcs");
                    m.repos.insert(url.to_string(), t.to_string());
                }
            }
        }
        Kind::GoMod => {
            let mut block = "";
            for raw in text.lines() {
                let line = raw.split("//").next().unwrap_or("").trim();
                if line.is_empty() {
                    continue;
                }
                if line == ")" {
                    block = "";
                    continue;
                }
                let (verb, rest) = match line.split_once(char::is_whitespace) {
                    Some((v, r)) if matches!(v, "require" | "replace") => (v, r.trim()),
                    _ => (block, line),
                };
                if rest == "(" {
                    block = verb;
                    continue;
                }
                match verb {
                    "require" => {
                        let mut it = rest.split_whitespace();
                        if let (Some(module), Some(version)) = (it.next(), it.next()) {
                            m.deps.insert(module.to_string(), version.to_string());
                        }
                    }
                    "replace" => {
                        if let Some((from, to)) = rest.split_once("=>") {
                            let from = from.split_whitespace().next().unwrap_or("");
                            m.replaces.insert(from.to_string(), to.trim().to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        Kind::Cargo | Kind::Pyproject => {
            let Ok(v) = toml::from_str::<toml::Table>(text) else {
                return m;
            };
            let mut tables: Vec<&toml::Table> = Vec::new();
            fn push<'v>(tables: &mut Vec<&'v toml::Table>, t: Option<&'v toml::Value>) {
                if let Some(t) = t.and_then(|t| t.as_table()) {
                    tables.push(t);
                }
            }
            let mut take = |t| push(&mut tables, t);
            if kind == Kind::Cargo {
                for s in ["dependencies", "dev-dependencies", "build-dependencies"] {
                    take(v.get(s));
                }
                take(v.get("workspace").and_then(|w| w.get("dependencies")));
                for (_, t) in v
                    .get("target")
                    .and_then(|t| t.as_table())
                    .into_iter()
                    .flatten()
                {
                    for s in ["dependencies", "dev-dependencies", "build-dependencies"] {
                        take(t.get(s));
                    }
                }
            } else {
                let poetry = v.get("tool").and_then(|t| t.get("poetry"));
                take(poetry.and_then(|p| p.get("dependencies")));
                take(poetry.and_then(|p| p.get("dev-dependencies")));
                for (_, g) in poetry
                    .and_then(|p| p.get("group"))
                    .and_then(|g| g.as_table())
                    .into_iter()
                    .flatten()
                {
                    take(g.get("dependencies"));
                }
                let project = v.get("project");
                let mut lists: Vec<&toml::Value> = Vec::new();
                lists.extend(project.and_then(|p| p.get("dependencies")));
                for (_, l) in project
                    .and_then(|p| p.get("optional-dependencies"))
                    .and_then(|o| o.as_table())
                    .into_iter()
                    .flatten()
                {
                    lists.push(l);
                }
                for l in lists.into_iter().filter_map(|l| l.as_array()).flatten() {
                    if let Some((name, spec)) = l.as_str().and_then(pep508) {
                        m.deps.insert(name, spec);
                    }
                }
            }
            for t in tables {
                for (k, val) in t {
                    if k == "python" {
                        continue;
                    }
                    let spec = match val {
                        toml::Value::String(s) => s.clone(),
                        toml::Value::Table(d) => {
                            if let Some(g) = d.get("git").and_then(|g| g.as_str()) {
                                format!("git+{g}")
                            } else if let Some(p) = d.get("path").and_then(|p| p.as_str()) {
                                format!("path:{p}")
                            } else if let Some(u) = d.get("url").and_then(|u| u.as_str()) {
                                u.to_string()
                            } else {
                                d.get("version")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("*")
                                    .to_string()
                            }
                        }
                        _ => continue,
                    };
                    m.deps.insert(k.clone(), spec);
                }
            }
        }
        Kind::Requirements => {
            for raw in text.lines() {
                let line = raw.split(" #").next().unwrap_or("").trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let url = line.strip_prefix("-e ").unwrap_or(line).trim();
                if url.starts_with("git+") || url.starts_with("http") {
                    let name = url
                        .split("#egg=")
                        .nth(1)
                        .map(String::from)
                        .unwrap_or_else(|| {
                            url.trim_end_matches(".git")
                                .rsplit('/')
                                .next()
                                .unwrap_or(url)
                                .to_string()
                        });
                    m.deps.insert(name.to_ascii_lowercase(), url.to_string());
                    continue;
                }
                if line.starts_with('-') {
                    continue;
                }
                if let Some((name, spec)) = pep508(line) {
                    m.deps.insert(name, spec);
                }
            }
        }
    }
    m
}

static PEP508: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([A-Za-z0-9][A-Za-z0-9._-]*)(\[[^\]]*\])?\s*(.*)$").unwrap());

/// `requests[socks]>=2.0 ; python_version<'3.8'` → (`requests`, `>=2.0`).
fn pep508(s: &str) -> Option<(String, String)> {
    let c = PEP508.captures(s)?;
    let name = c[1].to_ascii_lowercase().replace('_', "-");
    let rest = c[3].split(';').next().unwrap_or("").trim();
    let spec = rest.strip_prefix('@').map(str::trim).unwrap_or(rest);
    Some((
        name,
        if spec.is_empty() {
            "*".into()
        } else {
            spec.into()
        },
    ))
}

static GITHUB_SHORTHAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(#\S+)?$").unwrap());

/// Where a spec takes a package from when not a registry: `git`, `URL`,
/// `local path`; with the scheme a fact may show.
fn source_of(spec: &str) -> Option<(&'static str, String)> {
    let s = spec.trim();
    let scheme = |n: usize| s.chars().take(n).collect::<String>();
    if s.starts_with("git+") || s.starts_with("git:") || s.starts_with("git@") {
        Some(("git", s.split("://").next().unwrap_or("git").to_string()))
    } else if let Some((p, _)) = ["github:", "gitlab:", "bitbucket:"]
        .iter()
        .find_map(|p| s.strip_prefix(*p).map(|r| (*p, r)))
    {
        Some(("git", p.to_string()))
    } else if s.starts_with("http://") || s.starts_with("https://") {
        Some((
            "URL",
            s.split("://").next().unwrap_or("").to_string() + "://",
        ))
    } else if s.starts_with("file:") || s.starts_with("link:") || s.starts_with("path:") {
        Some(("local path", scheme(s.find(':').unwrap_or(0) + 1)))
    } else if s.starts_with("../") || s.starts_with("./") || s.starts_with('/') {
        Some(("local path", "path".into()))
    } else if GITHUB_SHORTHAND.is_match(s) && !s.starts_with('@') {
        Some(("git", "GitHub shorthand".into()))
    } else {
        None
    }
}

/// The first two numbers of a version spec: `^1.2.3` → (1, 2).
fn version(spec: &str) -> Option<(u64, u64)> {
    let s = spec.trim_start_matches(|c: char| !c.is_ascii_digit());
    let mut parts = s.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

/// A major bump: `1.x` → `2.x`, or `0.2` → `0.3`.
fn major_bump(old: &str, new: &str) -> bool {
    match (version(old), version(new)) {
        (Some((a0, b0)), Some((a1, b1))) => a1 > a0 || (a0 == 0 && a1 == 0 && b1 > b0),
        _ => false,
    }
}

fn short(spec: &str) -> String {
    crate::util::truncate(spec.trim(), 30)
}

/// The line of the new file that names `name`.
fn line_of(text: &str, name: &str) -> Option<u32> {
    let quoted = [format!("\"{name}\""), format!("'{name}'")];
    text.lines()
        .position(|l| {
            let t = l.trim_start();
            quoted.iter().any(|q| l.contains(q.as_str()))
                || t.strip_prefix(name).is_some_and(|rest| {
                    rest.is_empty()
                        || rest.starts_with([' ', '=', '<', '>', '~', '!', '[', '@', '\t'])
                })
        })
        .map(|i| i as u32 + 1)
}

/// Supply-chain facts of a manifest's or a lock's whole-file unit.
pub(crate) fn facts(review: &Review, f: &ChangedFile) -> Vec<Fact> {
    let name = file_name(&f.path);
    let old = f.old_content.as_deref().unwrap_or("");
    let new = f.new_content.as_deref().unwrap_or("");
    if let Some(kind) = manifest_kind(&name) {
        return manifest_facts(kind, &name, old, new);
    }
    if let Some(manifests) = manifests_of(&name) {
        return lock_facts(review, f, &name, manifests, old, new);
    }
    Vec::new()
}

/// More than this many packages added (or bumped) in one manifest make
/// one fact, not one each.
const MANY: usize = 5;

fn names(list: &[&String]) -> String {
    let shown: Vec<&str> = list.iter().take(3).map(|s| s.as_str()).collect();
    match list.len() {
        n if n > 3 => format!("{} and {} more", shown.join(", "), n - 3),
        _ => shown.join(", "),
    }
}

fn manifest_facts(kind: Kind, file: &str, old: &str, new: &str) -> Vec<Fact> {
    let o = parse(kind, old);
    let n = parse(kind, new);
    let mut out = Vec::new();
    let at = |name: &str| line_of(new, name);

    // Install scripts.
    for (script, cmd) in &n.scripts {
        let before = o.scripts.get(script);
        if before == Some(cmd) {
            continue;
        }
        let verb = if before.is_some() { "changed" } else { "added" };
        let shown = crate::util::truncate(&super::lines::mask_secrets(cmd), 50);
        let weight = if kind == Kind::Npm {
            Weight::Attention
        } else {
            Weight::Look
        };
        out.push(fact_at(
            Class::SupplyChain,
            weight,
            format!("{script} script {verb}, it runs on install: {shown}"),
            at(script),
            script,
        ));
    }

    // Packages added or bumped, and from where.
    let mut added: Vec<&String> = Vec::new();
    let mut bumped: Vec<&String> = Vec::new();
    let mut majors: Vec<&String> = Vec::new();
    for (pkg, spec) in &n.deps {
        let before = o.deps.get(pkg);
        if before == Some(spec) {
            continue;
        }
        let from = source_of(spec);
        let was_from = before.and_then(|b| source_of(b));
        if let Some((what, scheme)) = &from
            && was_from.is_none()
        {
            let text = match before {
                None => format!("package {pkg} added from a {what} source ({scheme})"),
                Some(_) => format!("package {pkg} now comes from a {what} source ({scheme})"),
            };
            out.push(fact_at(
                Class::SupplyChain,
                Weight::Look,
                text,
                at(pkg),
                pkg,
            ));
            continue;
        }
        match before {
            None => added.push(pkg),
            Some(b) if major_bump(b, spec) => majors.push(pkg),
            Some(_) => bumped.push(pkg),
        }
    }
    let spec_of = |p: &String| short(&n.deps[p]);
    let was = |p: &String| short(&o.deps[p]);
    if added.len() > MANY {
        out.push(fact_at(
            Class::SupplyChain,
            Weight::Look,
            format!(
                "{} packages added to {file}: {}",
                added.len(),
                names(&added)
            ),
            at(added[0]),
            "",
        ));
    } else {
        for p in &added {
            out.push(fact_at(
                Class::SupplyChain,
                Weight::Look,
                format!("package {p} added ({})", spec_of(p)),
                at(p),
                p,
            ));
        }
    }
    if majors.len() > MANY {
        out.push(fact_at(
            Class::SupplyChain,
            Weight::Look,
            format!(
                "{} packages bumped a major version: {}",
                majors.len(),
                names(&majors)
            ),
            at(majors[0]),
            "",
        ));
    } else {
        for p in &majors {
            out.push(fact_at(
                Class::SupplyChain,
                Weight::Look,
                format!(
                    "package {p} bumped a major version, {} → {}",
                    was(p),
                    spec_of(p)
                ),
                at(p),
                p,
            ));
        }
    }
    if bumped.len() > MANY {
        out.push(fact_at(
            Class::SupplyChain,
            Weight::Note,
            format!("{} packages bumped: {}", bumped.len(), names(&bumped)),
            at(bumped[0]),
            "",
        ));
    } else {
        for p in &bumped {
            out.push(fact_at(
                Class::SupplyChain,
                Weight::Note,
                format!("package {p} bumped, {} → {}", was(p), spec_of(p)),
                at(p),
                p,
            ));
        }
    }

    // Go: a module replaced by a local copy or a fork.
    for (module, to) in &n.replaces {
        if o.replaces.get(module) == Some(to) {
            continue;
        }
        let target = to.split_whitespace().next().unwrap_or("");
        let what = if target.starts_with('.') || target.starts_with('/') {
            "a local path"
        } else {
            "another module"
        };
        out.push(fact_at(
            Class::SupplyChain,
            Weight::Look,
            format!("{module} replaced by {what} ({})", short(target)),
            line_of(new, "replace").or_else(|| at(module)),
            module,
        ));
    }

    // Composer: a repository to take packages from.
    for (url, t) in &n.repos {
        if !o.repos.contains_key(url) {
            out.push(fact_at(
                Class::SupplyChain,
                Weight::Look,
                format!("a {t} repository added to take packages from"),
                line_of(new, "repositories"),
                "repositories",
            ));
        }
    }
    out
}

/// Each package's version in a lock.
fn lock_packages(lock: &str, text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    match lock {
        "package-lock.json" | "npm-shrinkwrap.json" | "composer.lock" => {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
                return out;
            };
            if lock == "composer.lock" {
                for s in ["packages", "packages-dev"] {
                    for p in v.get(s).and_then(|x| x.as_array()).into_iter().flatten() {
                        if let (Some(n), Some(ver)) = (
                            p.get("name").and_then(|x| x.as_str()),
                            p.get("version").and_then(|x| x.as_str()),
                        ) {
                            out.insert(n.to_string(), ver.to_string());
                        }
                    }
                }
            } else if let Some(pkgs) = v.get("packages").and_then(|x| x.as_object()) {
                for (k, p) in pkgs {
                    let Some(name) = k.rsplit("node_modules/").next().filter(|n| !n.is_empty())
                    else {
                        continue;
                    };
                    if let Some(ver) = p.get("version").and_then(|x| x.as_str()) {
                        out.insert(name.to_string(), ver.to_string());
                    }
                }
            } else if let Some(deps) = v.get("dependencies").and_then(|x| x.as_object()) {
                for (k, p) in deps {
                    if let Some(ver) = p.get("version").and_then(|x| x.as_str()) {
                        out.insert(k.clone(), ver.to_string());
                    }
                }
            }
        }
        "Cargo.lock" | "poetry.lock" | "uv.lock" => {
            let Ok(v) = toml::from_str::<toml::Table>(text) else {
                return out;
            };
            for p in v
                .get("package")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
            {
                if let (Some(n), Some(ver)) = (
                    p.get("name").and_then(|x| x.as_str()),
                    p.get("version").and_then(|x| x.as_str()),
                ) {
                    out.insert(n.to_string(), ver.to_string());
                }
            }
        }
        "go.sum" => {
            for l in text.lines() {
                let mut it = l.split_whitespace();
                if let (Some(m), Some(ver)) = (it.next(), it.next())
                    && !ver.ends_with("/go.mod")
                {
                    out.insert(m.to_string(), ver.to_string());
                }
            }
        }
        "yarn.lock" => {
            let mut names: Vec<String> = Vec::new();
            for l in text.lines() {
                if l.starts_with('#') || l.trim().is_empty() {
                    continue;
                }
                if !l.starts_with(' ') && l.trim_end().ends_with(':') {
                    names = l
                        .trim_end()
                        .trim_end_matches(':')
                        .split(", ")
                        .map(|s| {
                            // `@scope/pkg@^1` → `@scope/pkg`: the `@` after
                            // the first character.
                            let s = s.trim_matches('"');
                            match s.char_indices().skip(1).find(|(_, c)| *c == '@') {
                                Some((i, _)) => s[..i].to_string(),
                                None => s.to_string(),
                            }
                        })
                        .collect();
                } else if let Some(v) = l
                    .trim()
                    .strip_prefix("version ")
                    .or_else(|| l.trim().strip_prefix("version:"))
                {
                    for n in &names {
                        out.insert(n.clone(), v.trim().trim_matches('"').to_string());
                    }
                }
            }
        }
        "pnpm-lock.yaml" => {
            static ENTRY: LazyLock<Regex> = LazyLock::new(|| {
                Regex::new(r#"^  ['"]?/?((?:@[^/@\s'"]+/)?[^@/\s'"(:]+)[@/](\d[^:('"\s]*)"#)
                    .unwrap()
            });
            for l in text.lines() {
                if let Some(c) = ENTRY.captures(l) {
                    out.insert(c[1].to_string(), c[2].to_string());
                }
            }
        }
        "Gemfile.lock" => {
            static GEM: LazyLock<Regex> =
                LazyLock::new(|| Regex::new(r"^    ([A-Za-z0-9_.-]+) \(([^)]+)\)$").unwrap());
            for l in text.lines() {
                if let Some(c) = GEM.captures(l) {
                    out.insert(c[1].to_string(), c[2].to_string());
                }
            }
        }
        _ => {}
    }
    out
}

/// The packages a lock adds or moves to another version: `left-pad 1.3.0
/// (new)`, `guzzlehttp/psr7 2.7.0 → 2.8.0`.
fn lock_changes(lock: &str, old: &str, new: &str) -> Vec<String> {
    let o = lock_packages(lock, old);
    lock_packages(lock, new)
        .iter()
        .filter_map(|(p, v)| match o.get(p) {
            None => Some(format!("{p} {v} (new)")),
            Some(was) if was != v => Some(format!("{p} {was} → {v}")),
            _ => None,
        })
        .collect()
}

fn lock_facts(
    review: &Review,
    f: &ChangedFile,
    lock: &str,
    manifests: &[&str],
    old: &str,
    new: &str,
) -> Vec<Fact> {
    let dir = f.path.parent();
    // A workspace's members each have a Cargo.toml; the lock is at the top.
    let anywhere = lock == "Cargo.lock";
    let with_manifest = review.files.iter().any(|g| {
        manifests.contains(&file_name(&g.path).as_str()) && (anywhere || g.path.parent() == dir)
    });
    if with_manifest {
        return Vec::new();
    }
    let changes = lock_changes(lock, old, new);
    // Hashes, metadata or a format teer can't read: no package to name.
    if changes.is_empty() {
        return Vec::new();
    }
    let manifest = manifests[0];
    let text = match changes.len() {
        1 | 2 => format!("{lock} changed without {manifest}: {}", changes.join(", ")),
        k => format!(
            "{lock} changed without {manifest}: {}, and {} more",
            changes[..2].join(", "),
            k - 2
        ),
    };
    vec![fact(Class::SupplyChain, Weight::Look, text, 0, lock)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(file: &str, old: &str, new: &str) -> Vec<Fact> {
        manifest_facts(manifest_kind(file).unwrap(), file, old, new)
    }

    #[test]
    fn npm_packages_and_install_scripts() {
        let old = r#"{"scripts": {"test": "jest"}, "dependencies": {"left-pad": "^1.0.0", "react": "^17.0.0", "lodash": "4.17.20"}}"#;
        let new = r#"{
  "scripts": {"test": "jest", "postinstall": "node setup.js"},
  "dependencies": {
    "left-pad": "^1.3.0",
    "react": "^18.2.0",
    "lodash": "4.17.21",
    "evil": "github:someone/evil",
    "axios": "^1.6.0"
  }
}"#;
        let f = m("package.json", old, new);
        let got: Vec<(&str, Weight, &str)> = f
            .iter()
            .map(|x| (x.subject.as_str(), x.weight, x.text.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "postinstall",
                    Weight::Attention,
                    "postinstall script added, it runs on install: node setup.js"
                ),
                (
                    "evil",
                    Weight::Look,
                    "package evil added from a git source (github:)"
                ),
                ("axios", Weight::Look, "package axios added (^1.6.0)"),
                (
                    "react",
                    Weight::Look,
                    "package react bumped a major version, ^17.0.0 → ^18.2.0"
                ),
                (
                    "left-pad",
                    Weight::Note,
                    "package left-pad bumped, ^1.0.0 → ^1.3.0"
                ),
                (
                    "lodash",
                    Weight::Note,
                    "package lodash bumped, 4.17.20 → 4.17.21"
                ),
            ]
        );
        assert_eq!(f[2].lines.unwrap().start, 8);
    }

    #[test]
    fn go_mod_and_requirements() {
        let old = "module x\n\ngo 1.22\n\nrequire (\n\tgithub.com/a/b v1.2.0\n)\n";
        let new = "module x\n\ngo 1.22\n\nrequire (\n\tgithub.com/a/b v1.3.0\n\tgithub.com/c/d v0.1.0 // indirect\n)\n\nreplace github.com/a/b => ../b\n";
        let f = m("go.mod", old, new);
        let subjects: Vec<&str> = f.iter().map(|x| x.subject.as_str()).collect();
        assert_eq!(
            subjects,
            ["github.com/c/d", "github.com/a/b", "github.com/a/b"]
        );
        assert_eq!(f[2].text, "github.com/a/b replaced by a local path (../b)");
        let r = m(
            "requirements.txt",
            "requests==2.31.0\n",
            "requests==2.32.0\n-e git+https://github.com/x/y.git#egg=y\nflask>=3.0\n",
        );
        let subjects: Vec<&str> = r.iter().map(|x| x.subject.as_str()).collect();
        // A package from a git source comes first: it says the most.
        assert_eq!(subjects, ["y", "flask", "requests"]);
        assert_eq!(r[0].text, "package y added from a git source (git+https)");
    }

    #[test]
    fn many_packages_make_one_fact() {
        let deps: Vec<String> = (0..8).map(|i| format!("\"p{i}\": \"1.0.{i}\"")).collect();
        let new = format!("{{\"dependencies\": {{{}}}}}", deps.join(", "));
        let f = m("package.json", "", &new);
        assert_eq!(f.len(), 1);
        assert_eq!(
            f[0].text,
            "8 packages added to package.json: p0, p1, p2 and 5 more"
        );
    }

    #[test]
    fn a_lock_says_which_packages_moved() {
        let old = r#"{"content-hash": "a", "packages": [{"name": "x/y", "version": "1.0.0"}]}"#;
        let hash_only = old.replace("\"a\"", "\"b\"");
        assert!(lock_changes("composer.lock", old, &hash_only).is_empty());
        let bumped = old.replace("1.0.0", "1.1.0");
        assert_eq!(
            lock_changes("composer.lock", old, &bumped),
            ["x/y 1.0.0 → 1.1.0"]
        );
        // A lock teer can't read names nothing.
        assert!(lock_changes("Cargo.lock", "dep 1\n", "pkg 1\n").is_empty());
    }

    #[test]
    fn locks_read_their_versions() {
        let yarn = "\"@babel/core@^7.0.0\", \"@babel/core@^7.1.0\":\n  version \"7.2.0\"\n\nleft-pad@^1.0.0:\n  version \"1.3.0\"\n";
        let p = lock_packages("yarn.lock", yarn);
        assert_eq!(p["@babel/core"], "7.2.0");
        assert_eq!(p["left-pad"], "1.3.0");
        // A broken header is no reason to fail.
        let _ = lock_packages("yarn.lock", ":\n\"\":\nü@1:\n  version \"1\"\n");
        let composer = r#"{"packages": [{"name": "guzzlehttp/psr7", "version": "2.8.0"}]}"#;
        assert_eq!(
            lock_packages("composer.lock", composer)["guzzlehttp/psr7"],
            "2.8.0"
        );
        let sum = "github.com/a/b v1.2.0 h1:x=\ngithub.com/a/b v1.2.0/go.mod h1:y=\n";
        assert_eq!(lock_packages("go.sum", sum)["github.com/a/b"], "v1.2.0");
        let pnpm = "packages:\n  /left-pad@1.3.0:\n    resolution: {}\n  '@scope/x@2.0.0':\n    resolution: {}\n";
        let p = lock_packages("pnpm-lock.yaml", pnpm);
        assert_eq!(p["left-pad"], "1.3.0");
        assert_eq!(p["@scope/x"], "2.0.0");
    }
}
