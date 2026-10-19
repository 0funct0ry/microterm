//! Agent integration (SPEC §12.5): the skill text, and the installer that places it where each
//! coding agent reads instructions. See docs/adr/0001-agent-integration.md for the locations.

use clap::ValueEnum;

pub mod install;
pub mod render;

pub const BEGIN: &str = "<!-- microterm:begin -->";
pub const END: &str = "<!-- microterm:end -->";
/// Directory (and skill name) used for Claude Code.
pub const SKILL_NAME: &str = "microterm-ctl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
    Antigravity,
    Generic,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Scope {
    User,
    Project,
}

use std::io::Write;

use crate::ctl::{SkillCmd, SkillOpts};

/// `microterm ctl skill …`. Needs no running instance. Returns the exit code.
pub fn run_skill(cmd: &SkillCmd, json: bool) -> i32 {
    let (opts, mode) = match cmd {
        SkillCmd::Install(o) => (o, Mode::Install),
        SkillCmd::Uninstall(o) => (o, Mode::Uninstall),
        SkillCmd::Show(o) => (o, Mode::Show),
    };
    match run(opts, mode, json) {
        Ok(()) => mt_proto::exit::OK,
        Err(msg) => {
            if json {
                eprintln!(
                    "{}",
                    serde_json::json!({"error":{"code":"bad_request","message":msg}})
                );
            } else {
                eprintln!("microterm: {msg}");
            }
            mt_proto::exit::USAGE
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Install,
    Uninstall,
    Show,
}

fn run(o: &SkillOpts, mode: Mode, json: bool) -> Result<(), String> {
    if mode == Mode::Show {
        let text = match o.agent {
            Agent::All => return Err("`skill show` needs one agent, not all".into()),
            a => render::for_agent(a),
        };
        let mut out = std::io::stdout().lock();
        if json {
            let _ = writeln!(
                out,
                "{}",
                serde_json::json!({"agent": format!("{:?}", o.agent).to_lowercase(), "content": text})
            );
        } else {
            let _ = out.write_all(text.as_bytes());
        }
        return Ok(());
    }
    let roots = install::Roots::from_env()?;
    let (targets, notes) = install::targets(o.agent, o.scope, o.dir.as_deref(), &roots)?;
    let mut report = Vec::new();
    for t in &targets {
        let action = if o.print {
            None
        } else if mode == Mode::Install {
            Some(install::install(t)?)
        } else {
            Some(install::uninstall(t)?)
        };
        report.push((t, action));
    }
    if json {
        let files: Vec<_> = report
            .iter()
            .map(|(t, a)| {
                serde_json::json!({
                    "agent": format!("{:?}", t.agent).to_lowercase(),
                    "path": t.path,
                    "action": a.map_or("dry-run", install::Action::as_str),
                })
            })
            .collect();
        println!("{}", serde_json::json!({"files": files, "notes": notes}));
        return Ok(());
    }
    for n in &notes {
        eprintln!("note: {n}");
    }
    for (t, a) in report {
        match a {
            Some(a) => println!("{} {}", a.as_str(), t.path.display()),
            None => {
                let verb = if mode == Mode::Install {
                    "would write"
                } else {
                    "would remove from"
                };
                println!("{verb} {}", t.path.display());
            }
        }
    }
    Ok(())
}
