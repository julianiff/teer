//! The built-in catalog, per language: authorization checks, entry points
//! and sinks by class, matched by name in the syntax tree; plus the user's
//! additions from `security.toml` ([`super::config::Extra`]).
//!
//! PHP, TypeScript/JavaScript and Go have one. Rust and Python have none
//! yet: their units still get the checks that read lines (secrets, config,
//! crypto).
//!
//! A name in the catalog is a small pattern, as `security.toml` writes it
//! too:
//!
//! | pattern | matches |
//! | --- | --- |
//! | `isGranted` | a call, attribute or decorator of that name, any receiver |
//! | `Gate::allows`, `fs.readFile`, `$client->request` | a call on a receiver whose last part has that name |
//! | `*client->request` | a call on a receiver whose last part ends in `client` |
//! | `->query`, `.query` | a method call of that name, any receiver |
//! | `exec()` | a plain function call, no receiver |
//! | `new RedirectResponse` | a constructor |
//! | `#[IsGranted]`, `@UseGuards` | an attribute or decorator only |
//! | `middleware('auth` | a call of that name whose arguments contain `'auth` |
//! | `log.*`, `fmt.Print*` | any name, or a name with that start |
//!
//! Names and receivers match without regard to case.

use super::config::{Extra, SecurityConfig};
use crate::units::lang::Lang;

/// What a sink does with what it's given: its subject in a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SinkKind {
    Sql,
    Shell,
    Eval,
    Deserialization,
    FilePath,
    Http,
    Template,
    Redirect,
}

impl SinkKind {
    pub const ALL: [SinkKind; 8] = [
        SinkKind::Sql,
        SinkKind::Shell,
        SinkKind::Eval,
        SinkKind::Deserialization,
        SinkKind::FilePath,
        SinkKind::Http,
        SinkKind::Template,
        SinkKind::Redirect,
    ];

    /// The fact's subject: `raw SQL`, `shell`, `eval`, `deserialization`,
    /// `file path`, `outgoing HTTP`, `template`, `redirect`.
    pub fn word(self) -> &'static str {
        match self {
            SinkKind::Sql => "raw SQL",
            SinkKind::Shell => "shell",
            SinkKind::Eval => "eval",
            SinkKind::Deserialization => "deserialization",
            SinkKind::FilePath => "file path",
            SinkKind::Http => "outgoing HTTP",
            SinkKind::Template => "template",
            SinkKind::Redirect => "redirect",
        }
    }

    /// Its key under `sinks` in `security.toml`: `sql`, `shell`, `eval`,
    /// `deserialization`, `file`, `http`, `template`, `redirect`.
    pub fn key(self) -> &'static str {
        match self {
            SinkKind::Sql => "sql",
            SinkKind::Shell => "shell",
            SinkKind::Eval => "eval",
            SinkKind::Deserialization => "deserialization",
            SinkKind::FilePath => "file",
            SinkKind::Http => "http",
            SinkKind::Template => "template",
            SinkKind::Redirect => "redirect",
        }
    }

    pub fn from_key(key: &str) -> Option<SinkKind> {
        SinkKind::ALL.into_iter().find(|k| k.key() == key)
    }

    /// The kind of a sink the user listed under `injection`, from its name:
    /// a query builder's raw method unless the name says otherwise.
    fn guess(name: &str) -> SinkKind {
        let n = name.to_ascii_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
        if has(&["redirect"]) {
            SinkKind::Redirect
        } else if has(&["shell", "exec", "command", "spawn", "process"]) {
            SinkKind::Shell
        } else if has(&["eval"]) {
            SinkKind::Eval
        } else if has(&["unserial", "deserial"]) {
            SinkKind::Deserialization
        } else if has(&["file", "path", "fopen", "readfile"]) {
            SinkKind::FilePath
        } else if has(&["http", "fetch", "request", "curl", "url"]) {
            SinkKind::Http
        } else if has(&["html", "template", "render"]) {
            SinkKind::Template
        } else {
            SinkKind::Sql
        }
    }
}

/// Which argument makes a sink dangerous, and what it must look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taint {
    /// Argument `n` (the last one when there are fewer) isn't a plain
    /// literal.
    NotLiteral(usize),
    /// Argument `n` (the last one when there are fewer) is built from a
    /// variable: interpolation, concatenation, `sprintf`, or a variable
    /// the same function built so.
    Built(usize),
    /// Any argument from `n` on isn't a plain literal.
    AnyNotLiteral(usize),
    /// An options argument says `shell: true`, and another isn't a literal.
    ShellOption,
}

/// How a call is written, as patterns tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Any of them (patterns only).
    Any,
    /// `f(x)`: no receiver.
    Function,
    /// `a.f(x)`, `$a->f(x)`, `A::f(x)`.
    Method,
    /// `new A(x)`.
    New,
    /// `#[A(x)]`, `@A(x)`: an attribute, decorator or doc annotation.
    Mark,
}

