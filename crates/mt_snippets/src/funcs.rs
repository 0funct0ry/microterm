//! The function and filter catalogue (SPEC §10.8.4). Each entry carries its signature,
//! description and an example; the registry drives parse-time checks, the editor and the
//! generated "Template functions" page. No function spawns a process or touches the network.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, FixedOffset};

use crate::context::{Context, Fact};
use crate::value::Val;

/// Upper bound for `n`/`len` style arguments.
pub const CAP: i64 = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fail(pub String);

impl From<&str> for Fail {
    fn from(s: &str) -> Self {
        Fail(s.to_string())
    }
}
impl From<String> for Fail {
    fn from(s: String) -> Self {
        Fail(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Str,
    Int,
    Bool,
}

impl Ty {
    pub fn name(self) -> &'static str {
        match self {
            Ty::Str => "string",
            Ty::Int => "integer",
            Ty::Bool => "true/false",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub name: &'static str,
    pub ty: Ty,
    pub optional: bool,
    /// Allowed literal values (empty = any).
    pub choices: &'static [&'static str],
}

const fn req(name: &'static str, ty: Ty) -> Param {
    Param {
        name,
        ty,
        optional: false,
        choices: &[],
    }
}
const fn opt(name: &'static str, ty: Ty) -> Param {
    Param {
        name,
        ty,
        optional: true,
        choices: &[],
    }
}
const fn opt_choice(name: &'static str, choices: &'static [&'static str]) -> Param {
    Param {
        name,
        ty: Ty::Str,
        optional: true,
        choices,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Func,
    Filter,
}

pub struct Env<'a> {
    pub ctx: &'a mut dyn Context,
    pub now: DateTime<FixedOffset>,
}

pub struct Args {
    pub pos: Vec<Val>,
    pub named: Vec<(String, Val)>,
}

impl Args {
    pub fn s(&self, i: usize) -> &str {
        self.pos.get(i).map_or("", Val::expose)
    }
    pub fn opt(&self, i: usize) -> Option<&str> {
        self.pos.get(i).map(Val::expose)
    }
    pub fn int(&self, i: usize) -> Result<i64, Fail> {
        let s = self.s(i).trim();
        s.parse()
            .map_err(|_| Fail(format!("expected an integer, got \"{s}\"")))
    }
    pub fn named(&self, k: &str) -> Option<&str> {
        self.named
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.expose())
    }
    pub fn named_int(&self, k: &str, default: i64) -> Result<i64, Fail> {
        match self.named(k) {
            None => Ok(default),
            Some(s) => s
                .trim()
                .parse()
                .map_err(|_| Fail(format!("{k}= expects an integer, got \"{s}\""))),
        }
    }
    pub fn named_bool(&self, k: &str, default: bool) -> Result<bool, Fail> {
        match self.named(k) {
            None => Ok(default),
            Some("true") => Ok(true),
            Some("false") => Ok(false),
            Some(s) => Err(Fail(format!("{k}= expects true or false, got \"{s}\""))),
        }
    }
    pub fn any_secret(&self) -> bool {
        self.pos.iter().any(Val::is_secret)
    }
}

pub type FuncFn = fn(&mut Env, &Args) -> Result<String, Fail>;
pub type FilterFn = fn(&mut Env, &str, &Args) -> Result<String, Fail>;

#[derive(Clone, Copy)]
pub enum Run {
    Func(FuncFn),
    Filter(FilterFn),
    /// Interactive (handled by the evaluator): `input`, `choice`, `secret`.
    Input,
    Choice,
    Secret,
    Cursor,
    Counter,
}

pub struct Def {
    pub name: &'static str,
    pub kind: Kind,
    pub group: &'static str,
    /// Signature as shown in docs and the editor.
    pub sig: &'static str,
    pub desc: &'static str,
    pub example: &'static str,
    pub params: &'static [Param],
    /// `(name, minimum count)` of trailing variadic string arguments.
    pub variadic: Option<(&'static str, usize)>,
    /// Named arguments (`default=` is accepted everywhere).
    pub named: &'static [(&'static str, Ty)],
    /// Result is generated or read from outside: memoised per call site (SPEC §10.8.3).
    pub memo: bool,
    pub run: Run,
}

#[allow(clippy::too_many_arguments)]
const fn func(
    name: &'static str,
    group: &'static str,
    sig: &'static str,
    desc: &'static str,
    example: &'static str,
    params: &'static [Param],
    named: &'static [(&'static str, Ty)],
    memo: bool,
    run: FuncFn,
) -> Def {
    Def {
        name,
        kind: Kind::Func,
        group,
        sig,
        desc,
        example,
        params,
        variadic: None,
        named,
        memo,
        run: Run::Func(run),
    }
}

