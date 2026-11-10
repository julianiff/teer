//! Facts read from a unit's added lines, in any file: secrets, security
//! config loosened, weak crypto. A line is added when the old file has no
//! line like it, so moving code doesn't count.

use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use std::sync::LazyLock;

use regex::Regex;

use super::fact;
use crate::security::model::{Class, Fact, Weight};

/// A unit's added lines, and the files around them.
pub(crate) struct Added<'a> {
    pub path: &'a Path,
    pub old_text: &'a str,
    pub new_text: &'a str,
    /// Every line of the new file.
    pub new_lines: Rc<Vec<&'a str>>,
    /// The unit's added lines, 1-based, on the new side.
    pub lines: Vec<u32>,
    /// The unit is test code wherever its file is: Rust's `mod tests`.
    pub test_code: bool,
}

impl<'a> Added<'a> {
    /// The lines of `new_lines` in `range` (all of them when `None`) that
    /// `keep` keeps and that `old` (the old file's lines, trimmed) lacks.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: &'a Path,
        old_text: &'a str,
        new_text: &'a str,
        old: &HashSet<&str>,
        new_lines: Rc<Vec<&'a str>>,
        range: Option<(u32, u32)>,
        keep: impl Fn(u32) -> bool,
    ) -> Added<'a> {
        let (from, to) = range.unwrap_or((1, new_lines.len() as u32));
        let lines = (from.max(1)..=to.min(new_lines.len() as u32))
            .filter(|&l| {
                let t = new_lines[l as usize - 1].trim();
                !t.is_empty() && !old.contains(t) && keep(l)
            })
            .collect();
        Added {
            path,
            old_text,
            new_text,
            new_lines,
            lines,
            test_code: false,
        }
    }

    /// Every added line of a whole file.
    #[cfg(test)]
    pub fn of(path: &'a Path, old_text: &'a str, new_text: &'a str) -> Added<'a> {
        let old: HashSet<&str> = old_text.lines().map(str::trim).collect();
        let new_lines = Rc::new(new_text.lines().collect());
        Added::new(path, old_text, new_text, &old, new_lines, None, |_| true)
    }

    pub fn text(&self, line: u32) -> &'a str {
        line.checked_sub(1)
            .and_then(|i| self.new_lines.get(i as usize))
            .copied()
            .unwrap_or("")
    }

    fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    fn ext(&self) -> String {
        self.path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    }

    /// A test, a fixture or a mock.
    pub fn in_tests(&self) -> bool {
        let name = self.name().to_ascii_lowercase();
        self.test_code
            || name.starts_with("test_")
            || self.path.components().any(|c| {
                matches!(
                    c.as_os_str()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .as_str(),
                    "test"
                        | "tests"
                        | "__tests__"
                        | "spec"
                        | "specs"
                        | "fixtures"
                        | "fixture"
                        | "testdata"
                        | "__mocks__"
                        | "mocks"
                        | "e2e"
                )
            })
            || name.contains(".test.")
            || name.contains(".spec.")
            || name.contains("_test.")
            || name.ends_with("test.php")
    }

    /// Documentation: secrets there are examples.
    fn is_docs(&self) -> bool {
        matches!(self.ext().as_str(), "md" | "mdx" | "rst" | "adoc" | "txt")
    }

    /// `.env.example` and friends: their values are meant to be replaced.
    fn is_example(&self) -> bool {
        let n = self.name().to_ascii_lowercase();
        ["example", "sample", "dist", "template", "tpl"]
            .iter()
            .any(|w| n.contains(w))
    }

    /// For a developer's machine or a test run, not production.
    fn is_dev_config(&self) -> bool {
        let n = self.name().to_ascii_lowercase();
        let dev_name = [".local", ".dev", ".test", "development", "_dev", "-dev"]
            .iter()
            .any(|w| n.contains(w));
        dev_name
            || self.in_tests()
            || self.path.components().any(|c| {
                matches!(
                    c.as_os_str()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .as_str(),
                    "dev" | "development" | "local" | "test" | "testing"
                )
            })
    }

    fn is_env(&self) -> bool {
        self.name().starts_with(".env")
    }

    /// YAML, JSON, TOML, INI, `.env`: not code.
    fn is_config(&self) -> bool {
        self.is_env()
            || matches!(
                self.ext().as_str(),
                "yml" | "yaml" | "json" | "toml" | "ini" | "conf" | "cfg" | "properties" | "neon"
            )
    }
}