/// A call, attribute or decorator, as the patterns see it.
#[derive(Debug, Clone, Copy)]
pub struct Call<'a> {
    pub form: Form,
    /// The called name: `query`, `IsGranted`.
    pub name: &'a str,
    /// The receiver, normalized by [`chain`]: `this.conn`, `gate`.
    pub recv: Option<&'a str>,
    /// The arguments' text, normalized by [`args_text`].
    pub args: &'a str,
}

/// One catalog name, parsed (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    /// As written.
    pub text: String,
    form: Form,
    /// Lowercase, `.`-separated; a leading `*` matches the end of the
    /// receiver's last part.
    recv: Option<String>,
    /// Lowercase; `*` is any name, `x*` any name starting with `x`.
    name: String,
    /// Lowercase, quotes as `'`: the arguments must contain it.
    arg: Option<String>,
}

impl Pattern {
    pub fn parse(text: &str) -> Pattern {
        let mut s = text.trim();
        let mut form = Form::Any;
        if let Some(rest) = s.strip_prefix("#[") {
            form = Form::Mark;
            s = rest.trim_end_matches(']');
        } else if let Some(rest) = s.strip_prefix('@') {
            form = Form::Mark;
            s = rest;
        } else if let Some(rest) = s.strip_prefix("new ") {
            form = Form::New;
            s = rest.trim();
        }
        let mut arg = None;
        if let Some(fun) = s.strip_suffix("()") {
            if form == Form::Any {
                form = Form::Function;
            }
            s = fun;
        } else if let Some(i) = s.find('(') {
            let a = s[i + 1..].trim_end_matches(')');
            if !a.is_empty() {
                arg = Some(a.to_ascii_lowercase().replace('"', "'"));
            }
            s = &s[..i];
        }
        let path = chain(s);
        let (recv, name) = match path.rfind('.') {
            Some(i) => (Some(path[..i].to_string()), path[i + 1..].to_string()),
            None => (None, path),
        };
        // `->query` and `.query`: a method of any receiver.
        let (recv, form) = match recv {
            Some(r) if r.is_empty() => (None, Form::Method),
            r => (r, form),
        };
        Pattern {
            text: text.trim().to_string(),
            form,
            recv,
            name,
            arg,
        }
    }

    /// What a fact calls it: the pattern without its syntax,
    /// `middleware('auth')`, `Gate::allows`, `IsGranted`.
    pub fn label(&self) -> String {
        let t = self
            .text
            .trim_start_matches("#[")
            .trim_end_matches(']')
            .trim_start_matches('@')
            .trim_start_matches("new ")
            .trim_start_matches("->")
            .trim_start_matches('.')
            .trim_end_matches("()");
        match &self.arg {
            Some(_) if !t.ends_with(')') => {
                let quote = if t.contains('"') { '"' } else { '\'' };
                if t.ends_with(quote) {
                    format!("{t})")
                } else {
                    format!("{t}{quote})")
                }
            }
            _ => t.to_string(),
        }
    }

    /// Has a receiver, an argument or a form of its own: a fact names it
    /// by [`Pattern::label`] rather than by the name in the code.
    pub fn qualified(&self) -> bool {
        self.recv.is_some() || self.arg.is_some()
    }

    pub fn matches(&self, c: &Call) -> bool {
        let form_ok = match self.form {
            Form::Any => true,
            Form::Method => c.form == Form::Method,
            f => f == c.form,
        };
        if !form_ok || !name_matches(&self.name, c.name) {
            return false;
        }
        if let Some(want) = &self.recv {
            let Some(have) = c.recv else { return false };
            let have = have.to_ascii_lowercase();
            let ok = if let Some(end) = want.strip_prefix('*') {
                have.rsplit('.').next().unwrap_or("").ends_with(end)
            } else {
                have == *want || have.ends_with(&format!(".{want}"))
            };
            if !ok {
                return false;
            }
        }
        self.arg
            .as_ref()
            .is_none_or(|a| c.args.contains(a.as_str()))
    }
}

fn name_matches(want: &str, have: &str) -> bool {
    // Prisma's `$queryRawUnsafe`: the `$` is part of the name in the code.
    let have = have.trim_start_matches('$');
    if want == "*" {
        return true;
    }
    if let Some(start) = want.strip_suffix('*') {
        return have.to_ascii_lowercase().starts_with(start);
    }
    have.eq_ignore_ascii_case(want)
}

/// A receiver or callee as patterns compare them: `->`, `?->`, `::` and
/// `\` become `.`, `$` and whatever is in brackets go, lowercase. A `*`
/// stays: patterns use it.
/// `$this->client->messages()` → `this.client.messages`.
pub fn chain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            '-' if chars.peek() == Some(&'>') => {
                chars.next();
                out.push('.');
            }
            '?' if chars.peek() == Some(&'-') || chars.peek() == Some(&'.') => {}
            ':' if chars.peek() == Some(&':') => {
                chars.next();
                out.push('.');
            }
            '\\' => out.push('.'),
            '$' | '!' | '&' => {}
            c if c.is_whitespace() => {}
            c => out.extend(c.to_lowercase()),
        }
    }
    let parts: Vec<&str> = out.split('.').filter(|p| !p.is_empty()).collect();
    let mut joined = parts.join(".");
    // `->name` and `.name` keep their mark: a method of any receiver.
    if text.trim_start().starts_with("->") || text.trim_start().starts_with('.') {
        joined.insert(0, '.');
    }
    joined
}