#[allow(clippy::too_many_arguments)]
const fn filter(
    name: &'static str,
    group: &'static str,
    sig: &'static str,
    desc: &'static str,
    example: &'static str,
    params: &'static [Param],
    named: &'static [(&'static str, Ty)],
    run: FilterFn,
) -> Def {
    Def {
        name,
        kind: Kind::Filter,
        group,
        sig,
        desc,
        example,
        params,
        variadic: None,
        named,
        memo: false,
        run: Run::Filter(run),
    }
}

const UTC: &[(&str, Ty)] = &[("utc", Ty::Bool)];
const NONE: &[Param] = &[];
const NO_NAMED: &[(&str, Ty)] = &[];

pub static CATALOGUE: &[Def] = &[
    // ---- Generators
    func("uuid", "Generators", "uuid [v4|v7]", "UUID, lowercase hyphenated (default v4; v7 is time-ordered).", "{{ uuid v7 }}", &[opt_choice("version", &["v4", "v7"])], NO_NAMED, true, f_uuid),
    func("ulid", "Generators", "ulid", "ULID (Crockford base32).", "{{ ulid }}", NONE, NO_NAMED, true, f_ulid),
    func("nanoid", "Generators", "nanoid [len=21]", "URL-safe random id.", "{{ nanoid len=12 }}", NONE, &[("len", Ty::Int)], true, f_nanoid),
    func("random", "Generators", "random MIN MAX", "Random integer in the inclusive range.", "{{ random 1 6 }}", &[req("min", Ty::Int), req("max", Ty::Int)], NO_NAMED, true, f_random),
    func("random_string", "Generators", "random_string LEN [charset=alnum]", "Random string. `charset` is alnum, alpha, lower, upper, digits, hex, or any other string used as the character set.", "{{ random_string 8 charset=hex }}", &[req("len", Ty::Int)], &[("charset", Ty::Str)], true, f_random_string),
    func("password", "Generators", "password [len=20] [symbols=true]", "Password with at least one lower-case letter, upper-case letter and digit (and a symbol when enabled).", "{{ password len=24 symbols=false }}", NONE, &[("len", Ty::Int), ("symbols", Ty::Bool)], true, f_password),
    func("hex", "Generators", "hex N", "N random bytes as 2N hex characters.", "{{ hex 16 }}", &[req("n", Ty::Int)], NO_NAMED, true, f_hex),
    func("base64_random", "Generators", "base64_random N", "N random bytes, standard base64.", "{{ base64_random 24 }}", &[req("n", Ty::Int)], NO_NAMED, true, f_base64_random),
    Def { name: "pick", kind: Kind::Func, group: "Generators", sig: "pick A B …", desc: "One of the arguments, chosen at random (at least one).", example: "{{ pick \"red\" \"green\" \"blue\" }}", params: NONE, variadic: Some(("option", 1)), named: NO_NAMED, memo: true, run: Run::Func(f_pick) },
    Def { name: "counter", kind: Kind::Func, group: "Generators", sig: "counter NAME [start=1] [step=1]", desc: "Per-window counter kept in memory. It advances only when an insert succeeds; previews do not advance it.", example: "{{ counter \"build\" start=100 }}", params: &[req("name", Ty::Str)], variadic: None, named: &[("start", Ty::Int), ("step", Ty::Int)], memo: true, run: Run::Counter },
    func("lorem", "Generators", "lorem [words=8]", "Lorem-ipsum words.", "{{ lorem words=12 }}", NONE, &[("words", Ty::Int)], false, f_lorem),
    // ---- Date and time
    func("now", "Date and time", "now [FMT=\"%Y-%m-%d %H:%M:%S\"]", "Formatted current time (strftime). Local time zone unless `utc=true`.", "{{ now \"%H:%M\" utc=true }}", &[opt("format", Ty::Str)], UTC, false, f_now),
    func("date", "Date and time", "date", "`%Y-%m-%d`.", "{{ date }}", NONE, UTC, false, f_date),
    func("time", "Date and time", "time", "`%H:%M:%S`.", "{{ time }}", NONE, UTC, false, f_time),
    func("timestamp", "Date and time", "timestamp", "Unix epoch seconds.", "{{ timestamp }}", NONE, NO_NAMED, false, f_timestamp),
    func("timestamp_ms", "Date and time", "timestamp_ms", "Unix epoch milliseconds.", "{{ timestamp_ms }}", NONE, NO_NAMED, false, f_timestamp_ms),
    func("iso8601", "Date and time", "iso8601", "RFC 3339 timestamp with the UTC offset.", "{{ iso8601 }}", NONE, UTC, false, f_iso8601),
    func("date_offset", "Date and time", "date_offset OFFSET [FMT=\"%Y-%m-%d\"]", "Current time shifted by ±N followed by s, m, h, d or w.", "{{ date_offset \"-7d\" }}", &[req("offset", Ty::Str), opt("format", Ty::Str)], UTC, false, f_date_offset),
    // ---- System (always the local machine)
    func("user", "System", "user", "Login name (passwd entry, not `$USER`).", "{{ user }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::User)),
    func("hostname", "System", "hostname", "Local host name.", "{{ hostname }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::Hostname)),
    func("os", "System", "os", "`macos` or `linux`.", "{{ os }}", NONE, NO_NAMED, false, |_, _| Ok(std::env::consts::OS.to_string())),
    func("arch", "System", "arch", "CPU architecture, e.g. `aarch64`.", "{{ arch }}", NONE, NO_NAMED, false, |_, _| Ok(std::env::consts::ARCH.to_string())),
    func("home", "System", "home", "Home directory.", "{{ home }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::Home)),
    func("shell", "System", "shell", "The shell microterm spawns.", "{{ shell }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::Shell)),
    func("tmpdir", "System", "tmpdir", "`$TMPDIR` or `/tmp`.", "{{ tmpdir }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::Tmpdir)),
    func("cpus", "System", "cpus", "Available parallelism.", "{{ cpus }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::Cpus)),
    func("env", "System", "env NAME", "microterm's own environment variable (not the pane shell's). An unset variable is a failure.", "{{ env \"AWS_PROFILE\" default=\"dev\" }}", &[req("name", Ty::Str)], NO_NAMED, false, f_env),
    func("local_ip", "System", "local_ip [v4|v6]", "First non-loopback address of an up interface (`getifaddrs`; no traffic).", "{{ local_ip }}", &[opt_choice("family", &["v4", "v6"])], NO_NAMED, false, |e, a| fact(e, Fact::LocalIp { v6: a.s(0) == "v6" })),
    func("microterm_version", "System", "microterm_version", "e.g. `1.0.0`.", "{{ microterm_version }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::Version)),
    // ---- Terminal and pane
    func("cwd", "Terminal and pane", "cwd", "Working directory (OSC 7, else the process cwd for local panes). Fails when unknown.", "cd {{ cwd | quote }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::Cwd)),
    func("cwd_basename", "Terminal and pane", "cwd_basename", "Last component of `cwd`.", "{{ cwd_basename }}", NONE, NO_NAMED, false, |e, _| Ok(basename(&fact(e, Fact::Cwd)?))),
    func("git_branch", "Terminal and pane", "git_branch", "Current branch (a detached HEAD gives the short SHA). Reads `.git/HEAD` from `cwd` upwards, following worktree `gitdir:` files. Fails for remote panes and outside a repository.", "git push -u origin {{ git_branch }}", NONE, NO_NAMED, false, f_git_branch),
    func("git_root", "Terminal and pane", "git_root", "Root of the enclosing repository or worktree. Fails for remote panes and outside a repository.", "cd {{ git_root }}", NONE, NO_NAMED, false, f_git_root),
    func("pane_id", "Terminal and pane", "pane_id", "Numeric pane id, as in `ctl` and `$MICROTERM_PANE`.", "{{ pane_id }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::PaneId)),
    func("tab_id", "Terminal and pane", "tab_id", "Numeric tab id.", "{{ tab_id }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::TabId)),
    func("window_id", "Terminal and pane", "window_id", "Numeric window id.", "{{ window_id }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::WindowId)),
    func("tab_index", "Terminal and pane", "tab_index", "1-based position in the tab bar.", "{{ tab_index }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::TabIndex)),
    func("tab_title", "Terminal and pane", "tab_title", "Effective tab title.", "{{ tab_title }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::TabTitle)),
    func("pane_title", "Terminal and pane", "pane_title", "Effective pane title.", "{{ pane_title }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::PaneTitle)),
    func("workspace", "Terminal and pane", "workspace", "Workspace name.", "{{ workspace }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::Workspace)),
    func("domain", "Terminal and pane", "domain", "`local`, or the SSH/serial domain name.", "{{ domain }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::Domain)),
    func("remote_host", "Terminal and pane", "remote_host", "Host of the pane's SSH domain. Fails for local panes.", "{{ remote_host }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::RemoteHost)),
    func("cols", "Terminal and pane", "cols", "Grid width.", "{{ cols }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::Cols)),
    func("rows", "Terminal and pane", "rows", "Grid height.", "{{ rows }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::Rows)),
    func("font_family", "Terminal and pane", "font_family", "Effective font family. Fails headless.", "{{ font_family }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::FontFamily)),
    func("font_size", "Terminal and pane", "font_size", "Effective font size (including the per-window override). Fails headless.", "{{ font_size }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::FontSize)),
    func("theme", "Terminal and pane", "theme", "Theme name. Fails headless.", "{{ theme }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::Theme)),
    func("fg_process", "Terminal and pane", "fg_process", "Foreground process name. Fails for remote panes.", "{{ fg_process }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::FgProcess)),
    func("shell_pid", "Terminal and pane", "shell_pid", "Shell pid. Fails for remote panes.", "{{ shell_pid }}", NONE, NO_NAMED, false, |e, _| fact(e, Fact::ShellPid)),
    func("last_command", "Terminal and pane", "last_command [n=1]", "Command line of the n-th last OSC 133 command. Fails without shell integration.", "{{ last_command }}", NONE, &[("n", Ty::Int)], false, |e, a| {
        let n = a.named_int("n", 1)?;
        fact(e, Fact::LastCommand(usize::try_from(n.max(1)).unwrap_or(1)))
    }),
    func("last_output", "Terminal and pane", "last_output [n=1]", "Output of the n-th last OSC 133 command, capped at `ctl.capture_max_bytes` (the tail is kept). Fails without shell integration.", "{{ last_output | trim }}", NONE, &[("n", Ty::Int)], false, |e, a| {
        let n = a.named_int("n", 1)?;
        fact(e, Fact::LastOutput(usize::try_from(n.max(1)).unwrap_or(1)))
    }),
    func("selection", "Terminal and pane", "selection", "Selected text in the pane, `\"\"` if none. Fails headless.", "grep -rn {{ selection | quote }} .", NONE, NO_NAMED, true, |e, _| fact(e, Fact::Selection)),
    func("clipboard", "Terminal and pane", "clipboard", "System clipboard text, `\"\"` if empty. Fails headless.", "echo {{ clipboard | quote }}", NONE, NO_NAMED, true, |e, _| fact(e, Fact::Clipboard)),
    // ---- Interactive
    Def { name: "input", kind: Kind::Func, group: "Interactive", sig: "input LABEL [default=\"\"]", desc: "Single-line text field in the fill dialog. Several tags with the same label share one value; the default comes from the first.", example: "{{ input \"Message\" | quote }}", params: &[req("label", Ty::Str)], variadic: None, named: &[], memo: false, run: Run::Input },
    Def { name: "choice", kind: Kind::Func, group: "Interactive", sig: "choice LABEL A B …", desc: "Single-select list (type to filter); the first option is preselected.", example: "{{ choice \"Env\" \"dev\" \"prod\" }}", params: &[req("label", Ty::Str)], variadic: Some(("option", 1)), named: &[], memo: false, run: Run::Choice },
    Def { name: "secret", kind: Kind::Func, group: "Interactive", sig: "secret LABEL", desc: "Masked field. The value is never logged, previewed, remembered, stored in the MRU or passed over MCP.", example: "{{ secret \"Token\" }}", params: &[req("label", Ty::Str)], variadic: None, named: &[], memo: false, run: Run::Secret },
    Def { name: "cursor", kind: Kind::Func, group: "Interactive", sig: "cursor", desc: "Emits nothing; marks where the shell's cursor ends up after a single-line, non-running insert.", example: "psql {{ cursor }}", params: NONE, variadic: None, named: &[], memo: false, run: Run::Cursor },
    // ---- Filters
    filter("upper", "Case", "upper", "Upper case.", "{{ input \"X\" | upper }}", NONE, NO_NAMED, |_, s, _| Ok(s.to_uppercase())),
    filter("lower", "Case", "lower", "Lower case.", "{{ input \"X\" | lower }}", NONE, NO_NAMED, |_, s, _| Ok(s.to_lowercase())),
    filter("title", "Case", "title", "Title Case (words split on whitespace).", "{{ input \"X\" | title }}", NONE, NO_NAMED, |_, s, _| Ok(title_case(s))),
    filter("snake", "Case", "snake", "snake_case.", "{{ input \"X\" | snake }}", NONE, NO_NAMED, |_, s, _| Ok(split_words(s).join("_"))),
    filter("kebab", "Case", "kebab", "kebab-case.", "{{ input \"Branch\" | kebab }}", NONE, NO_NAMED, |_, s, _| Ok(split_words(s).join("-"))),
    filter("camel", "Case", "camel", "camelCase.", "{{ input \"X\" | camel }}", NONE, NO_NAMED, |_, s, _| Ok(camel_case(s))),
    filter("slug", "Case", "slug", "URL slug: lower-case alphanumeric runs joined by `-`.", "{{ input \"Title\" | slug }}", NONE, NO_NAMED, |_, s, _| Ok(split_words(s).join("-"))),
    filter("trim", "Text", "trim", "Strips leading and trailing whitespace.", "{{ last_output | trim }}", NONE, NO_NAMED, |_, s, _| Ok(s.trim().to_string())),
    filter("truncate", "Text", "truncate N [ellipsis=\"…\"]", "Shortens to at most N characters, ellipsis included.", "{{ input \"X\" | truncate 20 }}", &[req("n", Ty::Int)], &[("ellipsis", Ty::Str)], f_truncate),
    filter("replace", "Text", "replace FROM TO", "Replaces every occurrence of FROM (literal text).", "{{ input \"X\" | replace \" \" \"_\" }}", &[req("from", Ty::Str), req("to", Ty::Str)], NO_NAMED, |_, s, a| {
        if a.s(0).is_empty() {
            return Err(Fail("replace: FROM must not be empty".into()));
        }
        Ok(s.replace(a.s(0), a.s(1)))
    }),
    filter("default", "Text", "default VALUE", "VALUE when the input is empty.", "{{ input \"X\" | default \"none\" }}", &[req("value", Ty::Str)], NO_NAMED, |_, s, a| {
        Ok(if s.is_empty() { a.s(0).to_string() } else { s.to_string() })
    }),
    filter("silent", "Text", "silent", "Emits nothing. A binding made in the same tag still receives the value.", "{{ id := uuid | silent }}", NONE, NO_NAMED, |_, s, _| Ok(s.to_string())),
    filter("quote", "Quoting", "quote", "POSIX single quotes (`'` becomes `'\\''`; empty becomes `''`).", "{{ input \"Message\" | quote }}", NONE, NO_NAMED, |_, s, _| Ok(sh_quote(s))),
    filter("dquote", "Quoting", "dquote", "Double quotes, escaping `\\ \" $` and the backtick.", "{{ input \"X\" | dquote }}", NONE, NO_NAMED, |_, s, _| Ok(dquote(s))),
    filter("json", "Quoting", "json", "JSON string literal.", "{{ input \"X\" | json }}", NONE, NO_NAMED, |_, s, _| Ok(json_string(s))),
    filter("urlencode", "Encoding", "urlencode", "Percent-encoding (unreserved characters are kept).", "{{ input \"Q\" | urlencode }}", NONE, NO_NAMED, |_, s, _| Ok(percent_encoding::utf8_percent_encode(s, URL_SET).to_string())),
    filter("base64", "Encoding", "base64", "Standard base64.", "{{ input \"X\" | base64 }}", NONE, NO_NAMED, |_, s, _| {
        use base64::Engine as _;
        Ok(base64::engine::general_purpose::STANDARD.encode(s))
    }),
    filter("base64d", "Encoding", "base64d", "Decodes standard base64 (invalid input is a failure).", "{{ input \"X\" | base64d }}", NONE, NO_NAMED, |_, s, _| {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .map_err(|_| Fail("base64d: invalid base64".into()))?;
        String::from_utf8(bytes).map_err(|_| Fail("base64d: decoded bytes are not UTF-8".into()))
    }),
    filter("sha256", "Hashing", "sha256", "Lower-case hex SHA-256.", "{{ input \"X\" | sha256 }}", NONE, NO_NAMED, |_, s, _| {
        use sha2::Digest as _;
        Ok(hex_of(&sha2::Sha256::digest(s.as_bytes())))
    }),
    filter("md5", "Hashing", "md5", "Lower-case hex MD5.", "{{ input \"X\" | md5 }}", NONE, NO_NAMED, |_, s, _| {
        use md5::Digest as _;
        Ok(hex_of(&md5::Md5::digest(s.as_bytes())))
    }),
    filter("basename", "Paths", "basename", "Last path component.", "{{ cwd | basename }}", NONE, NO_NAMED, |_, s, _| Ok(basename(s))),
    filter("dirname", "Paths", "dirname", "Path without its last component.", "{{ cwd | dirname }}", NONE, NO_NAMED, |_, s, _| Ok(dirname(s))),
];

pub fn lookup(name: &str) -> Option<&'static Def> {
    CATALOGUE.iter().find(|d| d.name == name)
}

pub fn names(kind: Kind) -> impl Iterator<Item = &'static str> {
    CATALOGUE
        .iter()
        .filter(move |d| d.kind == kind)
        .map(|d| d.name)
}

// ---------------------------------------------------------------------------------------------
// Helpers

fn fact(env: &mut Env, f: Fact) -> Result<String, Fail> {
    env.ctx.fact(&f).map_err(Fail)
}

fn rand_u64(ctx: &mut dyn Context) -> u64 {
    let mut b = [0u8; 8];
    ctx.fill_random(&mut b);
    u64::from_le_bytes(b)
}

/// Uniform in `0..n` (`n > 0`) by rejection sampling.
fn below(ctx: &mut dyn Context, n: u64) -> u64 {
    let limit = u64::MAX - (u64::MAX % n);
    loop {
        let x = rand_u64(ctx);
        if x < limit {
            return x % n;
        }
    }
}

fn capped(n: i64, what: &str) -> Result<usize, Fail> {
    if (0..=CAP).contains(&n) {
        Ok(n as usize)
    } else {
        Err(Fail(format!(
            "{what} must be between 0 and {CAP} (got {n})"
        )))
    }
}

fn hex_of(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn f_uuid(e: &mut Env, a: &Args) -> Result<String, Fail> {
    if a.s(0) == "v7" {
        let mut tail = [0u8; 10];
        e.ctx.fill_random(&mut tail);
        let ms = u64::try_from(e.now.timestamp_millis()).unwrap_or(0);
        Ok(uuid::Builder::from_unix_timestamp_millis(ms, &tail)
            .into_uuid()
            .hyphenated()
            .to_string())
    } else {
        let mut b = [0u8; 16];
        e.ctx.fill_random(&mut b);
        Ok(uuid::Builder::from_random_bytes(b)
            .into_uuid()
            .hyphenated()
            .to_string())
    }
}

fn f_ulid(e: &mut Env, _: &Args) -> Result<String, Fail> {
    let mut b = [0u8; 16];
    e.ctx.fill_random(&mut b);
    let ms = u64::try_from(e.now.timestamp_millis()).unwrap_or(0);
    Ok(ulid::Ulid::from_parts(ms, u128::from_le_bytes(b) & ((1u128 << 80) - 1)).to_string())
}

const NANOID: &[u8; 64] = b"_-0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

fn f_nanoid(e: &mut Env, a: &Args) -> Result<String, Fail> {
    let n = capped(a.named_int("len", 21)?, "len")?;
    let mut b = vec![0u8; n];
    e.ctx.fill_random(&mut b);
    Ok(b.iter()
        .map(|x| NANOID[(*x & 63) as usize] as char)
        .collect())
}

fn f_random(e: &mut Env, a: &Args) -> Result<String, Fail> {
    let (lo, hi) = (a.int(0)?, a.int(1)?);
    if lo > hi {
        return Err(Fail(format!("random: MIN {lo} is greater than MAX {hi}")));
    }
    let span = (hi as i128 - lo as i128 + 1) as u128;
    let off = if span > u64::MAX as u128 {
        rand_u64(e.ctx) as i128
    } else {
        below(e.ctx, span as u64) as i128
    };
    Ok((lo as i128 + off).to_string())
}

fn charset(name: &str) -> Result<Vec<char>, Fail> {
    let set = match name {
        "alnum" => "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
        "alpha" => "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "lower" => "abcdefghijklmnopqrstuvwxyz",
        "upper" => "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "digits" => "0123456789",
        "hex" => "0123456789abcdef",
        other => other,
    };
    let v: Vec<char> = set.chars().collect();
    if v.is_empty() {
        Err(Fail("random_string: charset is empty".into()))
    } else {
        Ok(v)
    }
}

fn f_random_string(e: &mut Env, a: &Args) -> Result<String, Fail> {
    let n = capped(a.int(0)?, "LEN")?;
    let set = charset(a.named("charset").unwrap_or("alnum"))?;
    Ok((0..n)
        .map(|_| set[below(e.ctx, set.len() as u64) as usize])
        .collect())
}

fn f_password(e: &mut Env, a: &Args) -> Result<String, Fail> {
    let n = capped(a.named_int("len", 20)?, "len")?;
    let symbols = a.named_bool("symbols", true)?;
    let classes: Vec<Vec<char>> = {
        let mut c = vec![
            "abcdefghijklmnopqrstuvwxyz".chars().collect::<Vec<_>>(),
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ".chars().collect(),
            "0123456789".chars().collect(),
        ];
        if symbols {
            c.push("!@#$%^&*()-_=+[]{}:,.?".chars().collect());
        }
        c
    };
    if n < classes.len() {
        return Err(Fail(format!(
            "password: len must be at least {}",
            classes.len()
        )));
    }
    let all: Vec<char> = classes.iter().flatten().copied().collect();
    let mut out: Vec<char> = classes
        .iter()
        .map(|c| c[below(e.ctx, c.len() as u64) as usize])
        .collect();
    while out.len() < n {
        out.push(all[below(e.ctx, all.len() as u64) as usize]);
    }
    for i in (1..out.len()).rev() {
        let j = below(e.ctx, i as u64 + 1) as usize;
        out.swap(i, j);
    }
    Ok(out.into_iter().collect())
}

fn f_hex(e: &mut Env, a: &Args) -> Result<String, Fail> {
    let mut b = vec![0u8; capped(a.int(0)?, "N")?];
    e.ctx.fill_random(&mut b);
    Ok(hex_of(&b))
}

fn f_base64_random(e: &mut Env, a: &Args) -> Result<String, Fail> {
    use base64::Engine as _;
    let mut b = vec![0u8; capped(a.int(0)?, "N")?];
    e.ctx.fill_random(&mut b);
    Ok(base64::engine::general_purpose::STANDARD.encode(b))
}

fn f_pick(e: &mut Env, a: &Args) -> Result<String, Fail> {
    if a.pos.is_empty() {
        return Err(Fail("pick: needs at least one argument".into()));
    }
    Ok(a.s(below(e.ctx, a.pos.len() as u64) as usize).to_string())
}

const LOREM: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua",
    "enim",
    "ad",
    "minim",
    "veniam",
    "quis",
];

fn f_lorem(_: &mut Env, a: &Args) -> Result<String, Fail> {
    let n = capped(a.named_int("words", 8)?, "words")?;
    Ok((0..n)
        .map(|i| LOREM[i % LOREM.len()])
        .collect::<Vec<_>>()
        .join(" "))
}

fn format_time(dt: DateTime<FixedOffset>, fmt: &str, a: &Args) -> Result<String, Fail> {
    use chrono::format::{Item, StrftimeItems};
    let dt = if a.named_bool("utc", false)? {
        dt.with_timezone(&FixedOffset::east_opt(0).expect("zero offset"))
    } else {
        dt
    };
    let items = StrftimeItems::new(fmt);
    if items.clone().any(|i| matches!(i, Item::Error)) {
        return Err(Fail(format!("invalid time format \"{fmt}\"")));
    }
    Ok(dt.format_with_items(items).to_string())
}

fn f_now(e: &mut Env, a: &Args) -> Result<String, Fail> {
    format_time(e.now, a.opt(0).unwrap_or("%Y-%m-%d %H:%M:%S"), a)
}
fn f_date(e: &mut Env, a: &Args) -> Result<String, Fail> {
    format_time(e.now, "%Y-%m-%d", a)
}
fn f_time(e: &mut Env, a: &Args) -> Result<String, Fail> {
    format_time(e.now, "%H:%M:%S", a)
}
fn f_timestamp(e: &mut Env, _: &Args) -> Result<String, Fail> {
    Ok(e.now.timestamp().to_string())
}
fn f_timestamp_ms(e: &mut Env, _: &Args) -> Result<String, Fail> {
    Ok(e.now.timestamp_millis().to_string())
}
fn f_iso8601(e: &mut Env, a: &Args) -> Result<String, Fail> {
    format_time(e.now, "%Y-%m-%dT%H:%M:%S%:z", a)
}

fn parse_offset(s: &str) -> Result<Duration, Fail> {
    let bad = || {
        Fail(format!(
            "bad offset \"{s}\" (use ±N and s, m, h, d or w, e.g. \"-7d\")"
        ))
    };
    let unit = s.chars().last().ok_or_else(bad)?;
    let n: i64 = s[..s.len() - unit.len_utf8()]
        .trim_start_matches('+')
        .parse()
        .map_err(|_| bad())?;
    let secs = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        'w' => 604_800,
        _ => return Err(bad()),
    };
    n.checked_mul(secs)
        .and_then(Duration::try_seconds)
        .ok_or_else(bad)
}

fn f_date_offset(e: &mut Env, a: &Args) -> Result<String, Fail> {
    let shifted = e
        .now
        .checked_add_signed(parse_offset(a.s(0))?)
        .ok_or_else(|| Fail("date_offset: result is out of range".into()))?;
    format_time(shifted, a.opt(1).unwrap_or("%Y-%m-%d"), a)
}

fn f_env(e: &mut Env, a: &Args) -> Result<String, Fail> {
    e.ctx
        .env(a.s(0))
        .ok_or_else(|| Fail(format!("environment variable {} is not set", a.s(0))))
}

// ---- git: `.git/HEAD` only, no process

fn find_git(start: &Path) -> Option<(PathBuf, PathBuf)> {
    for dir in start.ancestors() {
        let dot = dir.join(".git");
        if dot.is_dir() {
            return Some((dir.to_path_buf(), dot));
        }
        if dot.is_file() {
            let text = std::fs::read_to_string(&dot).ok()?;
            let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
            let p = Path::new(target);
            let gitdir = if p.is_absolute() {
                p.to_path_buf()
            } else {
                dir.join(p)
            };
            return Some((dir.to_path_buf(), gitdir));
        }
    }
    None
}

fn git_here(e: &mut Env) -> Result<(PathBuf, PathBuf), Fail> {
    if fact(e, Fact::IsRemote)? == "true" {
        return Err(Fail("not available in a remote pane".into()));
    }
    let cwd = fact(e, Fact::Cwd)?;
    find_git(Path::new(&cwd)).ok_or_else(|| Fail(format!("{cwd} is not inside a git repository")))
}

fn f_git_root(e: &mut Env, _: &Args) -> Result<String, Fail> {
    Ok(git_here(e)?.0.to_string_lossy().into_owned())
}

fn f_git_branch(e: &mut Env, _: &Args) -> Result<String, Fail> {
    let (_, gitdir) = git_here(e)?;
    let head = std::fs::read_to_string(gitdir.join("HEAD"))
        .map_err(|er| Fail(format!("cannot read HEAD: {er}")))?;
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        Ok(r.strip_prefix("refs/heads/")
            .unwrap_or(r.strip_prefix("refs/").unwrap_or(r))
            .to_string())
    } else if head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(head[..7].to_string())
    } else {
        Err(Fail("unrecognised HEAD".into()))
    }
}

// ---- Filters

fn split_words(s: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if c.is_uppercase() && prev_lower && !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower = c.is_lowercase() || c.is_numeric();
            cur.extend(c.to_lowercase());
        } else {
            prev_lower = false;
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn capitalise(w: &str) -> String {
    let mut c = w.chars();
    c.next().map_or_else(String::new, |f| {
        f.to_uppercase()
            .chain(c.flat_map(char::to_lowercase))
            .collect()
    })
}

fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(capitalise)
        .collect::<Vec<_>>()
        .join(" ")
}

fn camel_case(s: &str) -> String {
    split_words(s)
        .iter()
        .enumerate()
        .map(|(i, w)| if i == 0 { w.clone() } else { capitalise(w) })
        .collect()
}

fn f_truncate(_: &mut Env, s: &str, a: &Args) -> Result<String, Fail> {
    let n = capped(a.int(0)?, "N")?;
    let ell = a.named("ellipsis").unwrap_or("…");
    if s.chars().count() <= n {
        return Ok(s.to_string());
    }
    let keep = n.saturating_sub(ell.chars().count());
    let mut out: String = s.chars().take(keep).collect();
    if n >= ell.chars().count() {
        out.push_str(ell);
    }
    Ok(out)
}

pub fn sh_quote(s: &str) -> String {
    if s.is_empty() {
        "''".into()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn dquote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const URL_SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub fn basename(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() {
            String::new()
        } else {
            "/".into()
        };
    }
    t.rsplit('/').next().unwrap_or(t).to_string()
}

pub fn dirname(p: &str) -> String {
    let t = p.trim_end_matches('/');
    match t.rfind('/') {
        None => ".".into(),
        Some(i) => {
            let d = t[..i].trim_end_matches('/');
            if d.is_empty() { "/" } else { d }.to_string()
        }
    }
}