// ---- secrets ----

struct KeyFormat {
    what: &'static str,
    re: Regex,
    /// How much of a match may be shown: its well-known prefix.
    prefix: usize,
}

impl KeyFormat {
    /// A match as a fact may show it: `AKIA…`, `sk-proj-…`.
    fn masked(&self, token: &str) -> String {
        match self.what {
            "private key" => "-----BEGIN … PRIVATE KEY-----".into(),
            // `sk-proj-…`, `sk-admin-…`, else `sk-…`.
            "OpenAI API key" => {
                let second = token[3..].find('-').map(|i| i + 4);
                match second {
                    Some(end) if end <= 12 => format!("{}…", &token[..end]),
                    _ => "sk-…".into(),
                }
            }
            _ => format!("{}…", &token[..self.prefix.min(token.len())]),
        }
    }
}

static KEYS: LazyLock<Vec<KeyFormat>> = LazyLock::new(|| {
    let k = |what, re: &str, prefix| KeyFormat {
        what,
        re: Regex::new(re).unwrap(),
        prefix,
    };
    vec![
        k("AWS access key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", 4),
        k("GitHub token", r"\bgh[pousr]_[A-Za-z0-9]{36,}\b", 4),
        k("GitHub token", r"\bgithub_pat_[A-Za-z0-9_]{40,}\b", 11),
        k("Slack token", r"\bxox[abposr]-[A-Za-z0-9-]{10,}", 5),
        k("Anthropic API key", r"\bsk-ant-[A-Za-z0-9_-]{30,}", 7),
        k("Stripe live key", r"\b[sr]k_live_[A-Za-z0-9]{16,}", 8),
        k(
            "OpenAI API key",
            r"\bsk-(?:proj|svcacct|admin)-[A-Za-z0-9_-]{40,}|\bsk-[A-Za-z0-9]{20}T3BlbkFJ[A-Za-z0-9]{20}|\bsk-[A-Za-z0-9]{48}\b",
            3,
        ),
        k("Google API key", r"\bAIza[0-9A-Za-z_-]{35}", 4),
        k("GitLab token", r"\bglpat-[A-Za-z0-9_-]{20,}", 6),
        k(
            "JWT",
            r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
            3,
        ),
        k(
            "private key",
            r"-----BEGIN (?:[A-Z]+ )?PRIVATE KEY(?: BLOCK)?-----",
            0,
        ),
    ]
});

/// A name a secret is given: `DB_PASSWORD`, `apiKey`, `client_secret`.
const SECRET_NAME: &str = r"[A-Za-z0-9_.\-]*(?i:password|passwd|passphrase|secret|token|api[_-]?key|apikey|access[_-]?key|private[_-]?key|auth[_-]?key|credentials?)[A-Za-z0-9_.\-]*";

static QUOTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?P<name>{SECRET_NAME})['"]?\s*(?:=>|:=|=|:)\s*[rbfu]?['"](?P<val>[^'"\s]{{10,200}})['"]"#
    ))
    .unwrap()
});

static UNQUOTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r##"^\s*(?:export\s+)?(?P<name>{SECRET_NAME})\s*[:=]\s*(?P<val>[^\s'"#]{{10,200}})\s*$"##
    ))
    .unwrap()
});

/// A value no one means to keep: `xxxx`, `changeme`, `<your key>`.
fn placeholder(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    [
        "example",
        "xxx",
        "placeholder",
        "changeme",
        "change_me",
        "dummy",
        "fake",
        "sample",
        "your",
        "redacted",
        "test",
        "123456",
        "abcdef",
        "...",
        "<",
        ">",
        "${",
        "{{",
        "****",
        "sm9obibkb2u", // "John Doe" in base64: jwt.io's example token
    ]
    .iter()
    .any(|p| l.contains(p))
        || {
            let distinct: HashSet<char> = v.chars().collect();
            v.len() >= 16 && distinct.len() < 8
        }
}