/// Arguments' text as patterns read it: lowercase, `"` as `'`.
pub fn args_text(text: &str) -> String {
    text.to_ascii_lowercase().replace('"', "'")
}

/// A sink: what it is, how it's called, and which argument matters.
#[derive(Debug, Clone)]
pub struct Sink {
    pub kind: SinkKind,
    pub pattern: Pattern,
    pub taint: Taint,
    /// The file must mention one of these (an import), or the sink isn't
    /// the one meant: `exec` is a shell only with `child_process`.
    pub requires: &'static [&'static str],
}

/// What makes a function an entry point, in words: `route`, `message
/// handler`, `command`.
pub fn entry_how(mark: &str) -> &'static str {
    match mark.to_ascii_lowercase().as_str() {
        "route" | "get" | "post" | "put" | "patch" | "delete" | "all" | "options" | "head"
        | "sse" => "route",
        "asmessagehandler" | "messagepattern" | "eventpattern" | "processor" => "message handler",
        "ascommand" | "command" => "command",
        "query" | "mutation" | "subscription" | "resolvefield" => "GraphQL resolver",
        "ascontroller" => "controller action",
        _ => "entry point",
    }
}

/// A language's catalog: the built-in names plus the user's.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub lang: Lang,
    /// Authorization checks, as patterns (see the module docs).
    pub checks: Vec<String>,
    /// What makes a function an entry point: attributes, decorators and
    /// route registrations (`Route::get`), as patterns.
    pub entry: Vec<String>,
    pub sinks: Vec<Sink>,
    /// Calls to a model's API: `messages.create`, `generateText()`.
    pub models: Vec<String>,
    /// Logging calls.
    pub logs: Vec<String>,
    /// Calls that put a value into an HTTP response.
    pub responses: Vec<String>,
    check_pats: Vec<Pattern>,
    entry_pats: Vec<Pattern>,
    model_pats: Vec<Pattern>,
    log_pats: Vec<Pattern>,
    response_pats: Vec<Pattern>,
}

impl Catalog {
    fn new(lang: Lang, b: Builtin, extra: &Extra) -> Catalog {
        let own = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut checks = own(b.checks);
        checks.extend(extra.checks.iter().cloned());
        let mut entry = own(b.entry);
        entry.extend(extra.entry.iter().cloned());
        let mut sinks: Vec<Sink> = b
            .sinks
            .iter()
            .map(|&(kind, p, taint, requires)| Sink {
                kind,
                pattern: Pattern::parse(p),
                taint,
                requires,
            })
            .collect();
        let mut models = own(b.models);
        let mut logs = own(b.logs);
        let mut responses = own(b.responses);
        // The user's sinks go first: their own names win over a built-in one.
        let mut theirs = Vec::new();
        for (key, names) in &extra.sinks {
            for n in names {
                let kind = match key.as_str() {
                    "injection" => Some(SinkKind::guess(n)),
                    "exposure" | "log" | "logs" => {
                        logs.push(n.clone());
                        None
                    }
                    "response" | "responses" => {
                        responses.push(n.clone());
                        None
                    }
                    "llm" => {
                        models.push(n.clone());
                        None
                    }
                    k => SinkKind::from_key(k),
                };
                if let Some(kind) = kind {
                    theirs.push(Sink {
                        kind,
                        pattern: Pattern::parse(n),
                        taint: if kind == SinkKind::Sql {
                            Taint::Built(0)
                        } else {
                            Taint::NotLiteral(0)
                        },
                        requires: &[],
                    });
                }
            }
        }
        theirs.append(&mut sinks);
        let parse = |v: &[String]| v.iter().map(|s| Pattern::parse(s)).collect::<Vec<_>>();
        Catalog {
            lang,
            check_pats: parse(&checks),
            entry_pats: parse(&entry),
            model_pats: parse(&models),
            log_pats: parse(&logs),
            response_pats: parse(&responses),
            checks,
            entry,
            sinks: theirs,
            models,
            logs,
            responses,
        }
    }

    /// The check `c` is, by the name a fact gives it: the code's own name
    /// (`IsGranted`), or the pattern's for one with a receiver or argument
    /// (`middleware('auth')`).
    pub fn check(&self, c: &Call) -> Option<String> {
        // `MyVoter::vote` before `->vote`: the narrower name says more.
        let p = self
            .check_pats
            .iter()
            .filter(|p| p.qualified())
            .chain(self.check_pats.iter().filter(|p| !p.qualified()))
            .find(|p| p.matches(c))?;
        Some(if p.qualified() {
            p.label()
        } else {
            c.name.to_string()
        })
    }

