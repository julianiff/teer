//! One side of one file, read for the security facts: its symbols (named
//! as `units::extract` names them), and the checks, entry-point marks,
//! sinks, model calls, logging and responses in them. Each is kept with
//! the symbol whose text holds it (its `owner`: the unit that changes when
//! it does) and the symbol it is about (its `target`: a TypeScript method's
//! decorators and a PHP method's doc block are in its class's text, but
//! are about the method).

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use tree_sitter::Node;

use crate::review::model::{Range, UnitKind};
use crate::security::catalog::{self, Call, Catalog, Form, SinkKind, Taint, entry_how};
use crate::units::extract::{parse_source, parse_tree};
use crate::units::lang::Lang;

/// Why a symbol is an entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// Its route (`POST /users`) or its name (`SyncCommand::execute`).
    pub name: String,
    /// What it is, in words: `route`, `message handler`, `HTTP handler`…
    pub how: &'static str,
}

impl Entry {
    /// "route POST /users", "message handler SendMail".
    pub fn words(&self) -> String {
        format!("{} {}", self.how, self.name)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Sym {
    pub path: String,
    pub name: String,
    pub kind: UnitKind,
    pub range: Range,
    pub parent: Option<usize>,
    /// Its declaration up to the body, attributes left out, without
    /// whitespace.
    pub sig: String,
    /// The short names it calls.
    pub calls: Vec<String>,
    /// PHP visibility (public when unsaid); true elsewhere.
    pub public: bool,
    pub is_abstract: bool,
    handler_sig: bool,
    /// It returns or holds a handler function (`func(w, r) {…}`).
    inline_handler: bool,
    use_server: bool,
    /// It is an entry point.
    pub entry: Option<Entry>,
    /// It registers routes: the first one's.
    pub registers: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum What {
    /// An authorization check: a call, attribute, decorator or middleware.
    Check(String),
    /// An attribute or decorator that makes its target an entry point.
    Mark {
        name: String,
        verb: Option<String>,
        path: Option<String>,
        /// Its first string argument: a command's name.
        first: Option<String>,
    },
    /// A sink given something that isn't a plain literal.
    Sink {
        kind: SinkKind,
        name: String,
        /// What it is given reads like request input.
        request: bool,
    },
    /// A call to a model's API.
    Model(String),
    /// A sensitive value logged.
    Log { label: &'static str, strong: bool },
    /// A sensitive value put into a response.
    Respond { label: &'static str, strong: bool },
}

#[derive(Debug, Clone)]
pub(crate) struct Event {
    /// The symbol whose text holds it.
    pub owner: usize,
    /// The symbol it is about.
    pub target: usize,
    pub line: u32,
    pub what: What,
}

/// A route registered by a call: `router.get('/x', requireAuth, show)`,
/// `Route::post(…)`, `mux.HandleFunc("/x", h)`.
#[derive(Debug, Clone)]
pub(crate) struct RouteReg {
    pub owner: usize,
    pub line: u32,
    /// `GET /x`, or the path alone.
    pub route: String,
    /// Handlers named by reference, short names.
    pub handlers: Vec<String>,
    /// Checks on it: middleware arguments, chained `->middleware('auth')`,
    /// an enclosing group's.
    pub checks: Vec<String>,
    /// Router-wide middleware (`router.use(requireAuth)`), not a route.
    pub is_use: bool,
}

pub(crate) struct Scan {
    pub syms: Vec<Sym>,
    pub events: Vec<Event>,
    pub routes: Vec<RouteReg>,
}

impl Scan {
    pub fn sym(&self, path: &str) -> Option<usize> {
        self.syms.iter().position(|s| s.path == path)
    }

    /// Symbol `i` as an entry point, or as the place that registers one:
    /// its name.
    pub fn entry_name(&self, i: usize) -> Option<String> {
        let s = self.syms.get(i)?;
        s.entry
            .as_ref()
            .map(|e| e.name.clone())
            .or_else(|| s.registers.clone())
    }

    /// The checks on symbol `i`: its own, on it (attributes, decorators),
    /// or in a helper of the same file it calls.
    pub fn checks_on(&self, i: usize) -> Vec<String> {
        let direct = |j: usize| -> Vec<String> {
            self.events
                .iter()
                .filter(|e| e.target == j)
                .filter_map(|e| match &e.what {
                    What::Check(c) => Some(c.clone()),
                    _ => None,
                })
                .collect()
        };
        let mut out = direct(i);
        if out.is_empty()
            && let Some(s) = self.syms.get(i)
        {
            for (j, other) in self.syms.iter().enumerate() {
                if j != i
                    && matches!(other.kind, UnitKind::Function | UnitKind::Method)
                    && other.parent == s.parent
                    && s.calls.contains(&other.name)
                {
                    out.extend(direct(j));
                }
            }
        }
        out
    }
}

/// Read `src` (the file `path`, in `lang`) with `cat`. `None` when it
/// doesn't parse.
pub(crate) fn scan(lang: Lang, path: &Path, src: &str, cat: &Catalog) -> Option<Scan> {
    let parsed = parse_source(lang, src)?;
    let tree = parse_tree(lang, src)?;
    let mut syms: Vec<Sym> = parsed
        .symbols
        .iter()
        .map(|s| Sym {
            path: s.path.clone(),
            name: s.name.clone(),
            kind: s.kind,
            range: s.range,
            parent: None,
            sig: String::new(),
            calls: s.calls.iter().map(|c| c.name.clone()).collect(),
            public: true,
            is_abstract: false,
            handler_sig: false,
            inline_handler: false,
            use_server: false,
            entry: None,
            registers: None,
        })
        .collect();
    let by_path: HashMap<String, usize> = syms
        .iter()
        .enumerate()
        .map(|(i, s)| (s.path.clone(), i))
        .collect();
    for s in syms.iter_mut() {
        s.parent = s
            .path
            .rsplit_once("::")
            .and_then(|(p, _)| by_path.get(p).copied());
    }
    let n_lines = src.lines().count().max(1);
    let mut line_owner = vec![0usize; n_lines];
    let mut order: Vec<usize> = (1..syms.len()).collect();
    // Outer symbols first, so inner ones paint over them.
    order.sort_by_key(|&i| std::cmp::Reverse(syms[i].range.len()));
    for i in order {
        let r = syms[i].range;
        for l in r.start.max(1)..=r.end.min(n_lines as u32) {
            line_owner[l as usize - 1] = i;
        }
    }
    let mut w = Walker {
        lang,
        src: src.as_bytes(),
        text: src,
        cat,
        syms,
        line_owner,
        events: Vec::new(),
        routes: Vec::new(),
        assigns: HashMap::new(),
        file_use_server: false,
    };
    let root = tree.root_node();
    w.collect_assigns(root);
    w.walk(root);
    w.finish(path);
    Some(Scan {
        syms: w.syms,
        events: w.events,
        routes: w.routes,
    })
}

struct Walker<'t, 'a> {
    lang: Lang,
    src: &'a [u8],
    text: &'a str,
    cat: &'a Catalog,
    syms: Vec<Sym>,
    line_owner: Vec<usize>,
    events: Vec<Event>,
    routes: Vec<RouteReg>,
    /// What each local name is assigned, per symbol: (value, `.=`/`+=`).
    assigns: HashMap<(usize, String), Vec<(Node<'t>, bool)>>,
    file_use_server: bool,
}

fn line(n: Node) -> u32 {
    n.start_position().row as u32 + 1
}

fn kids<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.children(&mut c).collect()
}

fn named<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.named_children(&mut c)
        .filter(|k| !k.kind().contains("comment"))
        .collect()
}

/// `App\Http\charge` → `charge`.
fn last_segment(s: &str) -> &str {
    s.rsplit(['\\', '.', ':']).next().unwrap_or(s).trim()
}

const HTTP_VERBS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

static FIRST_STRING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"['"`]([^'"`]*)['"`]"#).unwrap());
static PATH_ARG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bpath\s*[:=]\s*['"]([^'"]*)['"]"#).unwrap());
static METHODS_ARG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)\bmethods?\s*[:=]\s*[\[{(]?([^\]})]*)"#).unwrap());
static VERB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\b").unwrap());
static ANNOTATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@([A-Za-z_][A-Za-z0-9_\\]*)\s*(\(([^)]*)\))?").unwrap());
static ROUTERISH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(app|router|routes?|server|fastify|api|r|v\d+|\w*router|\w*app)$").unwrap()
});
static REQUEST_PHP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\$(request|req)\b|\$_(GET|POST|REQUEST|COOKIE|FILES|SERVER)\b|->(query|request|attributes|headers|cookies|files)->|->input\(|->getContent\(|->route\(").unwrap()
});
static REQUEST_TS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(req|request)\.(body|query|params|headers|cookies|url|originalUrl|path|get)\b|\bctx\.(request|params|query)\b|\bsearchParams\b|\bformData\b|\bparams\.\w+|\bquery\.\w+|\bbody\.\w+").unwrap()
});
static REQUEST_GO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\br\.(URL|Form|PostForm|Body|Header|FormValue|PostFormValue|PathValue|Cookie)\b|\bc\.(Param|Query|PostForm|FormValue|DefaultQuery|GetHeader|QueryParam)\b|\bmux\.Vars\b|\bchi\.URLParam\b|\.Query\(\)\.Get\b").unwrap()
});

