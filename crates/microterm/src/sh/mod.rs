//! `microterm sh`: an interactive control shell over the `ctl` command set (SPEC §12.7).
//!
//! It adds no IPC methods and no capability: a line is `ctl`'s argv without `microterm ctl`,
//! parsed by the same clap types and run by the same `ctl_cmd::dispatch`. This module is the
//! entry point; `lexer`, `expand` and `session` are the pure grammar and state layers, `engine`
//! runs lines, `complete` and `repl` are the line editor.

mod complete;
mod engine;
mod expand;
mod history;
mod lexer;
mod live;
mod repl;
mod session;

use std::io::{IsTerminal, Read, Write};

use crate::cli::ShArgs;
use crate::output::CtlError;
use crate::render::{render_error, RenderOpts};
use engine::{Engine, Flow, Io, Opts};

/// Where the commands come from.
enum Source {
    Interactive,
    /// `-c CMDS`: a terminal still gets the rich forms.
    Commands(String),
    /// A file or piped stdin: always plain.
    Script(String),
}

fn read_script(args: &ShArgs) -> Result<Source, String> {
    if let Some(c) = &args.commands {
        return Ok(Source::Commands(c.clone()));
    }
    match &args.file {
        Some(f) if f.as_os_str() != "-" => std::fs::read_to_string(f)
            .map(Source::Script)
            .map_err(|e| format!("{}: {e}", f.display())),
        file => {
            let stdin = std::io::stdin();
            if file.is_none() && stdin.is_terminal() && std::io::stdout().is_terminal() {
                return Ok(Source::Interactive);
            }
            let mut text = String::new();
            stdin
                .lock()
                .read_to_string(&mut text)
                .map_err(|e| format!("stdin: {e}"))?;
            Ok(Source::Script(text))
        }
    }
}

pub fn run(args: ShArgs, config: Option<&std::path::Path>) -> i32 {
    let source = match read_script(&args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("microterm: {e}");
            return mt_proto::exit::USAGE;
        }
    };
    let interactive = matches!(source, Source::Interactive);
    // `[sh]` is read when the session starts; a broken config falls back to the defaults (the
    // GUI shows the error banner, and `microterm config check` explains it).
    let settings = mt_config::load::load(&mt_config::load::config_path(config))
        .map(|l| l.config.sh)
        .unwrap_or_default();
    let no_color = args.no_color || settings.color == mt_config::ShColor::Never;
    let mut render = RenderOpts::detect(no_color, false);
    if settings.color == mt_config::ShColor::Always && !no_color && !no_color_env() {
        render.err_color = true;
    }
    let opts = Opts {
        json: args.json,
        readonly: args.readonly,
        interactive,
        errexit: args.errexit,
        confirm_prompts: settings.confirm_prompts,
        socket: args.socket.clone(),
    };
    let shared = interactive.then(|| live::Shared::new(args.socket.clone()));
    let mut engine = Engine::new(opts, shared.clone());
    // Exit 2 when no instance can be reached at the start.
    if let Err(e) = engine.connect() {
        report(&e, args.json, &render);
        return e.code;
    }
    match source {
        Source::Interactive => repl::run(
            engine,
            shared.expect("interactive sessions have shared state"),
            render,
            !args.no_history,
            settings.history_send_text,
        ),
        Source::Commands(text) | Source::Script(text) => {
            let rich = matches!(args_mode(&args), Mode::Commands) && render.tty;
            let mut out = std::io::stdout();
            let mut err = std::io::stderr();
            let code = {
                let mut io = Io {
                    out: &mut out,
                    err: &mut err,
                    render,
                    rich,
                };
                match engine.run_script(&text, &mut io) {
                    Flow::Exit(c) => c,
                    Flow::Continue => engine.session.last,
                }
            };
            let _ = out.flush();
            code
        }
    }
}

fn no_color_env() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

enum Mode {
    Commands,
    Script,
}

fn args_mode(args: &ShArgs) -> Mode {
    if args.commands.is_some() {
        Mode::Commands
    } else {
        Mode::Script
    }
}

fn report(e: &CtlError, json: bool, render: &RenderOpts) {
    let _ = std::io::stderr().write_all(render_error(e, json, render).as_bytes());
}

#[cfg(test)]
mod tests {
    use clap::{Command, CommandFactory};

    use super::engine::{Engine, Flow, Io, Opts};
    use super::expand::BUILTIN_ALIASES;
    use super::session::{is_meta, META_WORDS};
    use crate::ctl::CtlArgs;
    use crate::render::RenderOpts;

    /// Every leaf subcommand path of the `ctl` tree (`["pane", "status", "set"]`, …).
    fn leaves(cmd: &Command, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        let subs: Vec<&Command> = cmd
            .get_subcommands()
            .filter(|s| s.get_name() != "help")
            .collect();
        if subs.is_empty() {
            out.push(prefix.clone());
        }
        for s in subs {
            prefix.push(s.get_name().to_string());
            leaves(s, prefix, out);
            prefix.pop();
        }
    }

    #[test]
    fn every_ctl_subcommand_is_reachable_from_sh() {
        let mut paths = Vec::new();
        leaves(&CtlArgs::command(), &mut Vec::new(), &mut paths);
        assert!(paths.len() > 25, "the clap tree is walked: {}", paths.len());
        for path in paths {
            assert!(
                !is_meta(&path[0]),
                "{path:?} is shadowed by a shell command"
            );
            // `--help` parses without a connection, so this proves the verb is routed to clap.
            let mut engine = Engine::new(Opts::default(), None);
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let mut io = Io {
                out: &mut out,
                err: &mut err,
                render: RenderOpts::PLAIN,
                rich: false,
            };
            let flow = engine.run_line(&format!("{} --help", path.join(" ")), &mut io);
            assert_eq!(flow, Flow::Continue);
            assert_eq!(
                engine.session.last,
                0,
                "{path:?}: {}",
                String::from_utf8_lossy(&err)
            );
            assert!(!out.is_empty(), "{path:?} printed no help");
            let text = String::from_utf8_lossy(&out);
            // `mcp` only runs outside the shell, so its prose may name `ctl`.
            assert!(
                path[0] == "mcp" || !text.contains("ctl "),
                "{path:?} help mentions the ctl prefix: {text}"
            );
        }
    }

    #[test]
    fn shell_words_and_aliases_never_shadow_ctl_commands() {
        let ctl: Vec<String> = CtlArgs::command_names();
        for (w, _) in META_WORDS {
            assert!(
                !ctl.iter().any(|c| c == w),
                "meta command {w} shadows a ctl command"
            );
        }
        for (a, _) in BUILTIN_ALIASES {
            assert!(
                !ctl.iter().any(|c| c == a),
                "alias {a} shadows a ctl command"
            );
            assert!(!is_meta(a), "alias {a} shadows a shell command");
        }
    }
}