    /// `c` marks or registers an entry point.
    pub fn entry(&self, c: &Call) -> Option<&Pattern> {
        self.entry_pats.iter().find(|p| p.matches(c))
    }

    /// The sink `c` is, if the file (`text`) is one it can be in.
    pub fn sink(&self, c: &Call, text: &str) -> Option<&Sink> {
        self.sinks.iter().find(|s| {
            s.pattern.matches(c)
                && (s.requires.is_empty() || s.requires.iter().any(|r| text.contains(r)))
        })
    }

    pub fn model(&self, c: &Call) -> bool {
        self.model_pats.iter().any(|p| p.matches(c))
    }

    pub fn log(&self, c: &Call) -> bool {
        self.log_pats.iter().any(|p| p.matches(c))
    }

    pub fn response(&self, c: &Call) -> bool {
        self.response_pats.iter().any(|p| p.matches(c))
    }

    /// The first check named in `code`, found as a whole word, by its
    /// label: for code a scan didn't parse, or a quick look along a path.
    pub fn check_in(&self, code: &str) -> Option<String> {
        let lower = code.to_ascii_lowercase().replace('"', "'");
        for p in &self.check_pats {
            if p.name == "*" || p.name.ends_with('*') {
                continue;
            }
            let mut from = 0;
            while let Some(i) = lower[from..].find(&p.name) {
                let at = from + i;
                let end = at + p.name.len();
                let before = lower[..at].chars().next_back();
                let after = lower[end..].chars().next();
                let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
                // `@Security` is an annotation, not the word "security".
                let mark_ok = p.form != Form::Mark || matches!(before, Some('@' | '['));
                // `Gate::allows` needs its receiver just before the name.
                let recv_ok = match &p.recv {
                    Some(r) if !r.starts_with('*') => {
                        let last = r.rsplit('.').next().unwrap_or(r);
                        lower[..at]
                            .trim_end_matches(['.', '-', '>', ':', '?'])
                            .ends_with(last)
                    }
                    _ => true,
                };
                if !word(before) && !word(after) && mark_ok && recv_ok {
                    let rest = &lower[end..];
                    let arg_ok = p.arg.as_ref().is_none_or(|a| {
                        rest.trim_start().starts_with('(')
                            && rest
                                .split(')')
                                .next()
                                .is_some_and(|args| args.contains(a.as_str()))
                    });
                    if arg_ok {
                        return Some(p.label());
                    }
                }
                from = end;
            }
        }
        None
    }
}

/// The built-in catalog for `lang` with the user's additions for it.
pub fn for_lang(lang: Lang, cfg: &SecurityConfig) -> Catalog {
    let mut extra = cfg.extra(lang.config_name());
    // JavaScript shares TypeScript's catalog, and its additions.
    if lang == Lang::JavaScript {
        let ts = cfg.extra("typescript");
        extra.checks.extend(ts.checks);
        extra.entry.extend(ts.entry);
        for (k, v) in ts.sinks {
            extra.sinks.entry(k).or_default().extend(v);
        }
    }
    Catalog::new(lang, builtin(lang), &extra)
}

type SinkDef = (SinkKind, &'static str, Taint, &'static [&'static str]);

struct Builtin {
    checks: &'static [&'static str],
    entry: &'static [&'static str],
    sinks: &'static [SinkDef],
    models: &'static [&'static str],
    logs: &'static [&'static str],
    responses: &'static [&'static str],
}

fn builtin(lang: Lang) -> Builtin {
    match lang {
        Lang::Php => PHP,
        Lang::Go => GO,
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => TS,
        Lang::Rust | Lang::Python => Builtin {
            checks: &[],
            entry: &[],
            sinks: &[],
            models: &[],
            logs: &[],
            responses: &[],
        },
    }
}

use SinkKind::*;
use Taint::*;

const NONE: &[&str] = &[];
const CHILD_PROCESS: &[&str] = &["child_process"];
const EXECA: &[&str] = &["execa"];
const FS: &[&str] = &[
    "'fs'",
    "\"fs\"",
    "'fs/promises'",
    "\"fs/promises\"",
    "node:fs",
    "fs-extra",
];
const NODE_SERIALIZE: &[&str] = &["node-serialize", "serialize-javascript"];