/// The first string literal in `s`, without its quotes.
fn first_string(s: &str) -> Option<String> {
    FIRST_STRING.captures(s).map(|c| c[1].to_string())
}

/// What a route mark says: its verb and path. `#[Route('/x', methods:
/// ['POST'])]` → (POST, /x); `@Get(':id')` → (GET, :id).
fn mark_route(name: &str, args: &str) -> (Option<String>, Option<String>) {
    let upper = name.to_ascii_uppercase();
    let verb = if HTTP_VERBS.contains(&upper.as_str()) {
        Some(upper)
    } else {
        METHODS_ARG.captures(args).and_then(|c| {
            let verbs: Vec<String> = VERB
                .find_iter(&c[1])
                .map(|m| m.as_str().to_ascii_uppercase())
                .collect();
            (!verbs.is_empty()).then(|| verbs.join("|"))
        })
    };
    let path = PATH_ARG
        .captures(args)
        .map(|c| c[1].to_string())
        .or_else(|| first_string(args));
    (verb, path)
}

/// `prefix` and `path` joined as one route path: `users` + `:id` →
/// `/users/:id`.
fn join_path(prefix: Option<&str>, path: Option<&str>) -> String {
    let mut out = String::new();
    for part in [prefix, path].into_iter().flatten() {
        let p = part.trim_matches('/');
        if !p.is_empty() {
            out.push('/');
            out.push_str(p);
        }
    }
    if out.is_empty() { "/".into() } else { out }
}

fn route_words(verb: Option<&str>, path: &str) -> String {
    match verb {
        Some(v) => format!("{v} {path}"),
        None => path.to_string(),
    }
}

/// Next.js: the route of an App Router `route.ts`, or of a file under
/// `pages/api`.
fn next_route(path: &Path) -> Option<(String, bool)> {
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let file = parts.last()?;
    let (stem, ext) = file.rsplit_once('.')?;
    if !matches!(ext, "ts" | "js" | "tsx" | "jsx" | "mjs") {
        return None;
    }
    let keep = |p: &&String| !(p.starts_with('(') && p.ends_with(')')) && !p.starts_with('@');
    if stem == "route"
        && let Some(app) = parts.iter().rposition(|p| p == "app")
    {
        let segs: Vec<&String> = parts[app + 1..parts.len() - 1]
            .iter()
            .filter(keep)
            .collect();
        let route = format!(
            "/{}",
            segs.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("/")
        );
        return Some((route, false));
    }
    let api = parts
        .windows(2)
        .position(|w| w[0] == "pages" && w[1] == "api")?;
    let mut segs: Vec<&str> = parts[api + 1..parts.len() - 1]
        .iter()
        .map(|s| s.as_str())
        .collect();
    if stem != "index" {
        segs.push(stem);
    }
    Some((format!("/{}", segs.join("/")), true))
}

