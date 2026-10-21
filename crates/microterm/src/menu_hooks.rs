//! The parts of context menus (SPEC §10.9) that need this crate: checking `ctl = "…"` strings
//! and `action = …` references at config load, and running `ctl` items.

use std::path::PathBuf;

use clap::Parser;
use mt_proto::CallerEnv;

use microterm::ctl::{CtlAction, CtlArgs, MenuCmd};

use crate::ctl_cmd::{connect, dispatch, Ctx};
use crate::render::{render, render_error, RenderOpts};

/// Parses a menu item's ctl argv with the `microterm ctl` grammar.
fn parse(argv: &[String]) -> Result<CtlArgs, String> {
    let args =
        CtlArgs::try_parse_from(std::iter::once("ctl".to_string()).chain(argv.iter().cloned()))
            .map_err(|e| {
                e.render()
                    .to_string()
                    .lines()
                    .next()
                    .unwrap_or("invalid ctl command")
                    .trim_start_matches("error: ")
                    .to_string()
            })?;
    match args.action {
        CtlAction::Menu(MenuCmd::Invoke { .. }) => {
            Err("`menu invoke` cannot be a menu item (no recursion)".into())
        }
        CtlAction::Mcp | CtlAction::Events { .. } | CtlAction::Record { .. } => {
            Err("this verb runs until interrupted and cannot be a menu item".into())
        }
        _ => Ok(args),
    }
}

fn check_ctl(raw: &str) -> Result<(), String> {
    let argv = mt_config::split_command(raw)?;
    parse(&argv).map(|_| ())
}

fn check_action(a: &mt_config::ActionRef) -> Result<(), String> {
    mt_ui::actions::resolve_action(&a.name, &a.arg).map(|_| ())
}

/// Runs a `ctl` item as the pane it was invoked on; returns its plain output.
fn run_ctl(argv: Vec<String>, pane: u64, socket: Option<PathBuf>) -> Result<String, String> {
    let args = parse(&argv)?;
    let mut client = connect(socket.as_deref()).map_err(|e| e.to_string())?;
    let mut sink = Vec::new();
    let mut ctx = Ctx {
        json: false,
        env: CallerEnv {
            pane: Some(pane),
            ..Default::default()
        },
        stream: &mut sink,
        allow_stdin: false,
        socket,
    };
    let opts = RenderOpts::PLAIN;
    match dispatch(&mut client, args.action, &mut ctx) {
        Ok(out) => Ok(render(&out, &opts)),
        Err(e) => Err(render_error(&e, false, &opts).trim().to_string()),
    }
}

pub fn install() {
    mt_config::menu::install_validators(mt_config::menu::Validators {
        action: check_action,
        ctl: check_ctl,
    });
    mt_ui::menu::install_ctl_runner(run_ctl);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctl_strings_are_checked_with_the_ctl_grammar() {
        assert!(check_ctl("pane zoom").is_ok());
        assert!(check_ctl("pane status set --state waiting 'look here'").is_ok());
        let e = check_ctl("pane zom").unwrap_err();
        assert!(e.contains("zom"), "{e}");
        assert!(check_ctl("menu invoke viewport/zoom")
            .unwrap_err()
            .contains("recursion"));
        assert!(check_ctl("events").is_err());
        assert!(check_ctl("pane 'unclosed").is_err());
    }

    #[test]
    fn installed_validators_reject_bad_items_at_load() {
        install();
        let e = mt_config::parse_str(
            "[[context_menu.viewport.items]]\nlabel = \"x\"\nctl = \"pane zom\"\n\n[[context_menu.viewport.items]]\nlabel = \"y\"\naction = \"NoSuchAction\"\n",
        )
        .unwrap_err();
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(
            e[0].message
                .starts_with("context_menu.viewport.items[0]: ctl \"pane zom\":"),
            "{}",
            e[0].message
        );
        assert_eq!(e[0].line, 3);
        assert_eq!(
            e[1].message,
            "context_menu.viewport.items[1]: unknown action \"NoSuchAction\""
        );
        assert!(mt_config::parse_str(
            "[[context_menu.viewport.items]]\nlabel = \"x\"\naction = { Split = \"left\" }\n"
        )
        .is_ok());
    }
}