const PHP: Builtin = Builtin {
    checks: &[
        "isGranted",
        "denyAccessUnlessGranted",
        "IsGranted",
        "@Security",
        "->vote",
        "authorize",
        "Gate::allows",
        "Gate::denies",
        "Gate::authorize",
        "Gate::check",
        "Gate::any",
        "Gate::none",
        "->can",
        "->cannot",
        "middleware('auth",
        "middleware('can:",
        "middleware('role",
        "middleware('permission",
        "hasRole",
        "hasPermissionTo",
    ],
    entry: &[
        "Route",
        "AsMessageHandler",
        "AsCommand",
        "AsController",
        "Route::get",
        "Route::post",
        "Route::put",
        "Route::patch",
        "Route::delete",
        "Route::any",
        "Route::options",
        "Route::resource",
        "Route::apiResource",
    ],
    sinks: &[
        (Sql, "->query", Built(0), NONE),
        (Sql, "->exec", Built(0), NONE),
        (Sql, "->executeQuery", Built(0), NONE),
        (Sql, "->executeStatement", Built(0), NONE),
        (Sql, "->createNativeQuery", Built(0), NONE),
        (Sql, "->createQuery", Built(0), NONE),
        (Sql, "->prepare", Built(0), NONE),
        (Sql, "DB::raw", Built(0), NONE),
        (Sql, "DB::select", Built(0), NONE),
        (Sql, "DB::statement", Built(0), NONE),
        (Sql, "DB::unprepared", Built(0), NONE),
        (Sql, "DB::insert", Built(0), NONE),
        (Sql, "DB::update", Built(0), NONE),
        (Sql, "DB::delete", Built(0), NONE),
        (Sql, "->whereRaw", Built(0), NONE),
        (Sql, "->orWhereRaw", Built(0), NONE),
        (Sql, "->havingRaw", Built(0), NONE),
        (Sql, "->orderByRaw", Built(0), NONE),
        (Sql, "->selectRaw", Built(0), NONE),
        (Sql, "->groupByRaw", Built(0), NONE),
        (Sql, "->where", Built(0), NONE),
        (Sql, "->andWhere", Built(0), NONE),
        (Sql, "->orWhere", Built(0), NONE),
        (Sql, "->having", Built(0), NONE),
        (Sql, "mysqli_query()", Built(1), NONE),
        (Sql, "pg_query()", Built(1), NONE),
        (Shell, "exec()", NotLiteral(0), NONE),
        (Shell, "shell_exec()", NotLiteral(0), NONE),
        (Shell, "system()", NotLiteral(0), NONE),
        (Shell, "passthru()", NotLiteral(0), NONE),
        (Shell, "proc_open()", NotLiteral(0), NONE),
        (Shell, "popen()", NotLiteral(0), NONE),
        (Shell, "pcntl_exec()", NotLiteral(0), NONE),
        (Shell, "Process::fromShellCommandline", NotLiteral(0), NONE),
        (Eval, "eval()", NotLiteral(0), NONE),
        (Eval, "create_function()", AnyNotLiteral(0), NONE),
        (Deserialization, "unserialize()", NotLiteral(0), NONE),
        (FilePath, "file_get_contents()", NotLiteral(0), NONE),
        (FilePath, "fopen()", NotLiteral(0), NONE),
        (FilePath, "readfile()", NotLiteral(0), NONE),
        (FilePath, "unlink()", NotLiteral(0), NONE),
        (FilePath, "file_put_contents()", NotLiteral(0), NONE),
        (FilePath, "file()", NotLiteral(0), NONE),
        (FilePath, "copy()", NotLiteral(0), NONE),
        (FilePath, "rename()", NotLiteral(0), NONE),
        (FilePath, "move_uploaded_file()", NotLiteral(1), NONE),
        (FilePath, "new SplFileObject", NotLiteral(0), NONE),
        (FilePath, "new BinaryFileResponse", NotLiteral(0), NONE),
        (Http, "curl_init()", NotLiteral(0), NONE),
        (Http, "curl_setopt(curlopt_url", NotLiteral(2), NONE),
        (Http, "*client->request", NotLiteral(1), NONE),
        (Http, "Http::get", NotLiteral(0), NONE),
        (Http, "Http::post", NotLiteral(0), NONE),
        (Http, "Http::put", NotLiteral(0), NONE),
        (Http, "Http::patch", NotLiteral(0), NONE),
        (Http, "Http::delete", NotLiteral(0), NONE),
        (Redirect, "header(location", Built(0), NONE),
        (Redirect, "->redirect", NotLiteral(0), NONE),
        (Redirect, "->away", NotLiteral(0), NONE),
        (Redirect, "redirect()", NotLiteral(0), NONE),
        (Redirect, "new RedirectResponse", NotLiteral(0), NONE),
    ],
    models: &[
        "messages.create",
        "chat.create",
        "completions.create",
        "responses.create",
    ],
    logs: &[
        "*logger.*",
        "log.*",
        "error_log()",
        "var_dump()",
        "print_r()",
        "dump()",
        "dd()",
        "syslog()",
    ],
    responses: &["new JsonResponse", "->json"],
};