/// Words of an identifier: `apiKey` → api, key; `DB_PASSWORD` → db,
/// password.
pub(crate) fn words(ident: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = ident.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let split = c.is_uppercase()
            && i > 0
            && (chars[i - 1].is_lowercase()
                || chars[i - 1].is_ascii_digit()
                || chars.get(i + 1).is_some_and(|n| n.is_lowercase())
                    && chars[i - 1].is_uppercase());
        if split && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Is a value of this name sensitive? Its label, and whether it is
/// strongly so (a password, a token) rather than personal (an email).
pub(crate) fn sensitive(ident: &str) -> Option<(&'static str, bool)> {
    // Words that make it about the value, not the value: `tokenCount`,
    // `passwordResetUrl`, `secretName`.
    const NOT: &str = "count reset url uri link type expires expiry expired expiration \
        length len field label max min limit usage csrf xsrf page next prev cursor required \
        valid validate invalid policy rule rules strength hint name id ttl lifetime is has \
        enabled verified sent template format pattern placeholder input column path hasher \
        encoder validator";
    let w = words(ident);
    if w.is_empty() || w.iter().any(|x| NOT.split_whitespace().any(|n| n == x)) {
        return None;
    }
    let has = |s: &str| w.iter().any(|x| x == s);
    let pair = |a: &str, b: &str| w.windows(2).any(|p| p[0] == a && p[1] == b);
    if has("password") || has("passwd") || has("passphrase") {
        Some(("password", true))
    } else if has("secret") || has("secrets") {
        Some(("secret", true))
    } else if has("apikey") || pair("api", "key") {
        Some(("api key", true))
    } else if has("privatekey") || pair("private", "key") {
        Some(("private key", true))
    } else if has("token") || has("accesstoken") || has("refreshtoken") {
        Some(("token", true))
    } else if has("authorization") {
        Some(("authorization", true))
    } else if has("creditcard") || pair("credit", "card") || pair("card", "number") {
        Some(("card number", true))
    } else if has("cvv") || has("cvc") {
        Some(("card code", true))
    } else if has("ssn") || pair("social", "security") {
        Some(("ssn", true))
    } else if has("email") {
        Some(("email", false))
    } else if has("phone") {
        Some(("phone", false))
    } else {
        None
    }
}

impl<'t, 'a> Walker<'t, 'a> {
    fn txt(&self, n: Node) -> &'a str {
        n.utf8_text(self.src).unwrap_or("")
    }

    fn owner_at(&self, line: u32) -> usize {
        let i = (line.max(1) as usize - 1).min(self.line_owner.len().saturating_sub(1));
        self.line_owner.get(i).copied().unwrap_or(0)
    }

    fn walk(&mut self, n: Node<'t>) {
        match self.lang {
            Lang::Php => self.php(n),
            Lang::Go => self.go(n),
            Lang::JavaScript | Lang::TypeScript | Lang::Tsx => self.ts(n),
            Lang::Rust | Lang::Python => {}
        }
        for k in kids(n) {
            self.walk(k);
        }
    }

    // ---- local names: what they are assigned ----

    fn collect_assigns(&mut self, n: Node<'t>) {
        let owner = self.owner_at(line(n));
        // (name, value, appended to with `.=` or `+=`)
        let mut found: Vec<(&str, Node<'t>, bool)> = Vec::new();
        let left = n.child_by_field_name("left");
        let right = n.child_by_field_name("right");
        match (self.lang, n.kind()) {
            (Lang::Php, "assignment_expression" | "augmented_assignment_expression") => {
                if let (Some(l), Some(r)) = (left, right)
                    && l.kind() == "variable_name"
                {
                    let append = n.kind() == "augmented_assignment_expression";
                    found.push((self.txt(l), r, append));
                }
            }
            (Lang::Go, "short_var_declaration" | "assignment_statement") => {
                if let (Some(l), Some(r)) = (left, right) {
                    let append = self
                        .txt(n)
                        .split_once('=')
                        .is_some_and(|(a, _)| a.ends_with('+'));
                    for (name, value) in named(l).into_iter().zip(named(r)) {
                        if name.kind() == "identifier" {
                            found.push((self.txt(name), value, append));
                        }
                    }
                }
            }
            (Lang::Go, "var_spec") => {
                if let Some(v) = n.child_by_field_name("value") {
                    let mut c = n.walk();
                    let names: Vec<Node<'t>> = n.children_by_field_name("name", &mut c).collect();
                    for (name, value) in names.into_iter().zip(named(v)) {
                        found.push((self.txt(name), value, false));
                    }
                }
            }
            (Lang::JavaScript | Lang::TypeScript | Lang::Tsx, "variable_declarator") => {
                if let (Some(name), Some(v)) = (
                    n.child_by_field_name("name"),
                    n.child_by_field_name("value"),
                ) && name.kind() == "identifier"
                {
                    found.push((self.txt(name), v, false));
                }
            }
            (
                Lang::JavaScript | Lang::TypeScript | Lang::Tsx,
                "assignment_expression" | "augmented_assignment_expression",
            ) => {
                if let (Some(l), Some(r)) = (left, right)
                    && l.kind() == "identifier"
                {
                    let append = n.kind() == "augmented_assignment_expression";
                    found.push((self.txt(l), r, append));
                }
            }
            _ => {}
        }
        for (name, value, append) in found {
            let name = name.trim_start_matches('$').to_string();
            if !name.is_empty() {
                self.assigns
                    .entry((owner, name))
                    .or_default()
                    .push((value, append));
            }
        }
        for k in kids(n) {
            self.collect_assigns(k);
        }
    }

    // ---- values: literal, built, from the request ----

    /// `n` holds a value that isn't a literal: a variable, a property, a
    /// call's argument that is one. Constants and calls of literals don't
    /// count.
    fn has_var(&self, n: Node) -> bool {
        let kind = n.kind();
        if kind.contains("comment") {
            return false;
        }
        match self.lang {
            Lang::Php => match kind {
                "variable_name" | "dynamic_variable_name" => true,
                "function_call_expression" => n
                    .child_by_field_name("arguments")
                    .is_some_and(|a| self.has_var(a)),
                "string"
                | "nowdoc"
                | "integer"
                | "float"
                | "boolean"
                | "null"
                | "name"
                | "qualified_name"
                | "class_constant_access_expression" => false,
                _ => named(n).into_iter().any(|k| self.has_var(k)),
            },
            Lang::Go => match kind {
                "identifier" => !is_constant_name(self.txt(n), &["nil", "true", "false", "iota"]),
                // `r.FormValue("x")` reads a variable; `fmt.Sprint("x")` doesn't.
                "call_expression" => {
                    n.child_by_field_name("arguments")
                        .is_some_and(|a| self.has_var(a))
                        || n.child_by_field_name("function")
                            .filter(|f| f.kind() == "selector_expression")
                            .and_then(|f| f.child_by_field_name("operand"))
                            .is_some_and(|o| {
                                !(o.kind() == "identifier" && GO_PACKAGES.contains(&self.txt(o)))
                                    && self.has_var(o)
                            })
                }
                "selector_expression" => n
                    .child_by_field_name("operand")
                    .is_some_and(|o| self.has_var(o)),
                "interpreted_string_literal"
                | "raw_string_literal"
                | "int_literal"
                | "float_literal"
                | "rune_literal"
                | "true"
                | "false"
                | "nil" => false,
                "func_literal" => false,
                _ => named(n).into_iter().any(|k| self.has_var(k)),
            },
            _ => match kind {
                "identifier" => !is_constant_name(
                    self.txt(n),
                    &["undefined", "NaN", "Infinity", "__dirname", "__filename"],
                ),
                "this" | "shorthand_property_identifier" => true,
                "new_expression" => n
                    .child_by_field_name("arguments")
                    .is_some_and(|a| self.has_var(a)),
                // `req.get('x')` reads a variable; `path.join('a')` doesn't.
                "call_expression" => {
                    n.child_by_field_name("arguments")
                        .is_some_and(|a| self.has_var(a))
                        || n.child_by_field_name("function")
                            .filter(|f| f.kind() == "member_expression")
                            .and_then(|f| f.child_by_field_name("object"))
                            .is_some_and(|o| {
                                !(o.kind() == "identifier" && JS_GLOBALS.contains(&self.txt(o)))
                                    && self.has_var(o)
                            })
                }
                "member_expression" => {
                    !self.txt(n).starts_with("process.env")
                        && n.child_by_field_name("object")
                            .is_some_and(|o| self.has_var(o))
                }
                "pair" => n
                    .child_by_field_name("value")
                    .is_some_and(|v| self.has_var(v)),
                "string"
                | "number"
                | "true"
                | "false"
                | "null"
                | "undefined"
                | "regex"
                | "property_identifier"
                | "arrow_function"
                | "function_expression"
                | "function" => false,
                _ => named(n).into_iter().any(|k| self.has_var(k)),
            },
        }
    }

    /// `n` is a string built from a variable: interpolation, concatenation,
    /// `sprintf`, or a local name assigned one.
    fn built(&self, n: Node, owner: usize, depth: u8) -> bool {
        let op = |n: Node| {
            n.child_by_field_name("operator")
                .map(|o| self.txt(o))
                .unwrap_or("")
        };
        match (self.lang, n.kind()) {
            (_, "parenthesized_expression") => named(n)
                .first()
                .is_some_and(|c| self.built(*c, owner, depth)),
            (Lang::Php, "encapsed_string" | "heredoc") => self.has_var(n),
            (Lang::Php, "binary_expression") => op(n) == "." && self.has_var(n),
            (Lang::Php, "function_call_expression") => {
                let f = n
                    .child_by_field_name("function")
                    .map(|f| last_segment(self.txt(f)).to_ascii_lowercase())
                    .unwrap_or_default();
                matches!(
                    f.as_str(),
                    "sprintf" | "vsprintf" | "str_replace" | "strtr" | "implode"
                ) && self.has_var(n)
            }
            (Lang::Php, "variable_name") => self.via_name(self.txt(n), owner, depth),
            (Lang::Go, "call_expression") => {
                n.child_by_field_name("function")
                    .is_some_and(|f| self.txt(f).ends_with("Sprintf"))
                    && self.has_var(n)
            }
            (Lang::Go, "binary_expression") => op(n) == "+" && self.has_var(n),
            (Lang::Go, "identifier") => self.via_name(self.txt(n), owner, depth),
            (Lang::Rust | Lang::Python | Lang::Php | Lang::Go, _) => false,
            (_, "template_string") => named(n)
                .into_iter()
                .any(|k| k.kind() == "template_substitution" && self.has_var(k)),
            (_, "binary_expression") => op(n) == "+" && self.has_var(n),
            (_, "identifier") => self.via_name(self.txt(n), owner, depth),
            _ => false,
        }
    }

    fn via_name(&self, name: &str, owner: usize, depth: u8) -> bool {
        if depth >= 2 {
            return false;
        }
        let key = (owner, name.trim_start_matches('$').to_string());
        self.assigns.get(&key).is_some_and(|values| {
            values
                .iter()
                .any(|(v, append)| self.built(*v, owner, depth + 1) || *append && self.has_var(*v))
        })
    }

    /// `n` (or what the names in it are assigned, two steps back) reads
    /// like request input.
    fn reads_request(&self, n: Node, owner: usize) -> bool {
        self.request_in(n, owner, 0)
    }

    fn request_in(&self, n: Node, owner: usize, depth: u8) -> bool {
        let re: &Regex = match self.lang {
            Lang::Php => &REQUEST_PHP,
            Lang::Go => &REQUEST_GO,
            _ => &REQUEST_TS,
        };
        if re.is_match(self.txt(n)) {
            return true;
        }
        if depth >= 2 {
            return false;
        }
        let mut names = Vec::new();
        self.var_names(n, &mut names);
        names.into_iter().any(|name| {
            self.assigns.get(&(owner, name)).is_some_and(|vs| {
                vs.iter()
                    .any(|(v, _)| self.request_in(*v, owner, depth + 1))
            })
        })
    }

    fn var_names(&self, n: Node, out: &mut Vec<String>) {
        match n.kind() {
            "identifier" | "variable_name" => {
                out.push(self.txt(n).trim_start_matches('$').to_string())
            }
            _ => {
                for k in named(n) {
                    self.var_names(k, out);
                }
            }
        }
    }

    /// Does a sink called with `args` get what `taint` asks for? Then
    /// whether it reads like request input.
    fn tainted(&self, taint: Taint, args: &[Node], owner: usize) -> Option<bool> {
        let pick = |i: usize| args.get(i).or(args.last()).copied();
        match taint {
            Taint::NotLiteral(i) => {
                let a = pick(i)?;
                self.has_var(a).then(|| self.reads_request(a, owner))
            }
            Taint::Built(i) => {
                let a = pick(i)?;
                self.built(a, owner, 0)
                    .then(|| self.reads_request(a, owner))
            }
            Taint::AnyNotLiteral(i) => {
                let vars: Vec<&Node> = args.iter().skip(i).filter(|a| self.has_var(**a)).collect();
                (!vars.is_empty()).then(|| vars.iter().any(|a| self.reads_request(**a, owner)))
            }
            Taint::ShellOption => {
                let is_options = |a: &Node| a.kind() == "object";
                let shell = args.iter().filter(|a| is_options(a)).any(|a| {
                    let t = self.txt(*a).replace(char::is_whitespace, "");
                    t.contains("shell:true")
                });
                let vars: Vec<&Node> = args
                    .iter()
                    .filter(|a| !is_options(a) && self.has_var(**a))
                    .collect();
                (shell && !vars.is_empty())
                    .then(|| vars.iter().any(|a| self.reads_request(**a, owner)))
            }
        }
    }

    /// The sensitive names a call's arguments hold: variables, properties,
    /// methods, and the keys of values that aren't literals. Words inside
    /// strings don't count.
    fn names_in(&self, n: Node, out: &mut Vec<String>) {
        let kind = n.kind();
        let push = |out: &mut Vec<String>, s: &str| {
            out.push(
                s.trim_matches(|c| c == '$' || c == '\'' || c == '"')
                    .to_string(),
            )
        };
        match (self.lang, kind) {
            (_, k) if k.contains("comment") => {}
            (Lang::Php, "variable_name") => push(out, self.txt(n)),
            (Lang::Php, "name") => {
                if n.parent().is_some_and(|p| {
                    matches!(
                        p.kind(),
                        "member_access_expression"
                            | "nullsafe_member_access_expression"
                            | "member_call_expression"
                            | "nullsafe_member_call_expression"
                    )
                }) {
                    push(out, self.txt(n));
                }
            }
            (Lang::Php, "array_element_initializer") => {
                let parts = named(n);
                if parts.len() == 2
                    && matches!(parts[0].kind(), "string" | "encapsed_string")
                    && self.has_var(parts[1])
                {
                    push(out, self.txt(parts[0]));
                }
                if let Some(v) = parts.last() {
                    self.names_in(*v, out);
                }
            }
            (Lang::Php, "string") => {}
            (Lang::Go, "identifier" | "field_identifier") => push(out, self.txt(n)),
            (Lang::Go, "interpreted_string_literal" | "raw_string_literal") => {}
            (Lang::Php | Lang::Go | Lang::Rust | Lang::Python, _) => {
                for k in named(n) {
                    self.names_in(k, out);
                }
            }
            (_, "identifier" | "property_identifier" | "shorthand_property_identifier") => {
                push(out, self.txt(n))
            }
            (_, "pair") => {
                if let (Some(k), Some(v)) =
                    (n.child_by_field_name("key"), n.child_by_field_name("value"))
                {
                    if self.has_var(v) {
                        push(out, self.txt(k));
                    }
                    self.names_in(v, out);
                }
            }
            (_, "string") => {}
            _ => {
                for k in named(n) {
                    self.names_in(k, out);
                }
            }
        }
    }

    // ---- calls ----

    fn arg_nodes(&self, args: Option<Node<'t>>) -> Vec<Node<'t>> {
        let Some(a) = args else { return Vec::new() };
        match a.kind() {
            "arguments" | "argument_list" => named(a)
                .into_iter()
                .map(|k| {
                    // PHP wraps each in `argument`, with a `name:` when named.
                    if k.kind() == "argument" {
                        named(k).last().copied().unwrap_or(k)
                    } else {
                        k
                    }
                })
                .collect(),
            _ => vec![a],
        }
    }

    /// A call, attribute or decorator: what the catalog makes of it.
    /// `target_line`, when set, is a line of the symbol it is about.
    #[allow(clippy::too_many_arguments)]
    fn on_call(
        &mut self,
        form: Form,
        name: &str,
        recv: Option<String>,
        args_node: Option<Node<'t>>,
        args_raw: &str,
        at: u32,
        target_line: Option<u32>,
        node: Option<Node<'t>>,
    ) {
        if name.is_empty() {
            return;
        }
        let owner = self.owner_at(at);
        let target = target_line.map(|l| self.owner_at(l)).unwrap_or(owner);
        let args = self.arg_nodes(args_node);
        let norm = catalog::args_text(args_raw);
        let call = Call {
            form,
            name,
            recv: recv.as_deref(),
            args: &norm,
        };
        let push = |w: &mut Self, what: What| {
            w.events.push(Event {
                owner,
                target,
                line: at,
                what,
            })
        };
        if let Some(check) = self.cat.check(&call) {
            push(self, What::Check(check));
        }
        if self.cat.entry(&call).is_some() {
            if form == Form::Mark {
                let (verb, path) = mark_route(name, args_raw);
                push(
                    self,
                    What::Mark {
                        name: name.to_string(),
                        verb,
                        path,
                        first: first_string(args_raw),
                    },
                );
            } else if let Some(node) = node {
                self.register(name, &args, node, owner, at);
            }
        } else if form == Form::Method
            && let Some(node) = node
        {
            self.route_by_shape(name, recv.as_deref().unwrap_or(""), &args, node, owner, at);
        }
        if form == Form::Mark {
            return;
        }
        if let Some(sink) = self.cat.sink(&call, self.text)
            && let Some(request) = self.tainted(sink.taint, &args, owner)
        {
            let shown = match recv.as_deref().map(last_segment) {
                Some(r) if !r.is_empty() && sink.pattern.label().contains(['.', ':']) => {
                    format!("{r}.{name}")
                }
                _ => name.to_string(),
            };
            push(
                self,
                What::Sink {
                    kind: sink.kind,
                    name: shown,
                    request,
                },
            );
        }
        if self.cat.model(&call) {
            let shown = match recv.as_deref().map(last_segment) {
                Some(r) if !r.is_empty() => format!("{r}.{name}"),
                _ => name.to_string(),
            };
            push(self, What::Model(shown));
        }
        let log = self.cat.log(&call);
        let respond = !log && self.cat.response(&call);
        if log || respond {
            let mut names = Vec::new();
            for a in &args {
                self.names_in(*a, &mut names);
            }
            let mut seen = Vec::new();
            for n in names {
                let Some((label, strong)) = sensitive(&n) else {
                    continue;
                };
                // A response is meant to carry tokens (a login's), emails
                // and phone numbers: only secrets count there.
                if respond && (!strong || matches!(label, "token" | "authorization")) {
                    continue;
                }
                if !seen.contains(&label) {
                    seen.push(label);
                    let what = if log {
                        What::Log { label, strong }
                    } else {
                        What::Respond { label, strong }
                    };
                    push(self, what);
                }
            }
        }
    }

    /// A route registration the catalog names: `Route::get('/x', …)`.
    fn register(&mut self, name: &str, args: &[Node<'t>], node: Node<'t>, owner: usize, at: u32) {
        let path = args.first().and_then(|a| self.string_value(*a));
        let verb = name.to_ascii_uppercase();
        let route = match path {
            Some(p) if HTTP_VERBS.contains(&verb.as_str()) || verb == "ANY" => {
                format!("{verb} {}", join_path(None, Some(&p)))
            }
            Some(p) => join_path(None, Some(&p)),
            None => return,
        };
        let mut handlers = Vec::new();
        for a in args.iter().skip(1) {
            match a.kind() {
                // `[UserController::class, 'show']`, `'UserController@show'`
                "array_creation_expression" => {
                    let last = named(*a)
                        .last()
                        .map(|e| named(*e).last().copied().unwrap_or(*e));
                    if let Some(m) = last.and_then(|e| self.string_value(e)) {
                        handlers.push(m);
                    }
                }
                "string" | "encapsed_string" => {
                    if let Some(s) = self.string_value(*a) {
                        handlers.push(s.rsplit('@').next().unwrap_or(&s).to_string());
                    }
                }
                "class_constant_access_expression" => handlers.push("__invoke".into()),
                _ => {}
            }
        }
        let checks = self.checks_around(node);
        self.routes.push(RouteReg {
            owner,
            line: at,
            route,
            handlers,
            checks,
            is_use: false,
        });
    }

    /// Checks chained on a registration (`->middleware('auth')`) or on a
    /// group it is in.
    fn checks_around(&self, node: Node<'t>) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = node;
        for _ in 0..24 {
            let Some(p) = cur.parent() else { break };
            if matches!(
                p.kind(),
                "member_call_expression" | "scoped_call_expression" | "call_expression"
            ) {
                // Chained after us, or a group whose callback we are in:
                // the call's own text, without the arguments we sit in.
                let own = match p.child_by_field_name("arguments") {
                    Some(a)
                        if a.start_byte() <= cur.start_byte() && cur.end_byte() <= a.end_byte() =>
                    {
                        let start = p.start_byte();
                        &self.text[start..a.start_byte()]
                    }
                    _ => {
                        let start = cur.end_byte().min(p.end_byte());
                        &self.text[start..p.end_byte()]
                    }
                };
                if let Some(c) = self.cat.check_in(own) {
                    out.push(c);
                }
            }
            cur = p;
        }
        out
    }

    /// The text of a string literal.
    fn string_value(&self, n: Node) -> Option<String> {
        match n.kind() {
            "string" | "encapsed_string" | "interpreted_string_literal" | "raw_string_literal" => {
                Some(
                    self.txt(n)
                        .trim_matches(|c| c == '\'' || c == '"' || c == '`')
                        .to_string(),
                )
            }
            "template_string" if !named(n).iter().any(|k| k.kind() == "template_substitution") => {
                Some(self.txt(n).trim_matches('`').to_string())
            }
            _ => None,
        }
    }

    /// The checks among a registration's arguments: `requireAuth`,
    /// `authMiddleware(h)`.
    fn arg_checks(&self, args: &[Node<'t>]) -> (Vec<String>, Vec<String>) {
        let mut checks = Vec::new();
        let mut handlers = Vec::new();
        for a in args {
            let (name, inner) = match a.kind() {
                "identifier" => (self.txt(*a).to_string(), None),
                "member_expression" | "selector_expression" => {
                    (last_segment(self.txt(*a)).to_string(), None)
                }
                "call_expression" => (
                    a.child_by_field_name("function")
                        .map(|f| last_segment(self.txt(f)).to_string())
                        .unwrap_or_default(),
                    a.child_by_field_name("arguments"),
                ),
                _ => continue,
            };
            let call = Call {
                form: Form::Function,
                name: &name,
                recv: None,
                args: "",
            };
            match self.cat.check(&call) {
                Some(c) => checks.push(c),
                None if inner.is_none() => handlers.push(name),
                None => {}
            }
            if let Some(inner) = inner {
                let (c, h) = self.arg_checks(&self.arg_nodes(Some(inner)));
                checks.extend(c);
                handlers.extend(h);
            }
        }
        (checks, handlers)
    }

    /// Express, Koa, Fastify and Go routers: `router.get('/x', h)`,
    /// `mux.HandleFunc("/x", h)`, `r.GET("/x", h)`; and their `use`.
    fn route_by_shape(
        &mut self,
        name: &str,
        recv: &str,
        args: &[Node<'t>],
        node: Node<'t>,
        owner: usize,
        at: u32,
    ) {
        let lower = name.to_ascii_lowercase();
        if lower == "use" {
            let (checks, _) = self.arg_checks(args);
            if !checks.is_empty() {
                self.routes.push(RouteReg {
                    owner,
                    line: at,
                    route: String::new(),
                    handlers: Vec::new(),
                    checks,
                    is_use: true,
                });
            }
            return;
        }
        if args.len() < 2 {
            return;
        }
        let Some(path) = self.string_value(args[0]) else {
            return;
        };
        let inline_kinds = [
            "arrow_function",
            "function_expression",
            "function",
            "func_literal",
        ];
        let route = match self.lang {
            Lang::Go => {
                let verb = match name {
                    "HandleFunc" | "Handle" => None,
                    "Any" => Some("ANY".to_string()),
                    n if HTTP_VERBS.contains(&n.to_ascii_uppercase().as_str()) => {
                        Some(n.to_ascii_uppercase())
                    }
                    _ => return,
                };
                let starts_ok = path.starts_with('/')
                    || path
                        .split_once(' ')
                        .is_some_and(|(v, p)| HTTP_VERBS.contains(&v) && p.starts_with('/'));
                if !starts_ok {
                    return;
                }
                // gorilla/mux: `.Methods("POST")` chained on it.
                let chained = node
                    .parent()
                    .filter(|p| p.kind() == "selector_expression")
                    .and_then(|p| p.parent())
                    .filter(|c| c.kind() == "call_expression")
                    .filter(|c| {
                        c.child_by_field_name("function")
                            .is_some_and(|f| self.txt(f).ends_with(".Methods"))
                    })
                    .and_then(|c| c.child_by_field_name("arguments"))
                    .and_then(|a| first_string(self.txt(a)))
                    .map(|v| v.to_ascii_uppercase());
                route_words(verb.or(chained).as_deref(), &path)
            }
            Lang::JavaScript | Lang::TypeScript | Lang::Tsx => {
                let verb = match lower.as_str() {
                    "get" | "post" | "put" | "patch" | "delete" | "options" | "head" => {
                        lower.to_ascii_uppercase()
                    }
                    "del" => "DELETE".into(),
                    "all" => "ALL".into(),
                    _ => return,
                };
                if !(path.starts_with('/') || path == "*") {
                    return;
                }
                let inline = args[1..].iter().any(|a| inline_kinds.contains(&a.kind()));
                if !inline && !ROUTERISH.is_match(last_segment(recv)) {
                    return;
                }
                route_words(Some(&verb), &path)
            }
            _ => return,
        };
        let (checks, handlers) = self.arg_checks(&args[1..]);
        let mut checks = checks;
        checks.extend(self.checks_around(node));
        // chi: `r.With(RequireAuth).Get("/x", h)`.
        let on = node
            .child_by_field_name("function")
            .and_then(|f| {
                f.child_by_field_name("object")
                    .or_else(|| f.child_by_field_name("operand"))
            })
            .map(|o| self.txt(o))
            .unwrap_or("");
        if let Some(c) = self.cat.check_in(on) {
            checks.push(c);
        }
        self.routes.push(RouteReg {
            owner,
            line: at,
            route,
            handlers,
            checks,
            is_use: false,
        });
    }

    // ---- declarations ----

    /// A function or method's declaration: its signature, visibility,
    /// handler parameters.
    fn def(&mut self, n: Node<'t>) {
        let i = self.owner_at(line(n));
        let Some(sym) = self.syms.get(i) else { return };
        if !matches!(sym.kind, UnitKind::Function | UnitKind::Method) || !sym.sig.is_empty() {
            return;
        }
        let mut start = n.start_byte();
        for k in kids(n) {
            if matches!(k.kind(), "attribute_list" | "decorator") || k.kind().contains("comment") {
                start = k.end_byte();
            } else {
                break;
            }
        }
        let end = n
            .child_by_field_name("body")
            .map(|b| b.start_byte())
            .unwrap_or(n.end_byte());
        let sig: String = self
            .text
            .get(start..end.max(start))
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let public = match self.lang {
            Lang::Php => kids(n)
                .into_iter()
                .find(|k| k.kind() == "visibility_modifier")
                .is_none_or(|v| self.txt(v) == "public"),
            _ => true,
        };
        let handler = self.handler_params(n);
        let s = &mut self.syms[i];
        s.sig = sig;
        s.public = public;
        s.handler_sig |= handler;
    }

    /// Its parameters are a web handler's: Go's `(w http.ResponseWriter, r
    /// *http.Request)`, `*gin.Context`, `echo.Context`; Express's `(req,
    /// res)`.
    fn handler_params(&self, n: Node) -> bool {
        let Some(params) = n.child_by_field_name("parameters") else {
            return false;
        };
        match self.lang {
            Lang::Go => {
                let types: Vec<String> = named(params)
                    .into_iter()
                    .filter_map(|p| p.child_by_field_name("type"))
                    .map(|t| self.txt(t).replace(char::is_whitespace, ""))
                    .collect();
                let has = |t: &str| types.iter().any(|x| x == t);
                (has("http.ResponseWriter") && has("*http.Request"))
                    || has("*gin.Context")
                    || has("echo.Context")
                    || has("*fiber.Ctx")
            }
            Lang::JavaScript | Lang::TypeScript | Lang::Tsx => {
                let ps = named(params);
                let name = |p: &Node| {
                    p.child_by_field_name("pattern")
                        .map(|x| self.txt(x))
                        .unwrap_or_else(|| self.txt(*p))
                        .to_string()
                };
                let text = self.txt(params);
                (ps.len() >= 2
                    && matches!(name(&ps[0]).as_str(), "req" | "request")
                    && matches!(name(&ps[1]).as_str(), "res" | "response" | "reply"))
                    || text.contains("NextApiRequest")
                    || text.contains("FastifyRequest")
            }
            _ => false,
        }
    }

    // ---- per language ----

    fn php(&mut self, n: Node<'t>) {
        let at = line(n);
        match n.kind() {
            "function_call_expression" => {
                let Some(f) = n.child_by_field_name("function") else {
                    return;
                };
                if !matches!(f.kind(), "name" | "qualified_name") {
                    return;
                }
                let name = last_segment(self.txt(f)).to_string();
                let args = n.child_by_field_name("arguments");
                let raw = args.map(|a| self.txt(a)).unwrap_or("");
                self.on_call(Form::Function, &name, None, args, raw, at, None, Some(n));
            }
            "member_call_expression"
            | "nullsafe_member_call_expression"
            | "scoped_call_expression" => {
                let Some(name) = n.child_by_field_name("name") else {
                    return;
                };
                let recv_node = n
                    .child_by_field_name("object")
                    .or_else(|| n.child_by_field_name("scope"));
                let recv = recv_node.map(|r| catalog::chain(self.txt(r)));
                let args = n.child_by_field_name("arguments");
                let raw = args.map(|a| self.txt(a)).unwrap_or("");
                let name = self.txt(name).to_string();
                self.on_call(Form::Method, &name, recv, args, raw, at, None, Some(n));
            }
            "object_creation_expression" => {
                let parts = named(n);
                let Some(class) = parts
                    .iter()
                    .find(|k| matches!(k.kind(), "name" | "qualified_name"))
                else {
                    return;
                };
                let name = last_segment(self.txt(*class)).to_string();
                let args = parts.iter().find(|k| k.kind() == "arguments").copied();
                let raw = args.map(|a| self.txt(a)).unwrap_or("");
                self.on_call(Form::New, &name, None, args, raw, at, None, Some(n));
            }
            "attribute" => {
                let decl = n.parent().and_then(|g| g.parent()).and_then(|l| l.parent());
                if decl.is_some_and(|d| {
                    matches!(
                        d.kind(),
                        "simple_parameter"
                            | "property_promotion_parameter"
                            | "variadic_parameter"
                            | "property_declaration"
                            | "const_declaration"
                            | "enum_case"
                    )
                }) {
                    return;
                }
                let Some(name) = named(n)
                    .into_iter()
                    .find(|k| matches!(k.kind(), "name" | "qualified_name"))
                else {
                    return;
                };
                let name = last_segment(self.txt(name)).to_string();
                let args = n.child_by_field_name("parameters");
                let raw = args.map(|a| self.txt(a)).unwrap_or("");
                let target = decl.map(line);
                self.on_call(Form::Mark, &name, None, args, raw, at, target, None);
            }
            "include_expression"
            | "include_once_expression"
            | "require_expression"
            | "require_once_expression" => {
                if let Some(e) = named(n).first()
                    && self.has_var(*e)
                {
                    let owner = self.owner_at(at);
                    let request = self.reads_request(*e, owner);
                    let name = self.txt(n).split_whitespace().next().unwrap_or("include");
                    self.events.push(Event {
                        owner,
                        target: owner,
                        line: at,
                        what: What::Sink {
                            kind: SinkKind::FilePath,
                            name: name.to_string(),
                            request,
                        },
                    });
                }
            }
            "shell_command_expression" => {
                if self.has_var(n) {
                    let owner = self.owner_at(at);
                    let request = self.reads_request(n, owner);
                    self.events.push(Event {
                        owner,
                        target: owner,
                        line: at,
                        what: What::Sink {
                            kind: SinkKind::Shell,
                            name: "backticks".into(),
                            request,
                        },
                    });
                }
            }
            "comment" => self.doc_block(n),
            "method_declaration" | "function_definition" => self.def(n),
            "class_declaration" => {
                let i = self.owner_at(at);
                if kids(n)
                    .iter()
                    .any(|k| k.kind() == "abstract_modifier" || self.txt(*k) == "abstract")
                {
                    self.syms[i].is_abstract = true;
                }
            }
            _ => {}
        }
    }

    /// A PHP doc block's annotations (`@Route("/x")`, `@IsGranted("X")`),
    /// about the declaration that follows it.
    fn doc_block(&mut self, n: Node<'t>) {
        let text = self.txt(n);
        if !text.starts_with("/**") {
            return;
        }
        let mut next = n.next_named_sibling();
        while let Some(s) = next {
            if s.kind() == "comment" {
                next = s.next_named_sibling();
            } else {
                break;
            }
        }
        let target = next.map(line);
        let start = line(n);
        for c in ANNOTATION.captures_iter(text) {
            let whole = c.get(0).unwrap();
            let at = start + text[..whole.start()].matches('\n').count() as u32;
            let name = last_segment(&c[1]).to_string();
            let raw = c.get(2).map(|m| m.as_str()).unwrap_or("");
            self.on_call(Form::Mark, &name, None, None, raw, at, target, None);
        }
    }

    fn ts(&mut self, n: Node<'t>) {
        let at = line(n);
        match n.kind() {
            "call_expression" | "new_expression" => {
                let field = if n.kind() == "call_expression" {
                    "function"
                } else {
                    "constructor"
                };
                let Some(f) = n.child_by_field_name(field) else {
                    return;
                };
                let args = n.child_by_field_name("arguments");
                let raw = args.map(|a| self.txt(a)).unwrap_or("");
                let new = n.kind() == "new_expression";
                match f.kind() {
                    "identifier" => {
                        let form = if new { Form::New } else { Form::Function };
                        let name = self.txt(f).to_string();
                        self.on_call(form, &name, None, args, raw, at, None, Some(n));
                    }
                    "member_expression" => {
                        let (Some(obj), Some(prop)) = (
                            f.child_by_field_name("object"),
                            f.child_by_field_name("property"),
                        ) else {
                            return;
                        };
                        let form = if new { Form::New } else { Form::Method };
                        let name = self.txt(prop).to_string();
                        let recv = Some(catalog::chain(self.txt(obj)));
                        self.on_call(form, &name, recv, args, raw, at, None, Some(n));
                    }
                    _ => {}
                }
            }
            "decorator" => {
                let Some(parent) = n.parent() else { return };
                let target = match parent.kind() {
                    "required_parameter" | "optional_parameter" | "formal_parameters" => return,
                    "class_body" => {
                        let mut next = n.next_named_sibling();
                        while let Some(s) = next {
                            if s.kind() == "decorator" || s.kind().contains("comment") {
                                next = s.next_named_sibling();
                            } else {
                                break;
                            }
                        }
                        next.map(line)
                    }
                    _ => Some(line(parent)),
                };
                let Some(inner) = named(n).into_iter().next() else {
                    return;
                };
                let (f, args) = match inner.kind() {
                    "call_expression" => (
                        inner.child_by_field_name("function"),
                        inner.child_by_field_name("arguments"),
                    ),
                    _ => (Some(inner), None),
                };
                let Some(f) = f else { return };
                let name = last_segment(self.txt(f)).to_string();
                let raw = args.map(|a| self.txt(a)).unwrap_or("");
                self.on_call(Form::Mark, &name, None, args, raw, at, target, None);
            }
            "assignment_expression" => {
                let (Some(l), Some(r)) = (
                    n.child_by_field_name("left"),
                    n.child_by_field_name("right"),
                ) else {
                    return;
                };
                let prop = l
                    .child_by_field_name("property")
                    .map(|p| self.txt(p))
                    .unwrap_or("");
                if l.kind() == "member_expression"
                    && matches!(prop, "innerHTML" | "outerHTML")
                    && self.has_var(r)
                {
                    let owner = self.owner_at(at);
                    let request = self.reads_request(r, owner);
                    self.events.push(Event {
                        owner,
                        target: owner,
                        line: at,
                        what: What::Sink {
                            kind: SinkKind::Template,
                            name: prop.to_string(),
                            request,
                        },
                    });
                }
            }
            "jsx_attribute" => {
                let parts = named(n);
                if parts
                    .first()
                    .is_none_or(|p| self.txt(*p) != "dangerouslySetInnerHTML")
                {
                    return;
                }
                let Some(value) = parts.get(1) else { return };
                if self.has_var(*value) {
                    let owner = self.owner_at(at);
                    let request = self.reads_request(*value, owner);
                    self.events.push(Event {
                        owner,
                        target: owner,
                        line: at,
                        what: What::Sink {
                            kind: SinkKind::Template,
                            name: "dangerouslySetInnerHTML".into(),
                            request,
                        },
                    });
                }
            }
            "expression_statement" => {
                let parts = named(n);
                let directive = parts.len() == 1
                    && parts[0].kind() == "string"
                    && matches!(self.txt(parts[0]), "'use server'" | "\"use server\"");
                if !directive {
                    return;
                }
                match n.parent().map(|p| p.kind()) {
                    Some("program") => self.file_use_server = true,
                    Some("statement_block") => {
                        let i = self.owner_at(at);
                        self.syms[i].use_server = true;
                    }
                    _ => {}
                }
            }
            "function_declaration"
            | "generator_function_declaration"
            | "method_definition"
            | "arrow_function"
            | "function_expression"
            | "function" => self.def(n),
            "arguments" => self.arg_refs(n),
            "abstract_class_declaration" => {
                let i = self.owner_at(at);
                self.syms[i].is_abstract = true;
            }
            _ => {}
        }
    }

    /// Checks passed by reference: `router.get('/x', requireAuth, h)`,
    /// `r.Use(RequireAuth)`.
    fn arg_refs(&mut self, n: Node<'t>) {
        for a in named(n) {
            let name = match a.kind() {
                "identifier" => self.txt(a),
                "member_expression" | "selector_expression" => last_segment(self.txt(a)),
                _ => continue,
            };
            let call = Call {
                form: Form::Function,
                name,
                recv: None,
                args: "",
            };
            if let Some(c) = self.cat.check(&call) {
                let at = line(a);
                let owner = self.owner_at(at);
                self.events.push(Event {
                    owner,
                    target: owner,
                    line: at,
                    what: What::Check(c),
                });
            }
        }
    }

    fn go(&mut self, n: Node<'t>) {
        let at = line(n);
        match n.kind() {
            "call_expression" => {
                let Some(f) = n.child_by_field_name("function") else {
                    return;
                };
                let args = n.child_by_field_name("arguments");
                let raw = args.map(|a| self.txt(a)).unwrap_or("");
                match f.kind() {
                    "identifier" => {
                        let name = self.txt(f).to_string();
                        self.on_call(Form::Function, &name, None, args, raw, at, None, Some(n));
                    }
                    "selector_expression" => {
                        let (Some(op), Some(field)) = (
                            f.child_by_field_name("operand"),
                            f.child_by_field_name("field"),
                        ) else {
                            return;
                        };
                        let name = self.txt(field).to_string();
                        let recv = Some(catalog::chain(self.txt(op)));
                        self.on_call(Form::Method, &name, recv, args, raw, at, None, Some(n));
                    }
                    _ => {}
                }
            }
            "function_declaration" | "method_declaration" => self.def(n),
            "func_literal" => {
                if self.handler_params(n) {
                    let i = self.owner_at(at);
                    self.syms[i].inline_handler = true;
                }
            }
            "argument_list" => self.arg_refs(n),
            _ => {}
        }
    }

    // ---- entry points ----

    fn finish(&mut self, path: &Path) {
        let n = self.syms.len();
        // Per symbol: (mark, verb, path, first string argument).
        type Marked = (String, Option<String>, Option<String>, Option<String>);
        let mut marks: Vec<Vec<Marked>> = vec![Vec::new(); n];
        for e in &self.events {
            if let What::Mark {
                name,
                verb,
                path,
                first,
            } = &e.what
                && e.target < n
            {
                marks[e.target].push((name.clone(), verb.clone(), path.clone(), first.clone()));
            }
        }
        let class_prefix = |i: Option<usize>| -> Option<String> {
            let m = &marks[i?];
            m.iter()
                .find(|(name, ..)| {
                    matches!(name.to_ascii_lowercase().as_str(), "route" | "controller")
                })
                .and_then(|(_, _, p, _)| p.clone())
        };
        let next = next_route(path);
        let mut entries: Vec<Option<Entry>> = vec![None; n];
        for i in 0..n {
            let s = &self.syms[i];
            if !matches!(s.kind, UnitKind::Function | UnitKind::Method) {
                continue;
            }
            let short = s.name.as_str();
            let parent = s.parent;
            let parent_name = parent
                .map(|p| self.syms[p].name.clone())
                .unwrap_or_default();
            // Its own attribute or decorator.
            if let Some((mname, verb, mpath, first)) = marks[i].first() {
                let how = entry_how(mname);
                let name = match how {
                    "route" => route_words(
                        verb.as_deref(),
                        &join_path(class_prefix(parent).as_deref(), mpath.as_deref()),
                    ),
                    "command" => first.clone().unwrap_or_else(|| s.path.clone()),
                    _ => s.path.clone(),
                };
                entries[i] = Some(Entry { name, how });
                continue;
            }
            // Its class's: a message handler's `__invoke`, a command's
            // `execute`, an invokable controller.
            if let Some(p) = parent {
                for (mname, verb, mpath, first) in &marks[p] {
                    let how = entry_how(mname);
                    let invoke = short == "__invoke";
                    let e = match how {
                        "message handler" if invoke || short.starts_with("handle") => Some(Entry {
                            name: parent_name.clone(),
                            how,
                        }),
                        "command" if invoke || short == "execute" => Some(Entry {
                            name: first.clone().unwrap_or_else(|| parent_name.clone()),
                            how,
                        }),
                        "route" if invoke => Some(Entry {
                            name: route_words(verb.as_deref(), &join_path(None, mpath.as_deref())),
                            how,
                        }),
                        "controller action" if s.public && !short.starts_with("__") => {
                            Some(Entry {
                                name: s.path.clone(),
                                how,
                            })
                        }
                        _ => None,
                    };
                    if e.is_some() {
                        entries[i] = e;
                        break;
                    }
                }
                if entries[i].is_some() {
                    continue;
                }
            }
            if self.lang == Lang::Php
                && let Some(p) = parent
            {
                // A controller whose routes are elsewhere (Laravel): its
                // public actions.
                let class = &self.syms[p];
                let routed_here = (0..n).any(|j| {
                    self.syms[j].parent == Some(p)
                        && marks[j].iter().any(|(m, ..)| entry_how(m) == "route")
                });
                if class.name.ends_with("Controller")
                    && !class.is_abstract
                    && !routed_here
                    && s.public
                    && (!short.starts_with("__") || short == "__invoke")
                {
                    entries[i] = Some(Entry {
                        name: s.path.clone(),
                        how: "controller action",
                    });
                    continue;
                }
            }
            // A handler factory: `func (s *S) users() http.HandlerFunc {
            // return func(w, r) {…} }`. One that registers its handlers
            // inline has its routes instead.
            let registers = self.routes.iter().any(|r| r.owner == i && !r.is_use);
            if s.handler_sig || (s.inline_handler && !registers) {
                let route = self
                    .routes
                    .iter()
                    .find(|r| !r.is_use && r.handlers.iter().any(|h| h == short))
                    .map(|r| r.route.clone());
                entries[i] = Some(Entry {
                    name: route.unwrap_or_else(|| s.path.clone()),
                    how: "HTTP handler",
                });
                continue;
            }
            let top = parent.is_none();
            if let Some((route, pages)) = &next
                && top
            {
                let upper = short.to_ascii_uppercase();
                let line_text = self
                    .text
                    .lines()
                    .nth(s.range.start as usize - 1)
                    .unwrap_or("");
                if !pages && HTTP_VERBS.contains(&upper.as_str()) && upper == short {
                    entries[i] = Some(Entry {
                        name: format!("{short} {route}"),
                        how: "route",
                    });
                    continue;
                }
                if *pages && (short == "handler" || line_text.contains("export default")) {
                    entries[i] = Some(Entry {
                        name: route.clone(),
                        how: "route",
                    });
                    continue;
                }
            }
            let exported = self
                .text
                .lines()
                .nth(s.range.start as usize - 1)
                .is_some_and(|l| l.trim_start().starts_with("export"));
            if s.use_server || (self.file_use_server && top && exported) {
                entries[i] = Some(Entry {
                    name: s.path.clone(),
                    how: "server action",
                });
                continue;
            }
            // Named as a handler by a registration in this file.
            if let Some(r) = self
                .routes
                .iter()
                .find(|r| !r.is_use && r.handlers.iter().any(|h| h == short))
            {
                entries[i] = Some(Entry {
                    name: r.route.clone(),
                    how: "route",
                });
            }
        }
        for (s, e) in self.syms.iter_mut().zip(entries) {
            s.entry = e;
        }
        for r in &self.routes {
            if !r.is_use
                && let Some(s) = self.syms.get_mut(r.owner)
                && s.registers.is_none()
            {
                s.registers = Some(r.route.clone());
            }
        }
    }
}

/// Go packages whose calls read no variable of their own.
const GO_PACKAGES: [&str; 30] = [
    "fmt", "strings", "strconv", "filepath", "path", "os", "time", "url", "http", "json", "bytes",
    "errors", "sort", "math", "base64", "hex", "sha256", "md5", "rand", "regexp", "template",
    "html", "io", "ioutil", "bufio", "log", "slog", "context", "exec", "sql",
];

/// JavaScript globals and modules whose calls read no variable of their own.
const JS_GLOBALS: [&str; 18] = [
    "path", "Math", "JSON", "Object", "Number", "String", "Array", "Date", "Buffer", "os", "url",
    "crypto", "console", "Promise", "Reflect", "Symbol", "Intl", "process",
];

/// A name that is a constant, not a variable: one of `safe`, or written
/// in capitals (`BASE_URL`).
fn is_constant_name(name: &str, safe: &[&str]) -> bool {
    safe.contains(&name)
        || (name.len() > 1
            && name.chars().any(|c| c.is_ascii_uppercase())
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::catalog::for_lang;
    use crate::security::config::SecurityConfig;

    fn scan_of(lang: Lang, path: &str, src: &str) -> Scan {
        let cat = for_lang(lang, &SecurityConfig::default());
        scan(lang, Path::new(path), src, &cat).unwrap()
    }

    fn whats(s: &Scan, sym: &str) -> Vec<What> {
        let i = s.sym(sym).unwrap();
        s.events
            .iter()
            .filter(|e| e.owner == i)
            .map(|e| e.what.clone())
            .collect()
    }

    fn sinks(s: &Scan, sym: &str) -> Vec<(SinkKind, bool)> {
        whats(s, sym)
            .into_iter()
            .filter_map(|w| match w {
                What::Sink { kind, request, .. } => Some((kind, request)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn words_split_identifiers() {
        assert_eq!(words("apiKey"), ["api", "key"]);
        assert_eq!(words("DB_PASSWORD"), ["db", "password"]);
        assert_eq!(words("getHTTPToken"), ["get", "http", "token"]);
        assert_eq!(sensitive("userPassword"), Some(("password", true)));
        assert_eq!(sensitive("passwordResetUrl"), None);
        assert_eq!(sensitive("tokenCount"), None);
        assert_eq!(sensitive("max_tokens"), None);
        assert_eq!(sensitive("email"), Some(("email", false)));
        assert_eq!(sensitive("passwordHasher"), None);
    }

    #[test]
    fn php_checks_marks_and_sinks() {
        let src = r#"<?php
#[Route('/brands')]
class BrandController extends AbstractController
{
    /** @IsGranted("ROLE_ADMIN") */
    #[Route('/{id}/export', methods: ['POST'])]
    public function export(Request $request, $id)
    {
        $this->denyAccessUnlessGranted('EXPORT', $id);
        $sql = "SELECT * FROM brand WHERE id = " . $request->get('id');
        $this->conn->executeQuery($sql);
        $this->conn->executeQuery('SELECT 1');
        shell_exec('ls ' . $dir);
        file_get_contents(__DIR__ . '/x.json');
        $this->logger->info('exported', ['password' => $plain]);
        return new JsonResponse(['ok' => true]);
    }

    private function helper() {}
}
"#;
        let s = scan_of(Lang::Php, "src/BrandController.php", src);
        let i = s.sym("BrandController::export").unwrap();
        let e = s.syms[i].entry.as_ref().unwrap();
        assert_eq!(e.how, "route");
        assert_eq!(e.name, "POST /brands/{id}/export");
        let checks = s.checks_on(i);
        assert!(
            checks.contains(&"denyAccessUnlessGranted".to_string()),
            "{checks:?}"
        );
        assert!(checks.contains(&"IsGranted".to_string()), "{checks:?}");
        assert_eq!(
            sinks(&s, "BrandController::export"),
            vec![(SinkKind::Sql, true), (SinkKind::Shell, false)]
        );
        assert!(whats(&s, "BrandController::export").contains(&What::Log {
            label: "password",
            strong: true
        }));
        // A private helper of a routed controller is no entry point.
        let h = s.sym("BrandController::helper").unwrap();
        assert!(s.syms[h].entry.is_none());
        assert!(!s.syms[h].public);
    }

    #[test]
    fn laravel_routes_and_controllers() {
        let routes = r#"<?php
Route::get('/users/{id}', [UserController::class, 'show'])->middleware('auth');
Route::middleware(['auth:sanctum'])->group(function () {
    Route::post('/users', [UserController::class, 'store']);
});
Route::get('/health', fn () => 'ok');
"#;
        let s = scan_of(Lang::Php, "routes/web.php", routes);
        let got: Vec<(&str, bool)> = s
            .routes
            .iter()
            .map(|r| (r.route.as_str(), !r.checks.is_empty()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("GET /users/{id}", true),
                ("POST /users", true),
                ("GET /health", false)
            ]
        );
        assert_eq!(s.routes[0].handlers, ["show"]);
        let ctrl = "<?php\nclass UserController extends Controller {\n  public function show($id) { return User::find($id); }\n  protected function hidden() {}\n}\n";
        let c = scan_of(Lang::Php, "app/Http/Controllers/UserController.php", ctrl);
        let show = c.sym("UserController::show").unwrap();
        assert_eq!(
            c.syms[show].entry.as_ref().unwrap().how,
            "controller action"
        );
        assert!(
            c.syms[c.sym("UserController::hidden").unwrap()]
                .entry
                .is_none()
        );
    }

    #[test]
    fn typescript_routes_decorators_and_sinks() {
        let src = r#"import { exec } from 'child_process';
@Controller('users')
export class UsersController {
  @Get(':id')
  @UseGuards(AuthGuard)
  find(@Query() q: string) {
    return this.db.query(`SELECT * FROM users WHERE id = ${q}`);
  }
}
router.get('/admin', requireAuth, (req, res) => {
  exec(req.query.cmd);
  res.json({ password: user.password });
});
export function render(el: HTMLElement, html: string) {
  el.innerHTML = html;
  el.innerHTML = '<b>static</b>';
}
"#;
        let s = scan_of(Lang::TypeScript, "src/users.ts", src);
        let find = s.sym("UsersController::find").unwrap();
        assert_eq!(s.syms[find].entry.as_ref().unwrap().name, "GET /users/:id");
        assert_eq!(s.checks_on(find), ["UseGuards"]);
        // The decorators are in the class's text.
        let class = s.sym("UsersController").unwrap();
        assert!(
            s.events
                .iter()
                .any(|e| e.owner == class && e.target == find)
        );
        assert_eq!(
            sinks(&s, "UsersController::find"),
            vec![(SinkKind::Sql, false)]
        );
        let module = s.sym("<module>").unwrap();
        assert_eq!(s.routes[0].route, "GET /admin");
        assert_eq!(s.routes[0].checks, ["requireAuth"]);
        assert_eq!(s.syms[module].registers.as_deref(), Some("GET /admin"));
        assert_eq!(sinks(&s, "<module>"), vec![(SinkKind::Shell, true)]);
        assert!(whats(&s, "<module>").contains(&What::Respond {
            label: "password",
            strong: true
        }));
        assert_eq!(sinks(&s, "render"), vec![(SinkKind::Template, false)]);
    }

    #[test]
    fn nextjs_routes_and_server_actions() {
        let route = "export async function POST(req: Request) { return fetch(req.url); }\nfunction helper() {}\n";
        let s = scan_of(
            Lang::TypeScript,
            "src/app/api/(admin)/users/route.ts",
            route,
        );
        let post = s.sym("POST").unwrap();
        assert_eq!(s.syms[post].entry.as_ref().unwrap().name, "POST /api/users");
        assert!(s.syms[s.sym("helper").unwrap()].entry.is_none());
        let actions = "'use server';\nexport async function save(data: FormData) {}\nasync function local() {}\n";
        let a = scan_of(Lang::TypeScript, "src/app/actions.ts", actions);
        assert_eq!(
            a.syms[a.sym("save").unwrap()].entry.as_ref().unwrap().how,
            "server action"
        );
        assert!(a.syms[a.sym("local").unwrap()].entry.is_none());
    }

    #[test]
    fn go_handlers_and_sinks() {
        let src = r#"package api

func Routes(mux *http.ServeMux) {
	mux.HandleFunc("/users", handleUsers)
	mux.Handle("/admin", RequireAuth(adminHandler))
}

func handleUsers(w http.ResponseWriter, r *http.Request) {
	id := r.URL.Query().Get("id")
	q := fmt.Sprintf("SELECT * FROM users WHERE id = %s", id)
	db.Query(q)
	db.Query("SELECT 1 WHERE id = $1", id)
	exec.Command("sh", "-c", r.FormValue("cmd"))
	log.Printf("token %s", token)
}

func helper(x int) int { return x }
"#;
        let s = scan_of(Lang::Go, "api/routes.go", src);
        let h = s.sym("handleUsers").unwrap();
        let e = s.syms[h].entry.as_ref().unwrap();
        assert_eq!((e.how, e.name.as_str()), ("HTTP handler", "/users"));
        assert!(s.syms[s.sym("helper").unwrap()].entry.is_none());
        assert_eq!(
            sinks(&s, "handleUsers"),
            vec![(SinkKind::Sql, true), (SinkKind::Shell, true)]
        );
        assert!(whats(&s, "handleUsers").contains(&What::Log {
            label: "token",
            strong: true
        }));
        let admin = s.routes.iter().find(|r| r.route == "/admin").unwrap();
        assert_eq!(admin.checks, ["RequireAuth"]);
        assert_eq!(admin.handlers, ["adminHandler"]);
    }

    #[test]
    fn literals_are_no_sinks() {
        let php = "<?php\nfunction f() {\n  $db->query('SELECT 1');\n  $db->where('status', $s);\n  include __DIR__ . '/x.php';\n  exec('ls');\n}\n";
        assert!(sinks(&scan_of(Lang::Php, "a.php", php), "f").is_empty());
        let ts = "function f() {\n  db.query('SELECT $1', [id]);\n  fetch(`${BASE_URL}/x`);\n  fetch('/api');\n}\n";
        assert!(sinks(&scan_of(Lang::TypeScript, "a.ts", ts), "f").is_empty());
        let go = "package p\nfunc f() {\n\tdb.Query(\"SELECT 1\")\n\tos.Open(\"config.yaml\")\n}\n";
        assert!(sinks(&scan_of(Lang::Go, "a.go", go), "f").is_empty());
    }
}