fn entropy(s: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    let n = s.chars().count() as f64;
    counts
        .values()
        .map(|&c| {
            let p = c as f64 / n;
            -p * p.log2()
        })
        .sum()
}

/// Does a literal read like a real secret: long enough, mixed, random?
fn looks_secret(v: &str) -> bool {
    if placeholder(v)
        || v.contains("://")
        || v.starts_with(['/', '.', '$', '%', '#', '@'])
        || v.ends_with(['(', ')'])
    {
        return false;
    }
    // Names, keys and routes: `reset_password_token`, `API_TOKEN`.
    let wordy = v
        .chars()
        .all(|c| c.is_ascii_lowercase() || matches!(c, '_' | '-' | '.' | ':'))
        || v.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    let has_digit_or_symbol = v
        .chars()
        .any(|c| c.is_ascii_digit() || !c.is_ascii_alphanumeric());
    let classes = [
        v.chars().any(|c| c.is_ascii_lowercase()),
        v.chars().any(|c| c.is_ascii_uppercase()),
        v.chars().any(|c| c.is_ascii_digit()),
        v.chars().any(|c| !c.is_ascii_alphanumeric()),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    !wordy && has_digit_or_symbol && classes >= 2 && entropy(v) >= 3.0
}

/// Is `name` one a secret is kept in, rather than its type, field, path
/// or label?
fn secret_name(name: &str) -> bool {
    const NOT: [&str; 44] = [
        "type",
        "field",
        "name",
        "label",
        "url",
        "uri",
        "path",
        "file",
        "param",
        "header",
        "length",
        "len",
        "min",
        "max",
        "policy",
        "regex",
        "pattern",
        "placeholder",
        "hint",
        "input",
        "column",
        "route",
        "endpoint",
        "prefix",
        "suffix",
        "format",
        "hash",
        "id",
        "env",
        "var",
        "ref",
        "count",
        "expires",
        "expiry",
        "ttl",
        "reset",
        "confirm",
        "confirmation",
        "rule",
        "rules",
        "required",
        "csrf",
        "xsrf",
        "mode",
    ];
    let w = super::scan::words(name);
    !w.iter().any(|x| NOT.contains(&x.as_str()))
}

/// Hard-coded credentials in added lines: known key formats anywhere, and
/// literals given a secret's name outside tests, docs and examples.
pub(crate) fn secrets(a: &Added) -> Vec<Fact> {
    let mut out = Vec::new();
    let tests = a.in_tests();
    let generic = !tests && !a.is_docs() && !a.is_example();
    for &l in &a.lines {
        let text = a.text(l);
        if text.len() > 2000 {
            continue;
        }
        let mut found = false;
        for k in KEYS.iter() {
            for m in k.re.find_iter(text) {
                let token = m.as_str();
                if k.what == "OpenAI API key" && token.starts_with("sk-ant-") {
                    continue;
                }
                if k.what != "private key" && placeholder(token) {
                    continue;
                }
                let weight = if tests && matches!(k.what, "JWT" | "private key") {
                    Weight::Look
                } else {
                    Weight::Attention
                };
                out.push(fact(
                    Class::Secret,
                    weight,
                    format!("hard-coded {} ({})", k.what, k.masked(token)),
                    l,
                    k.what,
                ));
                found = true;
            }
        }
        if found || !generic {
            continue;
        }
        let caps = QUOTED.captures(text).or_else(|| {
            (a.is_env() || a.is_config())
                .then(|| UNQUOTED.captures(text))
                .flatten()
        });
        if let Some(c) = caps {
            let name = c.name("name").map(|m| m.as_str()).unwrap_or("");
            let val = c.name("val").map(|m| m.as_str()).unwrap_or("");
            if secret_name(name) && looks_secret(val) {
                let shown = crate::util::truncate(name, 40);
                out.push(fact(
                    Class::Secret,
                    Weight::Attention,
                    format!("hard-coded secret in {shown}"),
                    l,
                    &shown,
                ));
            }
        }
    }
    out
}

/// `text` with every known key format masked: for showing a command.
pub(crate) fn mask_secrets(text: &str) -> String {
    let mut out = text.to_string();
    for k in KEYS.iter() {
        out =
            k.re.replace_all(&out, |c: &regex::Captures| k.masked(&c[0]))
                .to_string();
    }
    out
}

// ---- security config ----

static CORS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r#"(?i)access-control-allow-origin['"]?\s*[:,=]?\s*['"]?\s*\*"#,
        r#"(?i)\b(?:allow[_-]?origins?|allowed[_-]?origins?|cors[_-]?origins?|origins?)['"]?\s*(?:=>|:|=)\s*\[?\s*(?:\[\]string\{\s*)?['"]\*['"]"#,
        r"\bAllowAllOrigins\s*[:=]\s*true\b",
    ]
    .iter()
    .map(|r| Regex::new(r).unwrap())
    .collect()
});