const TS: Builtin = Builtin {
    checks: &[
        "@UseGuards",
        "@Roles",
        "canActivate",
        "requireAuth",
        "isAuthenticated",
        "ensureAuthenticated",
        "authorize",
        "passport.authenticate",
        "getServerSession",
        // NextAuth's `await auth()`; not Firebase's `firebase.auth()`.
        "auth()",
        "@Auth",
        "withAuth",
        "requireUser",
        "requireSession",
        "withPageAuthRequired",
        "withApiAuthRequired",
        "checkPermission",
        "checkPermissions",
        "hasPermission",
        "hasRole",
        "jwt.verify",
    ],
    entry: &[
        "@Controller",
        "@Get",
        "@Post",
        "@Put",
        "@Patch",
        "@Delete",
        "@All",
        "@Options",
        "@Head",
        "@Sse",
        "@MessagePattern",
        "@EventPattern",
        "@Query",
        "@Mutation",
        "@Subscription",
    ],
    sinks: &[
        (Eval, "eval()", NotLiteral(0), NONE),
        (Eval, "new Function", AnyNotLiteral(0), NONE),
        (Eval, "vm.runInNewContext", NotLiteral(0), NONE),
        (Eval, "vm.runInThisContext", NotLiteral(0), NONE),
        (Eval, "vm.runInContext", NotLiteral(0), NONE),
        (Eval, "vm.compileFunction", NotLiteral(0), NONE),
        (Eval, "new vm.Script", NotLiteral(0), NONE),
        (Shell, "exec()", NotLiteral(0), CHILD_PROCESS),
        (Shell, "execSync()", NotLiteral(0), CHILD_PROCESS),
        (Shell, "child_process.exec", NotLiteral(0), NONE),
        (Shell, "child_process.execSync", NotLiteral(0), NONE),
        (Shell, "cp.exec", NotLiteral(0), CHILD_PROCESS),
        (Shell, "cp.execSync", NotLiteral(0), CHILD_PROCESS),
        (Shell, "childProcess.exec", NotLiteral(0), CHILD_PROCESS),
        (Shell, "childProcess.execSync", NotLiteral(0), CHILD_PROCESS),
        (Shell, "spawn()", ShellOption, CHILD_PROCESS),
        (Shell, "spawnSync()", ShellOption, CHILD_PROCESS),
        (Shell, "child_process.spawn", ShellOption, NONE),
        (Shell, "child_process.spawnSync", ShellOption, NONE),
        (Shell, "cp.spawn", ShellOption, CHILD_PROCESS),
        (Shell, "execaCommand", NotLiteral(0), EXECA),
        (Shell, "execaCommandSync", NotLiteral(0), EXECA),
        (Sql, ".$queryRawUnsafe", NotLiteral(0), NONE),
        (Sql, ".$executeRawUnsafe", NotLiteral(0), NONE),
        (Sql, ".query", Built(0), NONE),
        (Sql, ".raw", Built(0), NONE),
        (Sql, ".whereRaw", Built(0), NONE),
        (Sql, ".orWhereRaw", Built(0), NONE),
        (Sql, ".havingRaw", Built(0), NONE),
        (Sql, ".orderByRaw", Built(0), NONE),
        (Sql, ".joinRaw", Built(0), NONE),
        (Sql, ".execute", Built(0), NONE),
        (Sql, ".where", Built(0), NONE),
        (Sql, ".andWhere", Built(0), NONE),
        (Sql, ".orWhere", Built(0), NONE),
        (Sql, "sequelize.literal", Built(0), NONE),
        (FilePath, "readFile", NotLiteral(0), FS),
        (FilePath, "readFileSync", NotLiteral(0), FS),
        (FilePath, "writeFile", NotLiteral(0), FS),
        (FilePath, "writeFileSync", NotLiteral(0), FS),
        (FilePath, "appendFile", NotLiteral(0), FS),
        (FilePath, "appendFileSync", NotLiteral(0), FS),
        (FilePath, "createReadStream", NotLiteral(0), FS),
        (FilePath, "createWriteStream", NotLiteral(0), FS),
        (FilePath, "unlink", NotLiteral(0), FS),
        (FilePath, "unlinkSync", NotLiteral(0), FS),
        (FilePath, "rm", NotLiteral(0), FS),
        (FilePath, "rmSync", NotLiteral(0), FS),
        (FilePath, "copyFile", NotLiteral(0), FS),
        (FilePath, "copyFileSync", NotLiteral(0), FS),
        (FilePath, "res.sendFile", NotLiteral(0), NONE),
        (FilePath, "res.download", NotLiteral(0), NONE),
        (Http, "fetch()", NotLiteral(0), NONE),
        (Http, "axios()", NotLiteral(0), NONE),
        (Http, "axios.get", NotLiteral(0), NONE),
        (Http, "axios.post", NotLiteral(0), NONE),
        (Http, "axios.put", NotLiteral(0), NONE),
        (Http, "axios.patch", NotLiteral(0), NONE),
        (Http, "axios.delete", NotLiteral(0), NONE),
        (Http, "axios.head", NotLiteral(0), NONE),
        (Http, "axios.request", NotLiteral(0), NONE),
        (Http, "got()", NotLiteral(0), NONE),
        (Http, "got.get", NotLiteral(0), NONE),
        (Http, "got.post", NotLiteral(0), NONE),
        (Http, "http.get", NotLiteral(0), NONE),
        (Http, "http.request", NotLiteral(0), NONE),
        (Http, "https.get", NotLiteral(0), NONE),
        (Http, "https.request", NotLiteral(0), NONE),
        (Template, ".insertAdjacentHTML", NotLiteral(1), NONE),
        (Template, "document.write", NotLiteral(0), NONE),
        (Template, "document.writeln", NotLiteral(0), NONE),
        (Redirect, ".redirect", AnyNotLiteral(0), NONE),
        (Redirect, "redirect()", NotLiteral(0), NONE),
        (
            Deserialization,
            "unserialize",
            NotLiteral(0),
            NODE_SERIALIZE,
        ),
        (Deserialization, "v8.deserialize", NotLiteral(0), NONE),
    ],
    models: &[
        "messages.create",
        "messages.stream",
        "chat.completions.create",
        "completions.create",
        "responses.create",
        "generateText()",
        "streamText()",
        "generateObject()",
        "streamObject()",
    ],
    logs: &["console.*", "*logger.*", "log.*"],
    responses: &[
        "res.json",
        "res.send",
        "Response.json",
        "NextResponse.json",
        "reply.send",
    ],
};

