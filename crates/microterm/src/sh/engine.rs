//! Runs `sh` input: lexes lines, expands sugar, handles meta commands and host escapes, and turns
//! everything else into `ctl` argv that `CtlArgs::try_parse_from` parses and `ctl_cmd::dispatch`
//! runs, so `sh` can do exactly what `ctl` can and nothing more (SPEC §12.7).

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command as Host, Stdio};
use std::sync::Arc;

use clap::Parser;
use mt_proto::{exit, CallerEnv};

use super::expand::{apply_sticky, expand_aliases, shorthand};
use super::lexer::{self, Conn, LexError, Stmt};
use super::live::Shared;
use super::session::{parse_meta, Meta, Session, Switch, META_HELP, META_WORDS};
use crate::client::Client;
use crate::ctl::{
    CtlAction, CtlArgs, PaneCmd, SkillCmd, StatusCmd, TabCmd, WindowCmd, WorkspaceCmd,
};
use crate::ctl_cmd::{self, Ctx};
use crate::output::{Body, CtlError, Output};
use crate::render::{render, render_error, render_warning, term_size, RenderOpts};

#[derive(Debug, Clone, Default)]
pub struct Opts {
    pub json: bool,
    pub readonly: bool,
    pub interactive: bool,
    /// `-e`: a script stops at the first failing command.
    pub errexit: bool,
    /// Offer `Close N panes? [y/N]` on exit 8 (interactive only).
    pub confirm_prompts: bool,
    pub socket: Option<PathBuf>,
}

/// Where a command's output goes.
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    /// Detected terminal facts; `tty` is overridden per statement.
    pub render: RenderOpts,
    /// Whether rich forms may be used at all (a terminal, not a script).
    pub rich: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Exit(i32),
}

pub struct Engine {
    pub opts: Opts,
    pub session: Session,
    /// `json on|off`; `--json` on the command line starts it on.
    pub json: bool,
    pub pager: bool,
    /// Lines run so far, for `history`.
    pub history: Vec<String>,
    pub shared: Option<Arc<Shared>>,
    client: Option<Client>,
    warned: HashSet<&'static str>,
    depth: usize,
}

/// Verbs that never change state, for `--readonly`.
pub fn readonly_allows(a: &CtlAction) -> bool {
    matches!(
        a,
        CtlAction::List
            | CtlAction::Events { .. }
            | CtlAction::Workspace(WorkspaceCmd::List)
            | CtlAction::Window(WindowCmd::List { .. })
            | CtlAction::Tab(TabCmd::List { .. })
            | CtlAction::Pane(
                PaneCmd::List { .. }
                    | PaneCmd::Get(_)
                    | PaneCmd::GetText { .. }
                    | PaneCmd::Wait { .. }
                    | PaneCmd::Status(StatusCmd::Get(_)),
            )
            | CtlAction::Skill(SkillCmd::Show(_))
    )
}

/// One line from the terminal, read a byte at a time straight from the descriptor: `std::io::stdin`
/// would buffer ahead and swallow what the user already typed for the next prompt.
#[allow(unsafe_code)]
fn read_answer() -> String {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        // SAFETY: reads at most one byte into a valid buffer.
        let n = unsafe { libc::read(0, byte.as_mut_ptr().cast(), 1) };
        if n <= 0 || byte[0] == b'\n' || line.len() > 64 {
            break;
        }
        line.push(byte[0]);
    }
    String::from_utf8_lossy(&line).into_owned()
}