static COOKIE_NAMED: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (
            r#"(?i)http_?only['"]?\s*(?:=>|:|=)\s*(?:false|0)\b"#,
            "HttpOnly",
        ),
        (
            r#"(?i)cookie[_.]?secure['"]?\s*(?:=>|:|=)\s*['"]?(?:false|0|off)\b"#,
            "Secure",
        ),
        (
            r#"(?i)\bsame_?site['"]?\s*(?:=>|:|=)\s*['"]?(?:none|false)\b"#,
            "SameSite",
        ),
        (r"\bSameSiteNoneMode\b", "SameSite"),
    ]
    .iter()
    .map(|(r, w)| (Regex::new(r).unwrap(), *w))
    .collect()
});

static SECURE_FALSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:^|[^\w])['"]?secure['"]?\s*(?:=>|:|=)\s*(?:false|0)\b"#).unwrap()
});

static TLS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r#"(?i)['"]verify['"]\s*=>\s*false\b"#,
        r"(?i)CURLOPT_SSL_VERIFY(?:PEER|HOST)\s*(?:=>|,)\s*(?:false|0)\b",
        r#"\brejectUnauthorized['"]?\s*:\s*false\b"#,
        r#"\bNODE_TLS_REJECT_UNAUTHORIZED['"]?\s*[:=]\s*['"]?0"#,
        r"\bInsecureSkipVerify\s*:\s*true\b",
        r"\bverify\s*=\s*False\b",
        r#"(?i)\bverify_peer(?:_name)?['"]?\s*(?:=>|:|=)\s*false\b"#,
        r#"\bstrictSSL['"]?\s*:\s*false\b"#,
        r#"(?i)\bssl[_-]?verify['"]?\s*(?:=>|:|=)\s*false\b"#,
        r"\bdanger_accept_invalid_certs\(\s*true\s*\)",
    ]
    .iter()
    .map(|r| Regex::new(r).unwrap())
    .collect()
});

static DEBUG_ENV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*(?:export\s+)?(?:APP_DEBUG|DEBUG)\s*=\s*['"]?(?:true|1|on)\b"#).unwrap()
});
static DEBUG_CONFIG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)^\s*['"]?(?:app_)?debug['"]?\s*(?:[:=]|=>)\s*['"]?(?:true|1|on)\b"#).unwrap()
});
static DEBUG_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*DEBUG\s*=\s*True\b").unwrap());
static CSP_UNSAFE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"'unsafe-(?:inline|eval)'").unwrap());

/// What matched, as a fact shows it: one line, short.
fn snippet(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    crate::util::truncate(
        one.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '\''),
        48,
    )
}