const GO: Builtin = Builtin {
    checks: &[
        "RequireAuth",
        "Authorize",
        "IsAuthorized",
        "CheckPermission",
        "HasPermission",
        "RequireRole",
        "authMiddleware",
        "AuthRequired",
        "RequireLogin",
        "Authenticate",
        "BasicAuth",
        "Enforce",
    ],
    entry: &[],
    sinks: &[
        (Shell, "exec.Command", AnyNotLiteral(0), NONE),
        (Shell, "exec.CommandContext", AnyNotLiteral(1), NONE),
        (Sql, ".Query", Built(0), NONE),
        (Sql, ".Exec", Built(0), NONE),
        (Sql, ".QueryRow", Built(0), NONE),
        (Sql, ".Queryx", Built(0), NONE),
        (Sql, ".QueryContext", Built(1), NONE),
        (Sql, ".ExecContext", Built(1), NONE),
        (Sql, ".QueryRowContext", Built(1), NONE),
        (Sql, ".Prepare", Built(0), NONE),
        (Sql, ".PrepareContext", Built(1), NONE),
        (Sql, ".Raw", Built(0), NONE),
        (Sql, ".Where", Built(0), NONE),
        (Template, "template.HTML", NotLiteral(0), NONE),
        (Template, "template.JS", NotLiteral(0), NONE),
        (Template, "template.URL", NotLiteral(0), NONE),
        (Template, "template.HTMLAttr", NotLiteral(0), NONE),
        (Template, "template.CSS", NotLiteral(0), NONE),
        (FilePath, "os.Open", NotLiteral(0), NONE),
        (FilePath, "os.OpenFile", NotLiteral(0), NONE),
        (FilePath, "os.ReadFile", NotLiteral(0), NONE),
        (FilePath, "os.WriteFile", NotLiteral(0), NONE),
        (FilePath, "os.Create", NotLiteral(0), NONE),
        (FilePath, "os.Remove", NotLiteral(0), NONE),
        (FilePath, "os.RemoveAll", NotLiteral(0), NONE),
        (FilePath, "ioutil.ReadFile", NotLiteral(0), NONE),
        (FilePath, "ioutil.WriteFile", NotLiteral(0), NONE),
        (FilePath, "http.ServeFile", NotLiteral(2), NONE),
        (Http, "http.Get", NotLiteral(0), NONE),
        (Http, "http.Post", NotLiteral(0), NONE),
        (Http, "http.Head", NotLiteral(0), NONE),
        (Http, "http.PostForm", NotLiteral(0), NONE),
        (Http, "http.NewRequest", NotLiteral(1), NONE),
        (Http, "http.NewRequestWithContext", NotLiteral(2), NONE),
        (Redirect, "http.Redirect", NotLiteral(2), NONE),
        (Redirect, "c.Redirect", NotLiteral(1), NONE),
    ],
    models: &[
        "Messages.New",
        "Completions.New",
        "Responses.New",
        "CreateChatCompletion",
        "CreateCompletion",
    ],
    logs: &["log.*", "slog.*", "*logger.*", "fmt.Print*"],
    responses: &[
        "c.JSON",
        ".Encode",
        "w.Write",
        "fmt.Fprintf",
        "fmt.Fprint",
        "fmt.Fprintln",
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    fn call<'a>(form: Form, recv: Option<&'a str>, name: &'a str, args: &'a str) -> Call<'a> {
        Call {
            form,
            name,
            recv,
            args,
        }
    }

    #[test]
    fn patterns_parse_their_forms() {
        let p = Pattern::parse("Gate::allows");
        assert!(p.matches(&call(Form::Method, Some("gate"), "allows", "")));
        assert!(p.matches(&call(
            Form::Method,
            Some("illuminate.support.facades.gate"),
            "allows",
            ""
        )));
        assert!(!p.matches(&call(Form::Method, Some("other"), "allows", "")));
        assert_eq!(p.label(), "Gate::allows");

        let m = Pattern::parse("->query");
        assert!(m.matches(&call(Form::Method, Some("this.conn"), "query", "")));
        assert!(!m.matches(&call(Form::Function, None, "query", "")));

        let f = Pattern::parse("exec()");
        assert!(f.matches(&call(Form::Function, None, "exec", "")));
        assert!(!f.matches(&call(Form::Method, Some("pdo"), "exec", "")));

        let mark = Pattern::parse("#[IsGranted]");
        assert!(mark.matches(&call(Form::Mark, None, "IsGranted", "")));
        assert!(!mark.matches(&call(Form::Function, None, "isGranted", "")));
        assert_eq!(Pattern::parse("@UseGuards").label(), "UseGuards");

        let arg = Pattern::parse("middleware('auth");
        assert!(arg.matches(&call(
            Form::Method,
            Some("route.get"),
            "middleware",
            "('auth:sanctum')"
        )));
        assert!(!arg.matches(&call(Form::Method, None, "middleware", "('throttle')")));
        assert_eq!(arg.label(), "middleware('auth')");

        let star = Pattern::parse("*client->request");
        assert!(star.matches(&call(Form::Method, Some("this.httpclient"), "request", "")));
        assert!(!star.matches(&call(Form::Method, Some("this.repo"), "request", "")));

        let any = Pattern::parse("fmt.Print*");
        assert!(any.matches(&call(Form::Method, Some("fmt"), "Println", "")));
        let all = Pattern::parse("console.*");
        assert!(all.matches(&call(Form::Method, Some("console"), "log", "")));

        let new = Pattern::parse("new RedirectResponse");
        assert!(new.matches(&call(Form::New, None, "RedirectResponse", "")));
        assert!(!new.matches(&call(Form::Function, None, "RedirectResponse", "")));
    }

    #[test]
    fn chains_normalize() {
        assert_eq!(chain("$this->client->messages()"), "this.client.messages");
        assert_eq!(chain("\\App\\Gate"), "app.gate");
        assert_eq!(chain("fs.promises"), "fs.promises");
        assert_eq!(chain("a?.b!.c"), "a.b.c");
        assert_eq!(chain("->query"), ".query");
    }

    #[test]
    fn the_catalog_merges_the_users_names() {
        let cfg: SecurityConfig = toml::from_str(
            "[php]\nchecks = [\"MyVoter::vote\"]\nentry = [\"AsWebhook\"]\nsinks.injection = [\"Db::raw\"]\nsinks.shell = [\"runCmd\"]\n[typescript]\nchecks = [\"mustBeAdmin\"]\n",
        )
        .unwrap();
        let php = for_lang(Lang::Php, &cfg);
        assert!(php.checks.iter().any(|c| c == "denyAccessUnlessGranted"));
        assert!(php.checks.iter().any(|c| c == "MyVoter::vote"));
        assert_eq!(
            php.check(&call(Form::Method, Some("this.myvoter"), "vote", "")),
            Some("MyVoter::vote".into())
        );
        assert!(
            php.entry(&call(Form::Mark, None, "AsWebhook", ""))
                .is_some()
        );
        let raw = php
            .sink(&call(Form::Method, Some("db"), "raw", ""), "")
            .unwrap();
        assert_eq!(raw.kind, SinkKind::Sql);
        let shell = php
            .sink(&call(Form::Function, None, "runCmd", ""), "")
            .unwrap();
        assert_eq!(shell.kind, SinkKind::Shell);
        // JavaScript reads TypeScript's names too.
        let js = for_lang(Lang::JavaScript, &cfg);
        assert!(js.checks.iter().any(|c| c == "mustBeAdmin"));
        assert!(
            for_lang(Lang::Go, &cfg)
                .checks
                .iter()
                .all(|c| c != "mustBeAdmin")
        );
    }

    #[test]
    fn sinks_that_need_an_import_need_it() {
        let ts = for_lang(Lang::TypeScript, &SecurityConfig::default());
        let exec = call(Form::Function, None, "exec", "(cmd)");
        assert!(
            ts.sink(&exec, "import { exec } from 'child_process'")
                .is_some()
        );
        assert!(ts.sink(&exec, "const exec = (x) => x").is_none());
        let regex = call(Form::Method, Some("re"), "exec", "(s)");
        assert!(ts.sink(&regex, "import 'child_process'").is_none());
    }

    #[test]
    fn checks_are_found_in_plain_code() {
        let php = for_lang(Lang::Php, &SecurityConfig::default());
        assert_eq!(
            php.check_in("$this->denyAccessUnlessGranted('EDIT', $brand);"),
            Some("denyAccessUnlessGranted".into())
        );
        assert_eq!(
            php.check_in("Route::get('/a', X::class)->middleware(\"auth\");"),
            Some("middleware('auth')".into())
        );
        assert_eq!(php.check_in("$this->isGrantedish()"), None);
        assert_eq!(php.check_in("->middleware('throttle')"), None);
    }
}