/// Clap prints the command as `ctl …` (and the docs as `microterm ctl …`); inside the shell the
/// prefix does not exist, so it is removed from usage lines, examples and cross-references.
fn without_ctl_prefix(text: &str) -> String {
    text.replace("microterm ctl ", "")
        .replace("ctl --help", "help")
        .replace("`ctl ", "`")
        .replace("Usage: ctl ", "Usage: ")
        .lines()
        .map(|l| match l.strip_prefix("       ctl ") {
            Some(rest) => format!("       {rest}"),
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn say(w: &mut dyn Write, s: &str) {
    let _ = writeln!(w, "{s}");
}

impl Engine {
    pub fn new(opts: Opts, shared: Option<Arc<Shared>>) -> Engine {
        Engine {
            json: opts.json,
            pager: opts.interactive,
            opts,
            session: Session::default(),
            history: Vec::new(),
            shared,
            client: None,
            warned: HashSet::new(),
            depth: 0,
        }
    }

    /// Connects now (an interactive or scripted session needs an instance from the start).
    pub fn connect(&mut self) -> Result<(), CtlError> {
        let c = ctl_cmd::connect(self.opts.socket.as_deref())?;
        self.client = Some(c);
        Ok(())
    }

    pub fn is_connected(&self) -> bool {
        self.shared
            .as_ref()
            .map_or(self.client.is_some(), |s| s.is_connected())
    }

    /// Runs a script: lines are joined while a quote, a trailing `\` or a trailing `&&`/`||`
    /// continues them.
    pub fn run_script(&mut self, text: &str, io: &mut Io<'_>) -> Flow {
        let mut buf = String::new();
        for line in text.lines() {
            buf.push_str(line);
            match lexer::parse(&buf, &|n| self.session.lookup(n)) {
                Err(LexError::Incomplete("continuation")) => {
                    buf.pop();
                    continue;
                }
                Err(LexError::Incomplete(_)) => {
                    buf.push('\n');
                    continue;
                }
                _ => {}
            }
            let one = std::mem::take(&mut buf);
            if let Flow::Exit(c) = self.run_line(&one, io) {
                return Flow::Exit(c);
            }
        }
        if !buf.trim().is_empty() {
            return self.run_line(&buf, io);
        }
        Flow::Continue
    }

    /// Runs one (possibly compound) line.
    pub fn run_line(&mut self, line: &str, io: &mut Io<'_>) -> Flow {
        let stmts = match lexer::parse(line, &|n| self.session.lookup(n)) {
            Ok(s) => s,
            Err(e) => {
                say(io.err, &format!("microterm: {e}"));
                self.session.last = exit::USAGE;
                return self.after_statement();
            }
        };
        for st in stmts {
            match st.conn {
                Conn::And if self.session.last != 0 => continue,
                Conn::Or if self.session.last == 0 => continue,
                _ => {}
            }
            if let Flow::Exit(c) = self.run_stmt(st, io) {
                return Flow::Exit(c);
            }
            if let Flow::Exit(c) = self.after_statement() {
                return Flow::Exit(c);
            }
        }
        Flow::Continue
    }

    fn after_statement(&self) -> Flow {
        if self.opts.errexit && self.session.last != 0 {
            Flow::Exit(self.session.last)
        } else {
            Flow::Continue
        }
    }

    fn run_stmt(&mut self, st: Stmt, io: &mut Io<'_>) -> Flow {
        if let Some(cmd) = &st.host {
            self.session.last = self.host(cmd, io);
            return Flow::Continue;
        }
        let words = expand_aliases(st.words.clone(), &self.session.aliases);
        if words.is_empty() {
            return Flow::Continue;
        }
        if let Some(m) = parse_meta(&words) {
            return match m {
                Ok(Meta::Source(path)) => self.source(&path, io),
                Ok(m) => {
                    let (res, flow) = self.meta(m, io);
                    let code = self.emit(res, false, &st, io);
                    self.session.last = code;
                    flow.unwrap_or(Flow::Continue)
                }
                Err(msg) => {
                    say(io.err, &format!("microterm: {msg}"));
                    self.session.last = exit::USAGE;
                    Flow::Continue
                }
            };
        }
        self.session.last = self.run_ctl(words, &st, io);
        Flow::Continue
    }

    /// `!cmd`: a host command with the terminal's stdio.
    fn host(&mut self, cmd: &str, io: &mut Io<'_>) -> i32 {
        if self.opts.readonly {
            say(io.err, "microterm: --readonly: host commands are disabled");
            return exit::USAGE;
        }
        let _ = io.out.flush();
        match Host::new("sh").arg("-c").arg(cmd).status() {
            Ok(s) => s.code().unwrap_or(1),
            Err(e) => {
                say(io.err, &format!("microterm: cannot run sh: {e}"));
                exit::USAGE
            }
        }
    }

    fn source(&mut self, path: &str, io: &mut Io<'_>) -> Flow {
        if self.depth >= 8 {
            say(io.err, "microterm: source: nested too deeply");
            self.session.last = exit::USAGE;
            return Flow::Continue;
        }
        match std::fs::read_to_string(path) {
            Ok(text) => {
                self.depth += 1;
                let flow = self.run_script(&text, io);
                self.depth -= 1;
                flow
            }
            Err(e) => {
                say(io.err, &format!("microterm: source: {path}: {e}"));
                self.session.last = exit::NOT_FOUND;
                Flow::Continue
            }
        }
    }

    fn run_ctl(&mut self, words: Vec<String>, st: &Stmt, io: &mut Io<'_>) -> i32 {
        let mut argv = apply_sticky(shorthand(words), &self.session.sticky);
        let mut forced = false;
        loop {
            let mut full = vec!["ctl".to_string()];
            full.extend(argv.iter().cloned());
            let args = match CtlArgs::try_parse_from(&full) {
                Ok(a) => a,
                Err(e) => return self.clap_error(&e, io),
            };
            let json = args.json || self.json;
            if self.opts.readonly && !readonly_allows(&args.action) {
                let what: Vec<&str> = argv
                    .iter()
                    .map(String::as_str)
                    .take_while(|a| !a.starts_with('-'))
                    .take(3)
                    .collect();
                let e = CtlError::usage(format!(
                    "--readonly: `{}` would change state",
                    what.join(" ")
                ));
                return self.emit(Err(e), json, st, io);
            }
            self.first_use_warning(&args.action, io);
            let res = match args.action {
                CtlAction::Skill(cmd) => {
                    let _ = io.out.flush();
                    Ok(Output::none().with_code(microterm::agent::run_skill(&cmd, json)))
                }
                CtlAction::Mcp => Err(CtlError::usage(
                    "mcp serves stdio: run `microterm ctl mcp` outside the shell",
                )),
                action => self.dispatch_once(action, json, io),
            };
            if let Err(e) = &res {
                if e.is_confirm_required()
                    && !forced
                    && self.opts.interactive
                    && self.opts.confirm_prompts
                {
                    if self.confirm(e, io) {
                        forced = true;
                        argv.insert(2.min(argv.len()), "--force".into());
                        continue;
                    }
                    say(io.err, "microterm: cancelled, nothing was closed");
                    return exit::CONFIRM_REQUIRED;
                }
            }
            return self.emit(res, json, st, io);
        }
    }

    fn clap_error(&self, e: &clap::Error, io: &mut Io<'_>) -> i32 {
        use clap::error::ErrorKind;
        let text = without_ctl_prefix(&e.to_string());
        match e.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                say(io.out, text.trim_end());
                exit::OK
            }
            _ => {
                say(io.err, text.trim_end());
                exit::USAGE
            }
        }
    }

    fn confirm(&self, e: &CtlError, io: &mut Io<'_>) -> bool {
        let msg = e
            .rpc
            .as_ref()
            .map_or(e.message.as_str(), |r| r.message.as_str());
        // "…would ask before closing tab 3; pass --force …" → keep the part before the `;`.
        let head = msg.split(';').next().unwrap_or(msg).trim_end_matches('.');
        let _ = write!(io.err, "{head}. Close anyway? [y/N] ");
        let _ = io.err.flush();
        matches!(
            read_answer().trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        )
    }

    fn first_use_warning(&mut self, action: &CtlAction, io: &mut Io<'_>) {
        let (key, msg) = match action {
            CtlAction::Pane(PaneCmd::SendText { .. }) => (
                "send-text",
                "send-text types into another pane, including secrets you pass; such lines are not saved to the history",
            ),
            CtlAction::Record { input: true, .. } => (
                "record-input",
                "record --input also captures keystrokes, including passwords typed in that pane",
            ),
            _ => return,
        };
        if self.warned.insert(key) {
            let _ = io.err.write_all(render_warning(msg, &io.render).as_bytes());
        }
    }

    /// Runs one verb on the connection (made on demand). A dead connection is dropped so the next
    /// command reconnects; Ctrl-C shuts the connection down, which ends a blocked wait.
    fn dispatch_once(
        &mut self,
        action: CtlAction,
        json: bool,
        io: &mut Io<'_>,
    ) -> Result<Output, CtlError> {
        let mut client = match self.client.take() {
            Some(c) => c,
            None => {
                let c = ctl_cmd::connect(self.opts.socket.as_deref()).map_err(|e| {
                    if let Some(s) = &self.shared {
                        s.set_connected(false);
                    }
                    CtlError::from(e)
                })?;
                if let Some(s) = &self.shared {
                    s.set_connected(true);
                    s.invalidate();
                }
                c
            }
        };
        if self.opts.interactive {
            ctl_cmd::begin_foreground(Some(client.raw_fd()));
        }
        let res = {
            let mut ctx = Ctx {
                json,
                env: CallerEnv::from_env(),
                stream: &mut *io.out,
                allow_stdin: false,
                socket: self.opts.socket.clone(),
            };
            ctl_cmd::dispatch(&mut client, action, &mut ctx)
        };
        let interrupted = self.opts.interactive && ctl_cmd::end_foreground();
        let lost = res.as_ref().err().is_some_and(|e| e.lost);
        if interrupted || lost {
            if let Some(s) = &self.shared {
                s.set_connected(!lost || interrupted);
            }
        } else {
            self.client = Some(client);
        }
        match res {
            Err(_) if interrupted => {
                say(io.err, "^C");
                Err(CtlError::silent(130))
            }
            other => other,
        }
    }

    /// Renders a result to its destination (stdout, a pager, a file or a host pipe) and returns
    /// the exit code.
    fn emit(
        &mut self,
        res: Result<Output, CtlError>,
        json: bool,
        st: &Stmt,
        io: &mut Io<'_>,
    ) -> i32 {
        match res {
            Ok(out) => {
                let plain_dest = st.pipe.is_some() || st.redirect.is_some();
                let rich = io.rich && io.render.tty && !json && !plain_dest;
                let opts = RenderOpts {
                    tty: rich,
                    color: rich && io.render.color,
                    ..io.render
                };
                if let Body::Spawned { spawned, .. } = &out.body {
                    self.session.remember_spawned(spawned);
                }
                let text = render(&out, &opts);
                self.deliver(&text, rich, st, io);
                for note in &out.notes {
                    say(io.err, &format!("microterm: {note}"));
                }
                out.code
            }
            Err(e) => {
                let _ = io
                    .err
                    .write_all(render_error(&e, json, &io.render).as_bytes());
                e.code
            }
        }
    }

    fn deliver(&mut self, text: &str, rich: bool, st: &Stmt, io: &mut Io<'_>) {
        if text.is_empty() {
            return;
        }
        if let Some(r) = &st.redirect {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(r.append)
                .truncate(!r.append)
                .open(&r.path);
            match file.and_then(|mut f| f.write_all(text.as_bytes())) {
                Ok(()) => {}
                Err(e) => say(io.err, &format!("microterm: {}: {e}", r.path)),
            }
            return;
        }
        if let Some(cmd) = &st.pipe {
            let _ = io.out.flush();
            match Host::new("sh")
                .arg("-c")
                .arg(cmd)
                .stdin(Stdio::piped())
                .spawn()
            {
                Ok(mut child) => {
                    if let Some(mut stdin) = child.stdin.take() {
                        // A closed pipe (`| head -1`) is fine.
                        let _ = stdin.write_all(text.as_bytes());
                    }
                    let _ = child.wait();
                }
                Err(e) => say(io.err, &format!("microterm: cannot run sh: {e}")),
            }
            return;
        }
        if rich && self.pager && self.page(text, io) {
            return;
        }
        let _ = io.out.write_all(text.as_bytes());
        let _ = io.out.flush();
    }

    /// Long output on a terminal goes through `$PAGER` (`less -R`).
    fn page(&self, text: &str, io: &mut Io<'_>) -> bool {
        let Some((_, rows)) = term_size() else {
            return false;
        };
        if text.lines().count() < rows.saturating_sub(1) {
            return false;
        }
        let pager = std::env::var("PAGER")
            .ok()
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| "less -R".into());
        let _ = io.out.flush();
        match Host::new("sh")
            .arg("-c")
            .arg(&pager)
            .stdin(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
                true
            }
            Err(_) => false,
        }
    }

    fn ok_mark(io: &Io<'_>) -> &'static str {
        if io.render.unicode {
            "✔"
        } else {
            "ok"
        }
    }

    fn meta(&mut self, m: Meta, io: &mut Io<'_>) -> (Result<Output, CtlError>, Option<Flow>) {
        let text = |s: String| Ok(Output::text(s));
        let switch = |current: bool, sw: Switch, what: &str| -> (bool, String) {
            match sw {
                Switch::On => (true, format!("{what} on")),
                Switch::Off => (false, format!("{what} off")),
                Switch::Show => (
                    current,
                    format!("{what} is {}", if current { "on" } else { "off" }),
                ),
            }
        };
        let res = match m {
            Meta::Exit => return (Ok(Output::none()), Some(Flow::Exit(exit::OK))),
            Meta::Source(_) => unreachable!("handled by run_stmt"),
            Meta::Help(words) => self.help(&words),
            Meta::Use(u) => text(self.session.apply_use(u)),
            Meta::Set(None) => text(
                self.session
                    .vars
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Meta::Set(Some((k, v))) => {
                self.session.vars.insert(k, v);
                Ok(Output::none())
            }
            Meta::Unset(k) => {
                self.session.vars.remove(&k);
                Ok(Output::none())
            }
            Meta::Env => {
                let mut lines = vec![format!("context: {}", self.session.sticky.describe())];
                lines.push(format!("json: {}", if self.json { "on" } else { "off" }));
                lines.push(format!(
                    "readonly: {}",
                    if self.opts.readonly { "on" } else { "off" }
                ));
                lines.push(format!("?={}", self.session.last));
                lines.extend(self.session.vars.iter().map(|(k, v)| format!("{k}={v}")));
                text(lines.join("\n"))
            }
            Meta::History => text(
                self.history
                    .iter()
                    .enumerate()
                    .map(|(i, l)| format!("{:>4}  {l}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Meta::Json(sw) => {
                let (v, msg) = switch(self.json, sw, "json");
                self.json = v;
                text(msg)
            }
            Meta::Pager(sw) => {
                let (v, msg) = switch(self.pager, sw, "pager");
                self.pager = v;
                text(msg)
            }
            Meta::Notify(sw) => {
                let current = self.shared.as_ref().is_some_and(|s| s.notices());
                let (v, msg) = switch(current, sw, "notices");
                if let Some(s) = &self.shared {
                    s.set_notices(v);
                }
                text(msg)
            }
            Meta::Refresh => match &self.shared {
                Some(s) => {
                    s.invalidate();
                    match s.snapshot() {
                        Some(snap) => {
                            let n: usize = snap
                                .workspaces
                                .iter()
                                .flat_map(|w| &w.windows)
                                .flat_map(|w| &w.tabs)
                                .map(|t| t.panes.len())
                                .sum();
                            text(format!("{} refreshed ({n} panes)", Self::ok_mark(io)))
                        }
                        None => Err(CtlError::new(
                            exit::NO_INSTANCE,
                            "no running microterm instance",
                        )),
                    }
                }
                None => text("nothing to refresh (no completion cache in scripts)".into()),
            },
            Meta::Alias(None) => {
                let mut lines: Vec<String> = super::expand::BUILTIN_ALIASES
                    .iter()
                    .filter(|(n, _)| !self.session.aliases.0.contains_key(*n))
                    .map(|(n, v)| format!("{n}={}", v.join(" ")))
                    .collect();
                lines.extend(
                    self.session
                        .aliases
                        .0
                        .iter()
                        .map(|(n, v)| format!("{n}={}", v.join(" "))),
                );
                lines.sort();
                text(lines.join("\n"))
            }
            Meta::Alias(Some((name, value))) => {
                self.session.aliases.0.insert(name, value);
                Ok(Output::none())
            }
            Meta::Reconnect => {
                self.client = None;
                match self.connect() {
                    Ok(()) => {
                        if let Some(s) = &self.shared {
                            s.set_connected(true);
                            s.invalidate();
                        }
                        text(format!("{} connected", Self::ok_mark(io)))
                    }
                    Err(e) => {
                        if let Some(s) = &self.shared {
                            s.set_connected(false);
                        }
                        Err(e)
                    }
                }
            }
            Meta::Clear => {
                if io.rich && io.render.tty {
                    let _ = io.out.write_all(b"\x1b[2J\x1b[H");
                }
                Ok(Output::none())
            }
            Meta::Version => text(format!("microterm {}", env!("CARGO_PKG_VERSION"))),
        };
        (res, None)
    }

    fn help(&self, words: &[String]) -> Result<Output, CtlError> {
        use clap::CommandFactory;
        let words = expand_aliases(words.to_vec(), &self.session.aliases);
        if words.is_empty() {
            let width = META_WORDS
                .iter()
                .map(|(w, _)| w.len())
                .chain(
                    CtlArgs::command()
                        .get_subcommands()
                        .map(|c| c.get_name().len()),
                )
                .max()
                .unwrap_or(10);
            let mut s = String::from(
                "microterm sh: every `microterm ctl` command, without the `microterm ctl` prefix.\n\n\
                 Commands (help COMMAND for details, e.g. `help pane get`):\n",
            );
            for c in CtlArgs::command()
                .get_subcommands()
                .filter(|c| !matches!(c.get_name(), "help" | "mcp"))
            {
                let about = c.get_about().map(|a| a.to_string()).unwrap_or_default();
                let about = about.lines().next().unwrap_or("");
                s.push_str(&format!("  {:<width$}  {about}\n", c.get_name()));
            }
            s.push_str("\nShell commands (help COMMAND for details, e.g. `help use`):\n");
            for (w, d) in META_WORDS {
                s.push_str(&format!("  {w:<width$}  {d}\n"));
            }
            s.push_str(
                "\nExamples:\n  pane list 'title:*cargo*,!exited'\n  pane get 7        use pane 7  ·  cat  ·  type 7 'ls'\n  \
                 tab new --title build -- cargo watch -x test\n  \
                 pane get-text 7 | grep error   ·   !date   ·   list > tree.txt\n\n\
                 Aliases: ls=list ws=workspace w=window t=tab p=pane type=pane send-text cat=pane get-text\n\
                 Separators: ; && ||   Redirects: > >>   Host pipe: | cmd   Host command: !cmd\n\
                 Tab completes verbs, flags, IDs and selectors.",
            );
            return Ok(Output::text(s));
        }
        if let Some((_, detail)) = META_HELP.iter().find(|(w, _)| *w == words[0]) {
            return Ok(Output::text(*detail));
        }
        let mut argv = vec!["ctl".to_string()];
        argv.extend(words.iter().cloned());
        argv.push("--help".into());
        match CtlArgs::try_parse_from(&argv) {
            Err(e) if e.kind() == clap::error::ErrorKind::DisplayHelp => {
                Ok(Output::text(without_ctl_prefix(e.to_string().trim_end())))
            }
            Err(e) => Err(CtlError::usage(format!(
                "no help for `{}`: {}",
                words.join(" "),
                e.to_string().lines().next().unwrap_or("")
            ))),
            Ok(_) => Err(CtlError::usage("no help for that")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Run {
        engine: Engine,
        out: Vec<u8>,
        err: Vec<u8>,
    }

    impl Run {
        fn new(opts: Opts) -> Run {
            // A socket path that cannot exist, so nothing can connect.
            let mut opts = opts;
            opts.socket = Some(std::env::temp_dir().join("mt-sh-engine-absent.sock"));
            Run {
                engine: Engine::new(opts, None),
                out: Vec::new(),
                err: Vec::new(),
            }
        }

        fn line(&mut self, line: &str) -> Flow {
            let mut io = Io {
                out: &mut self.out,
                err: &mut self.err,
                render: RenderOpts::PLAIN,
                rich: false,
            };
            self.engine.run_line(line, &mut io)
        }

        fn script(&mut self, text: &str) -> Flow {
            let mut io = Io {
                out: &mut self.out,
                err: &mut self.err,
                render: RenderOpts::PLAIN,
                rich: false,
            };
            self.engine.run_script(text, &mut io)
        }

        fn out(&self) -> String {
            String::from_utf8_lossy(&self.out).into_owned()
        }

        fn err(&self) -> String {
            String::from_utf8_lossy(&self.err).into_owned()
        }
    }

    fn run() -> Run {
        Run::new(Opts::default())
    }

    #[test]
    fn variables_and_exit_status() {
        let mut r = run();
        r.line("set A=hello");
        r.line("set");
        assert_eq!(r.out(), "A=hello\n");
        assert_eq!(r.engine.session.last, 0);
        r.line("json maybe");
        assert_eq!(r.engine.session.last, exit::USAGE);
        assert!(r.err().contains("expected on or off"), "{}", r.err());
    }

    #[test]
    fn connectives_follow_the_exit_code() {
        let mut r = run();
        r.line("json maybe || set A=recovered");
        r.line("json maybe && set B=never");
        r.line("set C=1 ; set D=2");
        let vars = &r.engine.session.vars;
        assert_eq!(vars.get("A").map(String::as_str), Some("recovered"));
        assert!(!vars.contains_key("B"));
        assert!(vars.contains_key("C") && vars.contains_key("D"));
    }

    #[test]
    fn dollar_question_mark_expands() {
        let mut r = run();
        r.line("json maybe");
        r.line("set CODE=$?");
        assert_eq!(r.engine.session.vars["CODE"], "1");
    }

    #[test]
    fn use_shows_and_clears_the_context() {
        let mut r = run();
        r.line("use pane 7");
        r.line("use");
        assert!(r.out().ends_with("pane 7\n"), "{}", r.out());
        r.line("use -");
        assert!(r.engine.session.sticky.is_empty());
    }

    #[test]
    fn aliases_are_defined_and_listed() {
        let mut r = run();
        r.line("alias ll=pane list --json");
        r.line("alias");
        assert!(r.out().contains("ll=pane list --json"), "{}", r.out());
        assert!(r.out().contains("ls=list"));
    }

    #[test]
    fn usage_errors_exit_1_and_help_exits_0() {
        let mut r = run();
        r.line("pane frobnicate");
        assert_eq!(r.engine.session.last, exit::USAGE);
        assert!(r.err().contains("frobnicate"), "{}", r.err());
        r.line("pane get --help");
        assert_eq!(r.engine.session.last, exit::OK);
        assert!(r.out().contains("Show one pane"), "{}", r.out());
        r.line("help tab");
        assert!(r.out().contains("rename"), "{}", r.out());
        r.line("help tab");
        assert!(
            r.out().contains("Usage: tab [OPTIONS] <COMMAND>"),
            "{}",
            r.out()
        );
        assert!(!r.out().contains("ctl"), "{}", r.out());
        r.line("pane get --nosuch");
        assert!(!r.err().contains("ctl"), "{}", r.err());
        r.line("help p get");
        assert!(
            r.out().contains("Show one pane"),
            "alias expands: {}",
            r.out()
        );
    }

    #[test]
    fn help_lists_ctl_and_shell_commands_and_details_each() {
        let mut r = run();
        r.line("help");
        let out = r.out();
        for w in [
            "pane",
            "tab",
            "window",
            "workspace",
            "list",
            "events",
            "record",
            "use",
            "alias",
        ] {
            assert!(out.contains(&format!("  {w} ")), "{w} missing: {out}");
        }
        assert!(!out.contains("  mcp "), "mcp does not run in sh");
        for (word, _) in META_WORDS {
            let mut r = run();
            r.line(&format!("help {word}"));
            assert_eq!(r.engine.session.last, 0, "help {word}");
            assert!(r.out().contains(word), "help {word}: {}", r.out());
            assert!(
                r.out().lines().count() >= 3 || *word == "quit" || *word == "exit",
                "{word}"
            );
        }
        r.line("help nosuch");
        assert_eq!(r.engine.session.last, exit::USAGE);
    }

    #[test]
    fn lex_errors_are_reported_and_fail_the_line() {
        let mut r = run();
        r.line("pane list 'unterminated");
        assert_eq!(r.engine.session.last, exit::USAGE);
        assert!(r.err().contains("unterminated quote"), "{}", r.err());
    }

    #[test]
    fn readonly_rejects_mutating_verbs_before_connecting() {
        let mut r = Run::new(Opts {
            readonly: true,
            ..Opts::default()
        });
        for line in [
            "pane close --force",
            "tab new",
            "type 7 hi",
            "pane send-text --pane 7 hi",
            "workspace delete old",
            "reload-config",
            "pane run -- ls",
        ] {
            r.line(line);
            assert_eq!(r.engine.session.last, exit::USAGE, "{line}");
        }
        assert!(r.err().contains("--readonly"), "{}", r.err());
        assert!(!r.err().contains("no running"), "rejected client-side");
        r.line("!echo hi");
        assert_eq!(r.engine.session.last, exit::USAGE);
    }

    #[test]
    fn readonly_allows_the_read_only_verbs() {
        let mut r = Run::new(Opts {
            readonly: true,
            ..Opts::default()
        });
        for line in [
            "list",
            "pane list",
            "pane get 7",
            "cat 7",
            "tab list",
            "pane status get 3",
        ] {
            r.line(line);
            // Allowed verbs get as far as connecting, which fails here: exit 2.
            assert_eq!(r.engine.session.last, exit::NO_INSTANCE, "{line}");
        }
    }

    #[test]
    fn no_instance_exits_2() {
        let mut r = run();
        r.line("pane list");
        assert_eq!(r.engine.session.last, exit::NO_INSTANCE);
        assert!(
            r.err().contains("no running microterm instance"),
            "{}",
            r.err()
        );
        let mut r = run();
        r.line("pane list --json");
        assert!(r.err().contains(r#""code":"no_instance""#), "{}", r.err());
    }

    #[test]
    fn errexit_stops_a_script_at_the_first_failure() {
        let mut r = Run::new(Opts {
            errexit: true,
            ..Opts::default()
        });
        let flow = r.script("set A=1\njson maybe\nset B=2\n");
        assert_eq!(flow, Flow::Exit(exit::USAGE));
        assert!(r.engine.session.vars.contains_key("A"));
        assert!(!r.engine.session.vars.contains_key("B"));
        let mut r = run();
        assert_eq!(r.script("json maybe\nset B=2\n"), Flow::Continue);
        assert!(r.engine.session.vars.contains_key("B"));
    }

    #[test]
    fn scripts_join_continued_lines() {
        let mut r = run();
        r.script("set A=one \\\n  && set B='two\nthree'\n# comment\nset C=3");
        let vars = &r.engine.session.vars;
        assert_eq!(vars["A"], "one");
        assert_eq!(vars["B"], "two\nthree");
        assert_eq!(vars["C"], "3");
    }

    #[test]
    fn exit_leaves_with_status_zero() {
        let mut r = run();
        r.line("json maybe");
        assert_eq!(r.line("exit"), Flow::Exit(0));
        assert_eq!(r.line("quit"), Flow::Exit(0));
    }

    #[test]
    fn redirects_write_the_rendered_output() {
        let mut r = run();
        let file = std::env::temp_dir().join(format!("mt-sh-redirect-{}.txt", std::process::id()));
        let path = file.display().to_string();
        r.line(&format!("version > {path}"));
        r.line(&format!("version >> {path}"));
        let written = std::fs::read_to_string(&file).unwrap();
        assert_eq!(written.lines().count(), 2);
        assert!(written.starts_with("microterm "));
        assert_eq!(r.out(), "", "redirected output is not printed");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn host_pipes_feed_a_process() {
        let mut r = run();
        let file = std::env::temp_dir().join(format!("mt-sh-pipe-{}.txt", std::process::id()));
        r.line(&format!("version | tr a-z A-Z > {}", file.display()));
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.starts_with("MICROTERM "), "{written}");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn history_lists_numbered_lines() {
        let mut r = run();
        r.engine.history = vec!["list".into(), "pane get 7".into()];
        r.line("history");
        assert_eq!(r.out(), "   1  list\n   2  pane get 7\n");
    }

    #[test]
    fn mcp_is_refused_inside_the_shell() {
        let mut r = run();
        r.line("mcp");
        assert_eq!(r.engine.session.last, exit::USAGE);
        assert!(r.err().contains("outside the shell"), "{}", r.err());
    }
}