/// Security config loosened by added lines: CORS open to any origin,
/// cookies losing a flag, TLS verification off, debug on, CSP unsafe.
pub(crate) fn config(a: &Added) -> Vec<Fact> {
    let mut out = Vec::new();
    // Tests turn TLS off for their own servers, and so on.
    if a.in_tests() {
        return out;
    }
    let dev = a.is_dev_config();
    for &l in &a.lines {
        let text = a.text(l);
        if text.len() > 500 {
            continue;
        }
        if let Some(m) = CORS.iter().find_map(|r| r.find(text)) {
            out.push(fact(
                Class::Config,
                Weight::Look,
                format!("CORS allows any origin ({})", snippet(m.as_str())),
                l,
                "cors",
            ));
        }
        let cookie = COOKIE_NAMED
            .iter()
            .find_map(|(r, w)| r.find(text).map(|m| (m, *w)))
            .or_else(|| {
                let m = SECURE_FALSE.find(text)?;
                let from = l.saturating_sub(5).max(1);
                let around: String = (from..=l).map(|i| a.text(i)).collect::<Vec<_>>().join("\n");
                let lower = around.to_ascii_lowercase();
                (lower.contains("cookie") || lower.contains("session")).then_some((m, "Secure"))
            });
        if let Some((m, flag)) = cookie {
            let what = if flag == "SameSite" {
                "cookie SameSite set to none".to_string()
            } else {
                format!("cookie loses {flag}")
            };
            out.push(fact(
                Class::Config,
                Weight::Look,
                format!("{what} ({})", snippet(m.as_str())),
                l,
                "cookie",
            ));
        }
        if let Some(m) = TLS.iter().find_map(|r| r.find(text)) {
            out.push(fact(
                Class::Config,
                Weight::Look,
                format!("TLS verification turned off ({})", snippet(m.as_str())),
                l,
                "tls",
            ));
        }
        let debug = if dev || a.is_example() {
            None
        } else if a.is_env() {
            DEBUG_ENV.find(text)
        } else if a.is_config() {
            DEBUG_CONFIG.find(text)
        } else if a.ext() == "py" {
            DEBUG_CODE.find(text)
        } else if a.ext() == "php" && a.path.components().any(|c| c.as_os_str() == "config") {
            DEBUG_CONFIG.find(text)
        } else {
            None
        };
        if let Some(m) = debug {
            out.push(fact(
                Class::Config,
                Weight::Look,
                format!("debug turned on ({})", snippet(m.as_str())),
                l,
                "debug",
            ));
        }
        if let Some(m) = CSP_UNSAFE.find(text) {
            out.push(fact(
                Class::Config,
                Weight::Look,
                format!("CSP allows {}", m.as_str()),
                l,
                "csp",
            ));
        }
    }
    out
}

// ---- crypto ----

static WEAK_HASH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(md5|sha1)\b\s*(?:\(|\.Sum|\.New)|createHash\(\s*['"](md5|sha1)['"]"#)
        .unwrap()
});
static PASSWORD_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)pass(?:word|wd|phrase)?\b|password").unwrap());
static WEAK_RANDOM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bMath\.random\(\)|\bmt_rand\s*\(|\brand\s*\(|\buniqid\s*\(|\brandom\.(?:random|randint|choice)\(|\brand\.(?:Intn|Int63n?|Int31n?|Int|Uint32|Uint64|Read|Float64)\(").unwrap()
});
static SECRET_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)token|secret|nonce|passw|salt|otp|api_?key|session_?id|csrf|reset_?code|verification_?code").unwrap()
});
static SECRET_COMPARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\$|\b)[\w.>-]*(?:token|secret|signature|hmac|digest|api_?key|password_?hash)\b\s*(?:===?|!==?)\s*([^\s);]+)|([^\s(=!]+)\s*(?:===?|!==?)\s*(?:\$|\b)[\w.>-]*(?:token|secret|signature|hmac|digest|api_?key|password_?hash)\b").unwrap()
});

/// Weak crypto in added lines: `md5` or `sha1` on a password, non-crypto
/// randomness for a token, `==` on a secret.
pub(crate) fn crypto(a: &Added) -> Vec<Fact> {
    let mut out = Vec::new();
    if a.is_docs() || a.is_config() || a.in_tests() {
        return out;
    }
    let code = matches!(
        a.ext().as_str(),
        "php" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" | "go" | "py" | "rb"
    );
    let go_math_rand = a.ext() == "go" && a.new_text.contains("\"math/rand");
    for &l in &a.lines {
        let text = a.text(l);
        if text.len() > 500 {
            continue;
        }
        if let Some(c) = WEAK_HASH.captures(text)
            && PASSWORD_WORD.is_match(text)
        {
            let algo = c
                .get(1)
                .or_else(|| c.get(2))
                .map(|m| m.as_str().to_ascii_lowercase())
                .unwrap_or_default();
            out.push(fact(
                Class::Crypto,
                Weight::Note,
                format!("{algo} used on a password"),
                l,
                &algo,
            ));
        }
        if let Some(m) = WEAK_RANDOM.find(text)
            && SECRET_WORD.is_match(text)
            && (!m.as_str().starts_with("rand.") || go_math_rand)
        {
            let what = m.as_str().trim_end_matches(['(', ' ']).to_string();
            out.push(fact(
                Class::Crypto,
                Weight::Note,
                format!("{what}() makes a secret; it isn't random enough"),
                l,
                &what,
            ));
        }
        if code && let Some(c) = SECRET_COMPARE.captures(text) {
            let other = c
                .get(1)
                .or_else(|| c.get(2))
                .map(|m| m.as_str())
                .unwrap_or("");
            let trivial = other
                .trim_matches(|ch| ch == '\'' || ch == '"')
                .to_ascii_lowercase();
            let literal = matches!(
                trivial.as_str(),
                "" | "null" | "nil" | "undefined" | "true" | "false" | "none"
            ) || trivial.parse::<f64>().is_ok()
                || other.starts_with("typeof");
            if !literal {
                let use_instead = match a.ext().as_str() {
                    "php" => "hash_equals",
                    "go" => "subtle.ConstantTimeCompare",
                    "py" => "hmac.compare_digest",
                    _ => "crypto.timingSafeEqual",
                };
                out.push(fact(
                    Class::Crypto,
                    Weight::Note,
                    format!("a secret compared with ==; {use_instead} doesn't leak timing"),
                    l,
                    "comparison",
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn added<'a>(path: &'a str, old: &'a str, new: &'a str) -> Added<'a> {
        Added::of(Path::new(path), old, new)
    }

    /// A fake key in a known format, put together at run time so no
    /// scanner mistakes this file for a leak.
    fn fake(parts: &[&str]) -> String {
        parts.concat()
    }

    #[test]
    fn added_lines_are_the_ones_the_old_file_lacks() {
        let a = added("a.txt", "x\ny\n", "y\nx\nz\n");
        assert_eq!(a.lines, [3]);
    }

    #[test]
    fn known_key_formats_are_found_and_masked() {
        let aws = fake(&["AKIA", "Q3EGRT7ZPLM2XKVB"]);
        let gh = fake(&["ghp_", "Zx9", "Lm2Qr8Tv4Wy6Ab1Cd3Ef5Gh7Ij9Kl0Mn2Op4Q"]);
        let src = format!("const a = '{aws}';\nconst b = \"{gh}\";\n");
        let f = secrets(&added("src/config.ts", "", &src));
        assert_eq!(f.len(), 2, "{f:?}");
        assert_eq!(f[0].text, "hard-coded AWS access key (AKIA…)");
        assert_eq!(f[0].weight, Weight::Attention);
        assert_eq!(f[1].text, "hard-coded GitHub token (ghp_…)");
        for x in &f {
            assert!(!x.text.contains(&aws) && !x.text.contains(&gh));
        }
        let pem = secrets(&added(
            "certs/key.pem",
            "",
            "-----BEGIN RSA PRIVATE KEY-----\nMIIE\n",
        ));
        assert_eq!(
            pem[0].text,
            "hard-coded private key (-----BEGIN … PRIVATE KEY-----)"
        );
    }

    #[test]
    fn placeholders_and_old_secrets_are_not_found() {
        let example = fake(&["AKIA", "IOSFODNN7EXAMPLE"]);
        let src =
            format!("key = '{example}'\napi_key = 'your-api-key-here'\ntoken = '${{TOKEN}}'\n");
        assert!(secrets(&added("a.py", "", &src)).is_empty());
        let real = fake(&["AKIA", "Q3EGRT7ZPLM2XKVB"]);
        let line = format!("key = '{real}'\n");
        assert!(
            secrets(&added("a.py", &line, &line)).is_empty(),
            "not added"
        );
    }

    #[test]
    fn named_literals_need_entropy() {
        let f = secrets(&added(
            "config/app.php",
            "",
            "'password' => 'Pr0d-Passw0rd!2024',\n'password_reset_route' => 'app_reset_password',\n'token_type' => 'Bearer9x',\n",
        ));
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].text, "hard-coded secret in password");
        assert!(!f[0].text.contains("Pr0d"));
        let env = secrets(&added(
            ".env",
            "",
            "DB_PASSWORD=s3cr3t-Pr0d-2024x\nDB_HOST=localhost\n",
        ));
        assert_eq!(env.len(), 1);
        // Tests and examples are full of them.
        assert!(
            secrets(&added(
                "tests/a.php",
                "",
                "'password' => 'Pr0d-Passw0rd!2024',\n"
            ))
            .is_empty()
        );
        assert!(
            secrets(&added(
                ".env.example",
                "",
                "DB_PASSWORD=s3cr3t-Pr0d-2024x\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn config_loosened() {
        let f = config(&added(
            "src/server.ts",
            "",
            // A mailer's `secure: false` is no cookie's.
            "const mail = { secure: false };\napp.use(cors({ origin: '*' }));\nconst agent = new https.Agent({ rejectUnauthorized: false });\nres.cookie('sid', id, { httpOnly: false });\nconst session = { cookie: { secure: false } };\n",
        ));
        let subjects: Vec<&str> = f.iter().map(|x| x.subject.as_str()).collect();
        assert_eq!(subjects, ["cors", "tls", "cookie", "cookie"], "{f:?}");
        assert_eq!(
            f[1].text,
            "TLS verification turned off (rejectUnauthorized: false)"
        );
        assert_eq!(f[3].text, "cookie loses Secure (secure: false)");
        let php = config(&added(
            "src/Client.php",
            "",
            "$client = new Client(['verify' => false]);\ncurl_setopt($ch, CURLOPT_SSL_VERIFYPEER, false);\n",
        ));
        assert_eq!(php.len(), 2);
        let go = config(&added(
            "tls.go",
            "",
            "cfg := &tls.Config{InsecureSkipVerify: true}\n",
        ));
        assert_eq!(go.len(), 1);
        let env = config(&added(".env", "", "APP_DEBUG=true\n"));
        assert_eq!(env[0].text, "debug turned on (APP_DEBUG=true)");
        assert!(config(&added(".env.local", "", "APP_DEBUG=true\n")).is_empty());
        // Tests turn TLS off for their own servers.
        assert!(
            config(&added(
                "test/server.test.ts",
                "",
                "rejectUnauthorized: false\n"
            ))
            .is_empty()
        );
        let mut inline = added(
            "src/net.rs",
            "",
            "let verify = False;\nrejectUnauthorized: false\n",
        );
        inline.test_code = true;
        assert!(config(&inline).is_empty());
        assert!(config(&added("config/packages/dev/web.yaml", "", "debug: true\n")).is_empty());
    }

    #[test]
    fn weak_crypto_is_a_note() {
        let f = crypto(&added(
            "src/Auth.php",
            "",
            "$hash = md5($password);\n$token = mt_rand();\nif ($token == $given) {}\nif ($token === null) {}\n",
        ));
        let texts: Vec<&str> = f.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "md5 used on a password",
                "mt_rand() makes a secret; it isn't random enough",
                "a secret compared with ==; hash_equals doesn't leak timing"
            ]
        );
        assert!(f.iter().all(|x| x.weight == Weight::Note));
    }
}
